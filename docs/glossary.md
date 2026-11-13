# Glossary

One term per line: `term`: definition (≤15 words). Specs use these terms and define nothing inline.

`gate`: a named command in `AGENTS.md` that a change must pass.
`lane`: a test runner with a fixed scope: behaviour, surface, eval.
`delta clause`: one EARS requirement in `changes/<id>/specs/`, applied to `spec/` at archive.
`committed marker`: the storage fact that a turn's transaction committed.
`step vocabulary`: the generated list of allowed Gherkin steps in `STEPS.md`.
`harness`: the test and spec-tooling crates: lanes, step vocabulary, spec linter.
`invariant`: a rule in `spec/index.md` or a domain index that names the gate, lint, lane, or review check proving it.
