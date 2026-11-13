# Delta: context/load-order

## ADDED

### R-context-01  The root guide always loads

WHERE a session runs in a repository THE SYSTEM SHALL load that repository's root `AGENTS.md`.

Acceptance: @R-context-01

### R-context-02  The nearest guide wins

WHEN a subtree of the working directory holds its own `AGENTS.md` THE SYSTEM SHALL load the nearest one to the path a tool call names, in place of the root guide for that call.

Acceptance: @R-context-02

### R-context-03  AGENTS.md wins over CLAUDE.md

WHEN `AGENTS.md` and `CLAUDE.md` sit in one directory THE SYSTEM SHALL load `AGENTS.md` in place of `CLAUDE.md`.

Acceptance: @R-context-03
Why: the file's own name is the tie-break a user can see, and a mirror that wins teaches a private rule.

### R-context-04  Pi directories are never read

THE SYSTEM SHALL NOT load context files from any `.pi/` directory.

Acceptance: @R-context-04

### R-context-05  The project context directory is `.tau`

WHERE `<project>/.tau/` holds context files THE SYSTEM SHALL load them in place of the agent-directory files of the same name.

Acceptance: @R-context-05

### R-context-06  SYSTEM.md replaces the prompt

WHEN a `SYSTEM.md` exists at a loaded layer THE SYSTEM SHALL use its body as the system prompt in place of tau's default.

Acceptance: @R-context-06

### R-context-07  APPEND_SYSTEM.md appends

WHEN an `APPEND_SYSTEM.md` exists at a loaded layer THE SYSTEM SHALL append its body after the system prompt.

Acceptance: @R-context-07

### R-context-08  One file loads per role

WHEN a context file of a role exists at more than one layer THE SYSTEM SHALL load the project layer's file alone for that role.

Acceptance: @R-context-08

### R-context-09  Nothing else loads unasked

THE SYSTEM SHALL load no document as context unless a loaded file names its path.

Acceptance: @R-context-09
Why: an auto-loaded document is a token bill every turn pays, and its absence from the report is invisible.

### R-context-10  The loaded set is reported

WHEN a session starts THE SYSTEM SHALL list each context file it loaded, by path, in its session start output.

Acceptance: @R-context-10
