# Delta: config/reload

## ADDED

### R-config-01  One root for a user's global files

THE SYSTEM SHALL keep a user's global files under one root, `~/.tau` by default.

Acceptance: @R-config-01

### R-config-02  XDG config home replaces the global file

WHEN `$XDG_CONFIG_HOME` is set and non-empty THE SYSTEM SHALL read the global layer from `$XDG_CONFIG_HOME/tau/config.toml` in place of `<root>/config.toml`.

Acceptance: @R-config-02

### R-config-03  A project file stacks on the global file

WHERE `<project>/.tau/config.toml` exists THE SYSTEM SHALL read configuration from the merge of the global layer and that file, the project value winning.

Acceptance: @R-config-03
Why: a project that changes one model key must not have to restate the whole global file to do it.

### R-config-04  A write keeps the file human-owned

WHEN tau writes a config file at either layer THE SYSTEM SHALL preserve its comments, formatting, and key order.

Acceptance: @R-config-04

### R-config-05  A live key applies without a restart

WHERE a config key takes effect live THE SYSTEM SHALL apply a changed value to the running session without a restart.

Acceptance: @R-config-05

### R-config-06  A restart key is marked in the file

THE SYSTEM SHALL mark every config key that takes effect only after a restart in the config file.

Acceptance: @R-config-06
Why: the file comment is the only place a user reads the difference before changing the key.

### R-config-07  A restart key is announced at the change

WHEN a restart-required key changes THE SYSTEM SHALL report that the new value awaits a restart.

Acceptance: @R-config-07

### R-config-08  Frontends share one config state

WHILE a session is connected to more than one frontend THE SYSTEM SHALL report the same config state to each.

Acceptance: @R-config-08

### R-config-09  Every listed key is accepted

WHEN a session starts THE SYSTEM SHALL accept every config key listed in `spec/config/reload/spec.md` with its default.

Acceptance: @R-config-09
Why: a key absent from the table is set by reading source, so the table is the key list.

### R-config-10  No config value reaches a session log

THE SYSTEM SHALL write no config value into a session event.

Acceptance: @R-config-10
Why: the file carries provider keys, and events are exported, copied, and shared.

### R-config-11  A table merges, an array replaces

WHEN the project layer sets a key whose global value is a table THE SYSTEM SHALL merge that table key by key, and SHALL replace an array wholesale.

Acceptance: @R-config-11
Why: appending arrays across layers makes a project's tool list depend on an unseen global file.

### R-config-12  Either layer reloads live

WHEN a config file at either layer changes on disk THE SYSTEM SHALL rebuild the merged view under R-config-05.

Acceptance: @R-config-12
