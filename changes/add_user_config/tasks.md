# Tasks: add_user_config

- [ ] Scenario: R-config-01..03 one document, write preserves, live key applies
- [ ] Scenario: R-config-04,05 restart key named in file and announced
- [ ] Scenario: R-config-06 frontends share one config state
- [ ] Scenario: R-config-08 every listed key applies with its default
- [ ] Scenario: R-config-09 no config value in a session event
- [ ] Implement: TOML read, write, merge in core via `toml_edit`
- [ ] Implement: `notify` reload, announced on core's event stream
- [ ] Implement: frontends subscribe; ACP exposes the same state
- [ ] Implement: key table with defaults in the leaf spec
- [ ] Gate: no local state tracked covers config placement
- [ ] spec-review before commit
