# adopt_context_file_loading

## Context

tau loads external context (instructions, prompts) with the same files, order, and precedence as
`pi`, so one loading model governs v1 and existing conventions keep working.

## Decision

Mirror `pi` 1.5.2: `docs/configuration.md` → *Agent directory*, *Project `.pi` directory*,
*Context files*. The reference defines the behaviour (paths, precedence, override scope, trust);
this spec states only that tau matches it and stays in sync when pi changes.

Consequences owned by tau: project files apply only after project trust; nested `AGENTS.md` is
per-directory nearest-wins; tau's own state lives in `.tau/`, never in the loaded directories.

Open: project directory name — keep `.pi/` for interop, or `.tau/`? `[NEEDS CLARIFICATION]`
