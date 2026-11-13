# Delta: workspace

Apply to `spec/workspace/spec.md` at archive time. Describe the difference, not the whole file.

## ADDED Requirements

### R-workspace-01  Flat members

THE SYSTEM SHALL list every workspace package in the root manifest through `members = ["crates/*"]`, with `resolver = "3"`.
Acceptance: gate workspace layout
Why: a named member list drops a new crate out of every gate silently; a glob cannot.

### R-workspace-02  Crate name equals directory

WHEN a package is added under `crates/` THE SYSTEM SHALL name the package after its directory.
Acceptance: gate workspace layout

### R-workspace-03  Local state out of git

THE SYSTEM SHALL keep `target/` and `.tau/` out of git through the root `.gitignore`.
Acceptance: gate no local state tracked
Why: `.tau/` holds sessions and machine-local state; a committed one is a leaked user transcript.

### R-workspace-04  Contracts committed with the types

THE SYSTEM SHALL keep the generated `contracts/` output committed with the wire types it came from.
Acceptance: gate contracts current

### R-workspace-05  Toolchain pinned to the named stable release

THE SYSTEM SHALL pin `rust-toolchain.toml` to the stable release named in `docs/stack.md`.
Acceptance: gate toolchain pinned
Why: a dependency's MSRV is a floor, not the pin; the named release is what a checkout builds on.

### R-workspace-06  A project's tau directory stays untracked

WHEN tau creates `<project>/.tau` inside a git working tree THE SYSTEM SHALL write `<project>/.tau/.gitignore` containing only `*`.

Acceptance: @R-workspace-06
Why: a store, config, and tapes sit in a checkout tau does not own; an ignore file inside the directory needs no tracked-file edit.

### R-workspace-07  The ignore file is written once

WHEN `<project>/.tau/.gitignore` exists THE SYSTEM SHALL leave it unchanged.

Acceptance: @R-workspace-07

### R-workspace-08  The write is disclosed

WHEN tau writes `<project>/.tau/.gitignore` THE SYSTEM SHALL name it in its session start output.

Acceptance: @R-workspace-08
