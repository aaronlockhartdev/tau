# Delta: session/memory

## ADDED

### R-session-19  Observations supersede a range

WHEN a session's tail passes the observation threshold THE SYSTEM SHALL append an observation block in the `observations` slot, naming the block range it supersedes.

Acceptance: @R-session-19
Why: replacing log rows loses the record, and an append-only log keeps the range and its summary.

### R-session-20  Reflections condense by appending

WHEN accumulated observations pass the reflection threshold THE SYSTEM SHALL append a reflection block naming the observations it condenses.

Acceptance: @R-session-20
Why: the condensation is a new fact, and the observations it summarises stay as their own record.

### R-session-21  Memory runs on the session's model

WHEN the observer or reflector runs THE SYSTEM SHALL use the session's own model as configured.

Acceptance: @R-session-21
Why: a second model config for memory is a second thing to keep in step with the provider.

### R-session-22  Memory cost stays in budget

THE SYSTEM SHALL bound observer and reflector cost by the context budget in user config.

Acceptance: @R-session-22
Why: a separate ceiling on memory cost is a second budget for the operator to reason about.

### R-session-23  The observer sees the agent's context

WHEN the observer runs THE SYSTEM SHALL give it the loaded context files.

Acceptance: @R-session-23
Why: an observer that skips `AGENTS.md` writes observations the agent's own rules forbid.

### R-session-24  Memory keeps the prompt prefix stable

WHEN memory updates THE SYSTEM SHALL append at the end of the prompt and leave the earlier prefix byte-identical.

Acceptance: @R-session-24
Why: a changed prefix invalidates the provider's prompt cache, which is the cost the layering exists to avoid.

### R-session-25  Thresholds come from config

THE SYSTEM SHALL read the observation and reflection thresholds from user config, at the defaults listed in `spec/config/reload/spec.md`.

Acceptance: @R-session-25
Why: a threshold baked into code cannot rise on a machine with room and fall on one without.

### R-session-26  Memory state reaches the frontend

WHILE memory updates THE SYSTEM SHALL expose observation and reflection counts to the connected frontend.

Acceptance: @R-session-26
Why: a compression that runs unseen looks like a stall, and counts are the smallest sign it works.

### R-session-27  Observations are stored

WHEN an observation is created THE SYSTEM SHALL write it to the session store.

Acceptance: @R-session-27
Why: an observation kept in prompt state dies with the process, and the log is the record.
