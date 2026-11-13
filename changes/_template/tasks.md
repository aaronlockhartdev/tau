# Tasks: {id}

One line per committable unit. Implementation lines name an `R-ID`. Spikes and gates first.

- [ ] spike: {measure} (gate: {n})
- [ ] R-XXX-NN: delta clause in changes/{id}/specs/{domain}/…, then {implement} in {path}; archive applies the clause
- [ ] R-XXX-NN: scenario in crates/tau-test/tests/features/{domain}.feature
- [ ] R-XXX-NN: step {name} in crates/tau-test/src/steps/ (STEPS.md regenerates)
- [ ] check: nextest · features · spec --check · contracts --check · steps --check · clippy · fmt
- [ ] spec-review (reviewer ticks)
