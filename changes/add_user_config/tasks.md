# Tasks: add_user_config

- [ ] Scenario: R-config-01..03 root, XDG layer, project layer merged
- [ ] Scenario: R-config-04,05 write preserves, live key applies
- [ ] Scenario: R-config-06,07 restart key marked in file and announced
- [ ] Scenario: R-config-08 frontends share one config state
- [ ] Scenario: R-config-09 every listed key applies with its default
- [ ] Scenario: R-config-10 no config value in a session event
- [ ] Scenario: R-config-11,12 table merge, array replace, reload either layer
- [ ] Implement: TOML read, write, and layer merge in core via `toml_edit`
- [ ] Implement: `notify` on both layers, announced on core's event stream
- [ ] Implement: frontends subscribe; ACP exposes the same state
- [ ] Implement: key table with defaults in the leaf spec
- [ ] spec-review before commit
