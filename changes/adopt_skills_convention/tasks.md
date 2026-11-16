# Tasks: adopt_skills_convention

- [ ] Scenario: R-tools-14,15 discovery order, same-name skills separate
- [ ] Scenario: R-tools-16,17 read-only, no permission from a skill
- [ ] Scenario: R-tools-18,19 allowed-tools enforced, bad frontmatter reported
- [ ] Scenario: R-tools-20,21 body on demand, loading reported
- [ ] Implement: frontmatter parser with the convention's field names
- [ ] Implement: skill list into context assembly
- [ ] archive: `spec/tools/skills/spec.md` is a new leaf of 8 clauses, so `spec/tools/spec.md`'s Leaves cell
gains `skills/spec.md (8 requirements)` and its own count line is untouched — a leaf moves no index count —
while `add_turn_loop`'s domain-index delta may already have raised it, so read that number at archive;
`spec/index.md`'s Leaves column gains the link, and the Corpus count moves by 8
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
