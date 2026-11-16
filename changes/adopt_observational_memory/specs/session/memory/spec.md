# Delta: session/memory

Apply to `spec/session/memory/spec.md` at archive time, as a new leaf of the `session` domain. Describe the difference, not the whole file.

## ADDED Requirements

### R-session-19  Observations supersede a range

WHEN a session's tail passes the observation threshold THE SYSTEM SHALL append an observation block in the `observations` slot, naming the block range it supersedes.
Acceptance: @R-session-19
Why: replacing log rows loses the record, and an append-only log keeps the range and its summary.

### R-session-20  Reflections condense by appending

WHEN accumulated observations pass the reflection threshold THE SYSTEM SHALL append a reflection block in the `reflections` slot, naming the observations it condenses.
Acceptance: @R-session-20
Why: the condensation is a new fact, and the observations it summarises stay as their own record.

### R-session-21  Memory runs on the session's model

WHEN the observer or reflector runs THE SYSTEM SHALL use the session's own model as configured.
Acceptance: @R-session-21
Why: a second model config for memory is a second thing to keep in step with the provider.

### R-session-22  Memory cost stays in budget

THE SYSTEM SHALL bound observer and reflector cost by the context budget in user config.
Acceptance: @R-session-22
Why: a summariser with no ceiling spends the budget it is summarising.

### R-session-23  Observer sees loaded context

THE SYSTEM SHALL give the observer the same context files the agent sees.
Acceptance: @R-session-23
Why: an observer blind to the project's rules summarises against them.

### R-session-24  Memory keeps the prompt prefix stable

WHEN an observation enters the context THE SYSTEM SHALL record it as a block in the `observations` slot and
leave the earlier prefix byte-identical.
Acceptance: @R-session-24
Why: a summary inserted into the prefix invalidates every cache entry behind it.

### R-session-25  Thresholds come from config

THE SYSTEM SHALL read the observation and reflection thresholds from user config, at the defaults user config lists.
Acceptance: @R-session-25
Why: a context budget the model's own window sets is one ceiling shared by two agents.

### R-session-26  Counts reach the frontend

THE SYSTEM SHALL report memory token counts to the frontend.
Acceptance: @R-session-26
Why: memory that spends tokens silently is a cost nobody sees.

### R-session-27  An observation outlives the process

WHEN a session resumes THE SYSTEM SHALL rebuild the observations from the log, not from prompt state alone.
Acceptance: @R-session-27
Why: prompt state dies with the process, and the log is what a resume reads.
