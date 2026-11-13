# Adopt the skills convention

## Context

Three skill sets already sit on this machine, under `.agents/skills/`, `.pi/skills/`, and `~/.agents/skills/`. tau loads none of them, so the instructions written for agents in this repo reach the built product not at all. Skills are also the form external agents already read, so a repo that ships them is read the same way by tau and by the agent driving the edit. The loading rule, frontmatter fields and discovery order are settled behaviour in pi 1.5.2 and in the Agent Skills specification, so the open question is which parts of them tau implements.

## Options

- Adopt Agent Skills as pi implements it: pi 1.5.2 `docs/skills.md` and the Agent Skills specification (read 2026-11-13); three skill sets are already installed in that layout on this machine. Cost: one tools leaf spec, no new dependency.
- tau-specific skill format: `unverified`, no external source; it loads none of the installed sets and no external agent loads its own.

## Decision

Adopt the Agent Skills convention as pi implements it. For mechanics, read pi 1.5.2 `docs/skills.md` and the Agent Skills specification. Do not restate them here, and do not diverge. That covers discovery, frontmatter, the name and description limits, `${CLAUDE_SKILL_DIR}`, invocation, and the `SKILL.md` template.

Tau's own rules:

- Load order: `<project>/skills`, `<project>/.agents/skills`, `<agent-dir>/skills`, `<agent-dir>/.agents/skills`, then the `extraSkillDirs` paths in user config. Same-name skills stay separate, listed with their directory.
- Skills are read-only, and a skill never grants a permission.
- Ship the experimental `allowed-tools` field at v1, with the same field names.
- Skill bodies are read by file path. Skills use the canonical glossary terms.

## Consequences

Cost: 27 corpus lines, read on demand; skill descriptions enter the prompt every turn at the length limits the convention sets; no new dependency. Skills are input to context assembly in core, so both frontends see the same set. The repo's `.agents/skills/` is read by the built product as well as by external agents, which makes the doc skills an executable spec. Skill text enters the prompt, so loading reports name, path, and description length per session.

Eval: none needed.

Open: none.
