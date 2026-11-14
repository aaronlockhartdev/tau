# Tasks: add_realtime_gui_output

- [ ] Scenario: R-surface-08,09 one stream drives both frontends
- [ ] Scenario: R-surface-10..12 closed block kept, never re-parsed, tail only
- [ ] Scenario: R-surface-13..15 open block, close render, table grid
- [ ] Scenario: R-surface-16 ATX headings only
- [ ] Scenario: R-protocol-01, R-surface-23 the stream carries the event time, a block shows it
- [ ] Implement: the event time on the event type in crates/tau-protocol; `contracts/` regenerates
- [ ] Implement: block tree as `TextView` rows in `gpui::list`
- [ ] Implement: tail-only re-parse per token delta
- [ ] Implement: monospace open block; styled render at close
- [ ] Measure: frame budget at `spike_gpui_surface`
- [ ] Gate: live output under load; desktop for IME and prompt field
- [ ] spec-review before commit
