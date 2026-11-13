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
