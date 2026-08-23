# Roadmap

What exists, what is next, and what is deliberately out of scope.

## Shipped

- the `tinydesktop-bus` wire contract: 54 member names, request payloads, the
  `DesktopResponse` envelope, and the contract version, in two pure-Rust
  dependencies
- the `tinydesktop` module: the vendored `agent-desktop` engine served over
  TinyBus, with a per-member permission preflight and blocking work kept off the
  dispatch task
- configuration from the loader: session, trace path and strictness, headed mode
- serde wire-form tests on every payload, exhaustive conversion tests on every
  shared enumeration, and in-memory bus tests on the served interface
- CI: format, clippy, build, test, per-file coverage, rustdoc, MSRV, contract
  dependency purity, and supply-chain checks
- a manual release workflow that versions, tags, builds the module for every
  supported platform, and creates a GitHub release with installable packages

## Next

- the members left out of contract version 1.0: session lifecycle, trace read
  and export, and the bundled skills loader. They are process-lifecycle concerns
  of a CLI rather than of a loaded module, so they need a design for how a
  module-scoped session is named before they are worth adding. Adding a member
  is a minor contract bump, which is why the bind rule allows it.
- a signal for surface changes, so a host can subscribe instead of polling
  `Wait`
- a live test suite, gated behind an environment variable and named `live_*`,
  running against a real application on macOS

## Out Of Scope

- a Linux accessibility backend. That belongs upstream in `agent-desktop`, and
  this module will pick it up through the gitlink.
- reimplementing anything the engine already does. This crate is an adapter: if
  a behavior is wrong, it is wrong in `vendor/agent-desktop` and gets fixed
  there.
- holding keyboard or mouse buttons across calls. `KeyDown`, `KeyUp`,
  `MouseDown`, and `MouseUp` fail closed on purpose and will stay that way until
  something owns the state they need.
- convenience wrappers that hide the envelope's error taxonomy from callers
