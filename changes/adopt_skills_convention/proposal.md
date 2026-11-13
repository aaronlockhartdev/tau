# adopt_skills_convention

## Context

Skills in `.agents/skills/` must load identically in pi, Claude Code, Cursor, and Codex.

## Decision

Follow the Agent Skills convention as documented by `pi` 1.5.2 `docs/skills.md` (frontmatter fields,
discovery locations, name/description-only startup load, malformed-file handling) and the Agent
Skills specification for portability. The references define the format; tau implements it verbatim.

Consequences owned by tau: descriptions are routing code — a bad description means the skill never
loads; skill bodies stay ≤60 lines because they enter context on invocation; project skills are
trust-gated like other project files.

Open: support the experimental `allowed-tools` field at v1? `[NEEDS CLARIFICATION]`
