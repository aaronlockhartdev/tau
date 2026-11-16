# Tasks: {id}

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [ ] spike: {measure} (gate: {n})
- [ ] R-<domain>-<NN>: delta clause in changes/{id}/specs/{domain}/…, then {implement} in {path}; archive applies the clause
- [ ] R-<domain>-<NN>: scenario in crates/tau-test/tests/features/{domain}.feature
- [ ] R-<domain>-<NN>: step {name} in crates/tau-test/src/steps/ (STEPS.md regenerates)
- [ ] archive: the cells the deltas move, by the numbers read off the files — a new leaf lands in its domain
index's Leaves cell as `leaf/spec.md (N requirements)` and in `spec/index.md`'s Leaves column as a link; a
clause added to an existing file moves that file's `Requirements: N` line; a clause added to a leaf moves the
leaf's count and the index Leaves entry that names it, never the index's own count; the Corpus count moves
by the clauses added
- [ ] check: nextest · features · spec --check · contracts --check · steps --check · clippy · fmt
- [ ] spec-review (reviewer ticks)
