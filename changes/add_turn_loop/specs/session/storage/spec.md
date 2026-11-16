# Delta: session/storage

Apply to `spec/session/storage/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-session-41  A session records its directory

WHEN a session starts THE SYSTEM SHALL record the working directory it opened at.
Acceptance: @R-session-41
Why: a resume runs in another directory, and the record must say where the session began.

### R-session-42  A session keeps its last model

WHEN a request sends a model THE SYSTEM SHALL record it as the session's last model.
Acceptance: @R-session-42
Why: a session switches models, so the row holds a last value, not a fixed one.

### R-session-43  A session keeps a display name

WHERE a session has no display name THE SYSTEM SHALL store the name its caller supplies, an adjective-noun
pair generated at the surface boundary.
Acceptance: @R-session-43

### R-session-44  A session key is UUID v7

WHEN the store records a session THE SYSTEM SHALL key it by a UUID version 7 in lowercase canonical text.
Acceptance: @R-session-44
Why: v7 sorts by creation time, so a session list and typed prefixes need no index.
