# Glossary

One term per line: `term`: definition (≤15 words). Specs use these terms and define nothing inline.

`gate`: a named command in `AGENTS.md` that a change must pass.
`lane`: a test runner with a fixed scope: behaviour, surface, eval.
`delta clause`: one EARS requirement in `changes/<id>/specs/`, applied to `spec/` at archive.
`step vocabulary`: the generated list of allowed Gherkin steps in `STEPS.md`.
`harness`: the test and spec-tooling crates: lanes, step vocabulary, spec linter.
`invariant`: a rule in `spec/index.md` or a domain index, with the check that proves it.
`root`: tau's user file root, `~/.tau`; XDG config home overrides the config path.
`fork`: a child session that copies a range of the parent's blocks and their requests.
`steer`: send input to a running child, stored as a block in its log.
`roster`: the view listing a session's children with their states.
`config layer`: the global config file, or the project file merged over it key by key.
`anchor`: the five-character handle a read serves for one line of one file.
`outline`: a file's anchored declaration rows, bodies read on demand.
`observation`: a condensation of a session's oldest messages, stored in the session.
`reflection`: a condensation of observations.
`tape`: a recorded model transcript that answers calls in the behaviour lane.
`layer`: the agent-directory level or the project level of a config, context, or skill file.
`tail`: the newest messages of a session, kept verbatim.
`project`: the working directory's git root, or the working directory when it is not one.
`M0`: a gate the scaffold created and nothing implements, reported `skipped (M0)`.
`event time`: the moment core produced a block or emitted a stream event, in Unix milliseconds UTC.
`session start output`: the lines tau prints when a session opens, before any block.
`block`: one item of a conversation, stored as one row of the log; the store's unit.
`request`: one round-trip to a provider: its model, its usage, its terminal reason.
`injection`: a block the runtime adds to the context, not text from a user.
`slot`: the context position a block names, such as a prompt part.
`user input`: text from a user that reaches the log outside a round-trip, stored as a block.
`usage`: a request's counts: input, cache read, cache write, output.
`terminal`: the reason a request ended, stamped on its row together with its counts.
`state`: a pair of log sequences naming the context a block was built from.
`stream`: the blocks one model call produces, closed at once by its provider.
`event stream`: core's channel to the frontends: the events of a session, one per state change.
`markdown block`: a renderable range of transcript markdown — paragraph, list, fence, table; gpui's block tree.
`fence`: markdown's fenced code span, open until its closing line.
`resume`: read a session's stored log to continue it in a new process.
`payload`: a block's content: the text or result a single block carries.
`run`: a session's blocks, in the log's sequence order.
`store setup`: a fresh store's journal upgrade and migrations, the steps a second process waits out.
`log`: the `blocks` table — every block, append-only, one row each.
`session`: one running agent's stored conversation: its row, its requests, its blocks.
`checkpoint`: WAL frames copied into the store file; `TRUNCATE` empties the WAL.
`context`: the blocks a request sends to the provider, named by a block's state pointers.
`JSONL`: the export format: one JSON object per line, one line per block.
