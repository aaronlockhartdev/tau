# Delta: tools/skills

## ADDED

### R-tools-15  Skill directories load in a fixed order

THE SYSTEM SHALL load skills from `<project>/skills`, `<project>/.agents/skills`, `<root>/skills`, `<root>/.agents/skills`, then the configured extra directories, in that order.

Acceptance: @R-tools-15

### R-tools-16  Same-name skills stay separate

WHEN two directories hold a skill of the same name THE SYSTEM SHALL list both with their directory.

Acceptance: @R-tools-16

### R-tools-17  A skill is read-only

THE SYSTEM SHALL NOT write to a skill file.

Acceptance: @R-tools-17

### R-tools-18  A skill grants no permission

WHEN a skill requests a permission THE SYSTEM SHALL treat it as ungranted.

Acceptance: @R-tools-18
Why: a skill file arrives from a repository a user did not audit, and instruction text is not authorisation.

### R-tools-19  allowed-tools is enforced

WHEN a skill declares `allowed-tools` THE SYSTEM SHALL restrict that skill's tool calls to the listed names.

Acceptance: @R-tools-19

### R-tools-20  An unreadable skill is reported

WHEN a skill's frontmatter lacks a name or description THE SYSTEM SHALL skip it and report the path in its session start output.

Acceptance: @R-tools-20

### R-tools-21  A body loads on demand

WHEN a skill's description is in context THE SYSTEM SHALL keep its body out of the prompt until the path is read (R-context-09).

Acceptance: @R-tools-21

### R-tools-22  Loading is reported per session

WHEN a session starts THE SYSTEM SHALL list each loaded skill with its name, path, and description length in its session start output (R-context-10).

Acceptance: @R-tools-22
