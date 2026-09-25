# Native Jev task loop

`RunGoal` keeps a bounded desktop task inside the module. The caller supplies
the app, optional exact window, prepared values, allowed actions and targets,
and visible success conditions. `policy.rs` builds Jev's closed choices and
gates the selected operation and target probabilities independently. `screen.rs`
collects bounded accessibility observations. `verify.rs` checks the caller's
conditions without asking Jev to judge its own completion.

Each step observes, asks Jev to choose, reobserves the chosen target, acts once,
and observes again. A changed or ambiguous target stops before mutation. A
possibly delivered failed or timed-out action stops as `action_uncertain`; the
module does not retry it. OS permission errors come from the underlying desktop
command preflight. With `require_confirmations: true` (the default), a
consequential action returns a one-use continuation handle. The host may set
`false` when it has already authorized continuous desktop execution within the
request's action scope.

The result carries executed turns, provider metrics, a stop reason, and compact
predicate evidence. The only value echoed for a value predicate is its
caller-supplied expected string or fragment after a match; other field content
is not returned. An absence predicate is deliberately unavailable because a
bounded accessibility snapshot cannot prove an element is absent.
