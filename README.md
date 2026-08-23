# tinydesktop

Native desktop automation as an installable TinyBus module.

tinydesktop wraps [`agent-desktop`](https://github.com/lahfir/agent-desktop) —
accessibility-tree observation and interaction for macOS, Windows, and Linux —
and serves it over TinyBus as fifty-four typed members. A host loads the
compiled `cdylib`, and an agent behind that host gets structured access to any
running application: no screenshots to interpret, no pixel matching, no browser.

It is a two-crate cargo workspace. `crates/tinydesktop-bus` is the wire contract
— member names, request payloads, the response envelope, and the contract
version, with no transport, no engine, and no behavior — and
`crates/tinydesktop` is the implementation, built as both an `rlib` and the
`cdylib` TinyBus loads. A host that only makes calls depends on the contract
crate alone and compiles neither the module, nor `tinybus`, nor a platform
accessibility backend.

## The model: observe, then act on what you observed

`Snapshot` walks an application's accessibility tree and hands back a compact
description in which every element carries a *ref* — a qualified handle like
`@s8f3k2p9:e1`. Interaction members take those refs. They do not take
coordinates, and they do not take selectors evaluated fresh at click time.

That indirection is the whole design. A ref is bound to the snapshot it came
from, so acting on one either reaches the element that was described or fails
with `STALE_REF` and asks for a fresh snapshot. What it will not do is click
whatever has since moved into that position.

```rust,ignore
use tinydesktop_bus::{names, DesktopResponse, RefRequest, SnapshotRequest};

let proxy = connection.proxy(names::INTERFACE, names::OBJECT_PATH, names::INTERFACE)?;

// `skeleton` caps the walk at three levels: structure without leaf detail,
// enough to decide where to look before spending a full walk on a subtree.
let tree: DesktopResponse = proxy
    .call(names::methods::SNAPSHOT, (SnapshotRequest {
        app: Some("Safari".to_owned()),
        skeleton: true,
        ..Default::default()
    },))
    .await?;

let clicked: DesktopResponse = proxy
    .call(names::methods::CLICK, (RefRequest::new("@s8f3k2p9:e1"),))
    .await?;
```

Two more properties worth knowing before you build on it:

**Headless by default.** A ref action goes through the platform's accessibility
API, not through synthesized input, so it does not steal focus, move the cursor,
or touch the pasteboard as a side effect. A run can proceed while someone else
is using the machine. Headed mode and the `input` members exist for the cases
that genuinely need a real cursor — both on purpose, and both the exception.

**Errors are replies, not failures.** Every member returns a `DesktopResponse`,
on success and on failure alike, carrying either the command's data or a
structured error with its code, its suggestion, whether a retry is safe, and how
to recover. A TinyBus error stays reserved for a transport or dispatch failure.
See [`crates/tinydesktop-bus/README.md`](crates/tinydesktop-bus/README.md).

## The member surface

Fifty-four members, listed in dispatch order by `tinydesktop_bus::names::METHODS`:

| Family | Members |
| --- | --- |
| Observation | `Snapshot` `Find` `Get` `Is` `Screenshot` |
| Interaction | `Click` `DoubleClick` `TripleClick` `RightClick` `Type` `SetValue` `Clear` `Focus` `Select` `Toggle` `Check` `Uncheck` `Expand` `Collapse` `Scroll` `ScrollTo` |
| Input | `Press` `KeyDown` `KeyUp` `Hover` `Drag` `MouseMove` `MouseClick` `MouseDown` `MouseUp` `MouseWheel` |
| Apps and windows | `Launch` `CloseApp` `ListApps` `ListWindows` `ListDisplays` `ListSurfaces` `FocusWindow` `ResizeWindow` `MoveWindow` `Minimize` `Maximize` `Restore` |
| Clipboard | `ClipboardGet` `ClipboardSet` `ClipboardClear` |
| Notifications | `ListNotifications` `NotificationAction` `DismissNotification` `DismissAllNotifications` |
| Waiting | `Wait` |
| System | `Version` `Status` `Permissions` |

`KeyDown`, `KeyUp`, `MouseDown`, and `MouseUp` validate their arguments and then
fail closed: holding a button down is stateful and this module is not, so a
success would be a lie the caller only discovers through a broken drag. They are
served rather than omitted so the refusal is a structured, explained reply
naming what to use instead, rather than an `UnknownMethod`.

Session lifecycle, trace read and export, and the engine's bundled skills loader
are deliberately absent from contract 1.0; see [`ROADMAP.md`](ROADMAP.md).

## Permissions and platforms

Desktop automation needs permissions a person grants — accessibility access, and
screen recording for captures. The module checks before it acts, because an
accessibility API called by an unauthorized process usually returns an *empty
tree* rather than an error: the snapshot succeeds, finds nothing, and the click
reports that the element is not there, which is indistinguishable from an
application that has no such button. Checking first turns that into a
`PERM_DENIED` naming the setting to change.

macOS and Windows have full accessibility backends. Linux builds, loads, and
answers, but implements no surfaces yet: observation there fails with
`PLATFORM_NOT_SUPPORTED`. That is inherited from the vendored engine and will
follow it.

## Layout

```text
Cargo.toml              # virtual workspace: members, shared metadata, lints
crates/
├── tinydesktop-bus/    # the wire contract — what crosses the bus
│   ├── README.md       # why the contract is its own crate
│   └── src/
│       ├── lib.rs      # crate docs + the entire public re-export surface
│       ├── names/      # interface, object path, one constant per member
│       ├── envelope/   # DesktopResponse and DesktopError
│       ├── vocabulary/ # enumerations shared across payloads
│       ├── observation/ interaction/ input/ apps/
│       ├── clipboard/ notifications/ waiting/ system/
│       └── version/    # contract version and the host bind rule
└── tinydesktop/        # the module — engine wrapper, adapter, and the cdylib
    ├── src/
    │   ├── lib.rs      # crate docs + public surface, re-exporting the contract
    │   ├── error/      # crate-wide `Error` and `Result<T>`
    │   ├── desktop/    # the engine: one method per member, split by family
    │   └── tinybus_module/   # bus interface, setup, and ABI v1 exports
    ├── tests/
    │   └── public_api.rs     # integration tests against the public API only
    └── examples/
        ├── basic.rs                  # ordinary library API usage
        ├── verify_module.rs          # local dynamic-module verification
        └── verify_github_release.rs  # tagged-release download and bus call
vendor/
├── tinybus/            # pinned TinyBus git submodule (host types, module SDK)
└── agent-desktop/      # pinned agent-desktop git submodule (the engine)
docs/
├── README.md           # documentation index and conventions
├── specs/              # behavior and architecture specifications
├── plans/              # implementation-ordered delivery plans
└── adr/                # immutable architecture decision records
```

The split is the point. A payload type describes what a frame carries; the
behavior that answers it is a different obligation. `tinydesktop` depends on
`tinydesktop-bus` and re-exports all of it, so `tinydesktop::SnapshotRequest`
and `tinydesktop_bus::SnapshotRequest` are the *same* type rather than
structural twins, and a host is never forced to choose between linking the whole
module and redefining the vocabulary.

Within each crate, feature areas use directory modules: implementation and
exports live in `mod.rs`, substantial types move to `types.rs`, and unit tests
live in `test.rs`. [`AGENTS.md`](AGENTS.md) holds the complete repository
guidance, and `CLAUDE.md` is a symlink to it so every coding agent reads one
source of truth.

## The vendored engine

`vendor/agent-desktop` is a git submodule pinned by its gitlink, the same way
TinyBus is. This repository never edits it: a behavior that is wrong is wrong
there, gets fixed there, and arrives here as a gitlink bump in its own commit.

`crates/tinydesktop` takes `agent-desktop-core` plus one of the three platform
adapter crates, selected by target. The contract crate takes neither — CI
asserts it.

## Development

Clone with submodules, or initialize them before building:

```sh
git submodule update --init --recursive
```

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo build --all-targets --all-features
cargo test --all-features
cargo run -p tinydesktop --example basic
cargo build -p tinydesktop --release --lib   # produces the installable cdylib
```

Those four checks are exactly what CI runs. Optional extras:

```sh
cargo doc --no-deps --all-features   # CI builds this with RUSTDOCFLAGS="-D warnings"
cargo deny check all                 # supply-chain check; see deny.toml
cargo install cargo-llvm-cov         # once, before running the coverage gate
.github/scripts/check-file-coverage.sh 90 coverage.json

# Load the built cdylib through the real TinyBus dynamic loader:
cargo run -p tinydesktop --example verify_module -- \
  target/release/libtinydesktop.so
```

The test suite is deterministic and needs no display server, no granted
permission, and nothing running: every assertion holds on a bare CI machine. It
covers the configuration parser, the contract-to-engine conversions, the
permission preflight, the serde wire form of every payload, and the served
interface over the in-memory transport. Anything that drives a real application
belongs in a live suite gated behind an environment variable.

## Releasing

Run the **Release** workflow from the Actions tab with a `patch`, `minor`, or
`major` bump. Use `current` only to resume an interrupted release whose version
commit and tag already exist. The workflow revalidates the workspace, versions
and tags it — one `[workspace.package]` version that every member inherits —
builds `crates/tinydesktop` as a TinyBus `cdylib`, and creates a GitHub release.
Assets follow `tinydesktop-<version>-<platform>.<tar.gz|zip>` and contain the
native module, its SHA-256 `modules.toml`, license, and
[`MODULE.md`](MODULE.md). Every release also publishes `checksum.toml`, which
TinyBus uses to verify an archive before extraction. The workflow loads the
published Ubuntu archive through TinyBus's GitHub release API and calls its
`Version` member before declaring the release successful. TinyBus itself is not
shipped by this repository; the pinned submodule is the build-time SDK. The
stable native matrix covers Ubuntu 22.04 and 24.04 on x86_64 and ARM64; Fedora
43 and 44 on x86_64 and ARM64; rolling Arch Linux on its officially supported
x86_64 architecture; macOS 15 and 26 on Intel and Apple Silicon; Windows Server
2022 and 2025 on x86_64; and Windows 11 on ARM64. Preview, deprecated, and
unofficial architecture images are not release gates. Do not hand-edit the
version in the root `Cargo.toml`.

## Documentation

- [`AGENTS.md`](AGENTS.md) — repository guidelines for humans and agents
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how to propose a change
- [`docs/specs/`](docs/specs/README.md) — behavior and architecture specs
- [`docs/plans/`](docs/plans/README.md) — test-first implementation plans
- [`docs/adr/`](docs/adr/0001-record-architecture-decisions.md) — architecture
  decision records
- [`SECURITY.md`](SECURITY.md) — how to report a vulnerability

## License

GPL-3.0-only. See [LICENSE](LICENSE). The vendored `agent-desktop` engine is
Apache-2.0; TinyBus is vendored under its own license.
