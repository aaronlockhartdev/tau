# Delta: tools/editing

## ADDED

### R-tools-23  A read answers in anchored rows

WHEN a file is read THE SYSTEM SHALL return each line with a five-character anchor.

Acceptance: @R-tools-23

### R-tools-24  No line-number editor

THE SYSTEM SHALL NOT register a line-number or fuzzy-matching editor.

Acceptance: @R-tools-24
Why: a stale line number edits the wrong line silently, which is the failure the tool set exists to remove.

### R-tools-25  An anchor names one file

WHEN an anchor is used THE SYSTEM SHALL resolve it against the file it was served for.

Acceptance: @R-tools-25

### R-tools-26  A stale anchor is refused

WHEN an anchor no longer matches the served line THE SYSTEM SHALL refuse the edit with the current rows.

Acceptance: @R-tools-26

### R-tools-27  An edit returns its diff

WHEN an edit commits THE SYSTEM SHALL return the changed range with anchors.

Acceptance: @R-tools-27

### R-tools-28  Undo is one step

WHEN a tool call requests undo THE SYSTEM SHALL revert that session's last edit, and no earlier one.

Acceptance: @R-tools-28

### R-tools-29  The outline tool is off by default

WHERE `tools.outline` is off THE SYSTEM SHALL NOT register the outline tool.

Acceptance: @R-tools-29

### R-tools-30  An outline is a skeleton

WHEN the outline tool runs THE SYSTEM SHALL return one anchored declaration row per symbol, with bodies read on demand.

Acceptance: @R-tools-30
