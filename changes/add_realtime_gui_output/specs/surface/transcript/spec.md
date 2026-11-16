# Delta: surface/transcript

Apply to `spec/surface/transcript/spec.md` at archive time. Describe the difference, not the whole file.
Apply to `spec/surface/transcript/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-surface-08  One stream carries every state change

WHILE a session runs THE SYSTEM SHALL publish every state change on core's event stream.

Acceptance: @R-surface-08
Why: a second path to the screen is a second reconciliation to keep equal, and a poll interval is the lag ceiling.

### R-surface-09  Frontends render the stream

WHILE a frontend displays a session THE SYSTEM SHALL update it from core's event stream alone.

Acceptance: @R-surface-09

### R-surface-10  A closed block keeps its layout

WHEN a markdown block closes THE SYSTEM SHALL retain its layout for the life of the transcript.

Acceptance: @R-surface-10

### R-surface-11  A closed block is never re-parsed

WHEN a token delta arrives THE SYSTEM SHALL leave every closed markdown block's render untouched.

Acceptance: @R-surface-11

### R-surface-12  Only the tail re-parses

WHILE a markdown block is open THE SYSTEM SHALL re-parse the tail block for each token delta.

Acceptance: @R-surface-12

### R-surface-13  An open block shows as text

WHILE a fenced block is open THE SYSTEM SHALL show it as monospace text.

Acceptance: @R-surface-13

### R-surface-14  A closed block renders in full

WHEN a fenced block closes THE SYSTEM SHALL render it as styled content.

Acceptance: @R-surface-14

### R-surface-15  A table lays out at close

WHEN a table block closes THE SYSTEM SHALL lay it out as a grid.

Acceptance: @R-surface-15

### R-surface-16  ATX headings only

THE SYSTEM SHALL treat only ATX heading syntax as a heading.

Acceptance: @R-surface-16
Why: a setext underline restyles the lines above it, which breaks a closed block's retained layout.

### R-surface-17  The prompt field stays usable

WHILE tokens stream at full rate THE SYSTEM SHALL accept typed text, IME composition, and paste in the prompt field.

Acceptance: gate desktop

### R-surface-18  Frame cost stays in budget

WHILE the transcript renders a tape at full token rate, with a storage writer and a checkpoint running
alongside, THE SYSTEM SHALL keep its frame cost inside the budget the surface spike records.

Acceptance: gate live output under load

### R-surface-23  A block shows its event time

WHEN a block renders THE SYSTEM SHALL label it with the event time of its first event.

Acceptance: @R-surface-23
