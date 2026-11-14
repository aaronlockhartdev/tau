# Tasks: adopt_property_lane

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [x] spike: `cargo nextest run -P ci` with proptest's 3 properties (gate: the lane stays under 5s)
- [x] R-harness-10: delta clause in changes/adopt_property_lane/specs/harness/spec.md, then the properties in `crates/tau-tools/src/bin/spec.rs` and `proptest` in `[workspace.dependencies]`; archive applies the clause
- [x] check: nextest · features · spec --check · contracts --check · steps --check · clippy · fmt
- [x] docs: `proptest` pin in the `docs/stack.md` test-lane row
- [x] spec-review (reviewer ticks)

Deferred, with the trigger each waits for: snapshot tests on `contracts/` when `generated()` returns a non-empty set; `loom` when a crate holds concurrent code; `cargo-fuzz` when a parser takes untrusted bytes.
