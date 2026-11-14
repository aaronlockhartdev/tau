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
