# Delta: config/reload

## ADDED

### R-config-01  One config document

WHERE a session runs THE SYSTEM SHALL read user configuration from one document at `<agent-dir>/tau/config.toml`.

Acceptance: @R-config-01

### R-config-02  A write keeps the file human-owned

WHEN tau writes the config file THE SYSTEM SHALL preserve comments, formatting, and key order.

Acceptance: @R-config-02

### R-config-03  A live key applies without a restart

WHERE a config key takes effect live THE SYSTEM SHALL apply a changed value to the running session without a restart.

Acceptance: @R-config-03

### R-config-04  A restart key is named in the file

THE SYSTEM SHALL mark every config key that takes effect only after a restart in the config file.

Acceptance: @R-config-04
Why: the file comment is the only place a user reads the difference before changing the key.

### R-config-05  A restart key is announced at the change

WHEN a restart-required key changes THE SYSTEM SHALL report that the new value awaits a restart.

Acceptance: @R-config-05

### R-config-06  Frontends share one config state

WHILE a session is connected to more than one frontend THE SYSTEM SHALL report the same config state to each.

Acceptance: @R-config-06

### R-config-07  The config file stays outside the repository

THE SYSTEM SHALL read and write the config file outside any path under git.

Acceptance: gate no local state tracked

### R-config-08  Every listed key is accepted

WHEN a session starts THE SYSTEM SHALL accept every config key listed in `spec/config/reload/spec.md` with its default.

Acceptance: @R-config-08
Why: a key absent from the table is set by reading source, so the table is the key list.

### R-config-09  No config value reaches a session log

THE SYSTEM SHALL write no config value into a session event.

Acceptance: @R-config-09
Why: the file carries provider keys, and events are exported, copied, and shared.
