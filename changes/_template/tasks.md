# Tasks: {id}

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [ ] spike: {measure} (gate: {n})
- [ ] R-<domain>-<NN>: delta clause in changes/{id}/specs/{domain}/…, then {implement} in {path}; archive applies the clause
- [ ] R-<domain>-<NN>: scenario in crates/tau-test/tests/features/{domain}.feature
- [ ] R-<domain>-<NN>: step {name} in crates/tau-test/src/steps/ (STEPS.md regenerates)
- [ ] archive: the cells the deltas move — each target leaf's count, each domain index's count and Leaves
cell, `spec/index.md`'s Corpus count and Leaves cells — by the numbers read off the files
- [ ] check: nextest · features · spec --check · contracts --check · steps --check · clippy · fmt
- [ ] spec-review (reviewer ticks)
