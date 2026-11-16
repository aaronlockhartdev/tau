# Tasks: add_subagent_support

- [ ] Scenario: R-tools-01..05 definition load, `task`, `send`, targets
- [ ] Scenario: R-tools-06..09 caps and inherited permissions
- [ ] Scenario: R-tools-10..13 fork history, cache, no spawn, cost
- [ ] Scenario: R-session-14..17 parentage, directed messages, cascade
- [ ] Scenario: R-acp-01..05 announce, deliver, states, approval, steer
- [ ] Scenario: R-surface-19..22 roster, count, attach, per-child cancel
- [ ] Implement: `task` and `send`, with the target roster in context
- [ ] Implement: `subagent_update` and directed messages in `tau-protocol`
- [ ] Implement: roster view, attach, steer, cancel
- [ ] Implement: directed-message block kinds. The parent column is not to be added: `add_session_storage` landed it as `sessions.parent` (R-session-08), and `Store::fork` writes it.
- [ ] R-config-13: `subagent.max_concurrent` (20) and `subagent.max_depth` (3) land in
`docs/stack.md`'s Config row with their defaults, that row becoming the key list when R-config-13
lands
- [ ] Verify: contracts current after the new wire types
- [ ] Eval: a fan-out review task, on demand
- [ ] archive: four new leaves, so four domain index cells and the Corpus count move by the numbers read
off the files — `spec/tools/spec.md` +13 with its first Leaves cell (`delegation`), `spec/acp/spec.md` +6
(`subagent`), `spec/surface/spec.md` +4 (`roster`), `spec/session/spec.md` +4 (`child`, joining its
existing leaves); `spec/index.md`'s Corpus count and those four domains' Leaves cells move to match
- [ ] check: `cargo run -p tau-tools --bin gates`; rows in state M0 reported `skipped (M0)`, manual rows by hand
- [ ] spec-review before commit
