# Tasks: add_test_lanes

- [ ] Implement: cucumber world driven by the tape (R-harness-04,05)
- [ ] Implement: headless surface tests through `TestWindowExt` (R-harness-06)
- [ ] Gate: behaviour and unit + surface run per commit (R-harness-07)
- [ ] Gate: CI builds `crates/tau-app` (R-harness-09)
- [ ] Fix in place: a lane over the commit window (R-harness-08)
- [ ] docs: clause layout in `specs/harness/lanes/spec.md` — `Acceptance:` adjacent to the obligation, the
form of `changes/_template/specs/_domain_/_area_/spec.md`; its 6 clauses sit in the blank-line form
- [ ] archive: `spec/harness/lanes/spec.md` is a new leaf, so `spec/harness/spec.md`'s count and Leaves
cell, `spec/index.md`'s harness Leaves cell and the Corpus count move by the numbers read off the files
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
