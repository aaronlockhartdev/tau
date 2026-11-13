# Delta: session/memory

## ADDED

### R-session-19  Messages become observations

WHEN a session's tail passes the observation threshold THE SYSTEM SHALL replace the oldest messages with observations.

Acceptance: @R-session-19

### R-session-20  Observations become reflections

WHEN accumulated observations pass the reflection threshold THE SYSTEM SHALL condense them into a reflection.

Acceptance: @R-session-20

### R-session-21  Memory runs on the session's model

WHEN the observer or reflector runs THE SYSTEM SHALL use the session's own model as configured.

Acceptance: @R-session-21

### R-session-22  Memory cost stays in the config budget

THE SYSTEM SHALL bound observer and reflector cost by the context budget in user config.

Acceptance: @R-session-22

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

### R-session-26  Memory state reaches the frontend

WHILE memory updates THE SYSTEM SHALL expose observation and reflection counts to the connected frontend.

Acceptance: @R-session-26

### R-session-27  Observations are stored

WHEN an observation is created THE SYSTEM SHALL write it to the session store.

Acceptance: @R-session-27
