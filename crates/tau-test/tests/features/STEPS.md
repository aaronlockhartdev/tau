# Step vocabulary

The allowed step vocabulary: a scenario uses only steps listed here, and adding a step is a code change listed in `tasks.md`. Generated from the step definitions in `crates/tau-test/src/steps/`: do not edit by hand.

- Given the workspace manifest
- When the members are listed
- Then the members come from the crates glob
- When the crates are read
- Then every crate is named after its directory
- Given the git index
- When the local state paths are listed
- Then target and .tau are untracked
- Given an open store
- When the store is opened again
- Then the second open is refused
- When a tool call commits its events and its state change
- Then the events and the state change are stored together
- When a tool call's commit is refused
- Then the refused commit stores nothing
- When an event for a turn with no state row is inserted
- When a late event for the committed turn is inserted
- When a stored event row is updated
- When a stored event row is deleted
- Then the store refuses the write
- Then the stored events carry the times they were given
- Then the session records a creation time
