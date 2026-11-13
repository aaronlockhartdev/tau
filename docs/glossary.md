# Glossary

One term per line: `term`: definition (≤15 words). Specs use these terms and define nothing inline.

`gate`: a named command in `AGENTS.md` that a change must pass.
`lane`: a test runner with a fixed scope: behaviour, surface, eval.
`delta clause`: one EARS requirement in `changes/<id>/specs/`, applied to `spec/` at archive.
`committed marker`: the storage fact that a turn's transaction committed.
`step vocabulary`: the generated list of allowed Gherkin steps in `STEPS.md`.
`harness`: the test and spec-tooling crates: lanes, step vocabulary, spec linter.
`invariant`: a rule in `spec/index.md` or a domain index that names the gate, lint, lane, or review check proving it.
`root`: tau's user file root, `~/.tau`; XDG config home overrides the config path.
`fork`: a child session that copies the parent's history and prompt cache.
`steer`: send input to a running child, applied at its next turn boundary.
`roster`: the view listing a session's children with their states.
`config layer`: the global config file, or the project file merged over it key by key.
`anchor`: the five-character handle a read serves for one line of one file.
`outline`: a file's anchored declaration rows, bodies read on demand.
`observation`: a condensation of a session's oldest messages, stored in the session.
`reflection`: a condensation of observations.
`tape`: a recorded model transcript that answers calls in the behaviour lane.
`layer`: the agent-directory level or the project level of a config, context, or skill file.
`tail`: the newest messages of a session, kept verbatim.
`project`: the working directory's git root, or the working directory when it is not a git tree.
`M0`: a gate the scaffold created and nothing implements, reported `skipped (M0)`.
