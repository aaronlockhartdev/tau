# Glossary

One term per line: `term`: definition (≤15 words). Specs use these terms and define nothing inline.

`gate`: a named command in `AGENTS.md`, with the state that says who runs it and when.
`lane`: a runner with a fixed scope; its name is a gate row's name.
`delta clause`: one EARS requirement in `changes/<id>/specs/`, applied to `spec/` at archive.
`step vocabulary`: the generated list of allowed Gherkin steps in `STEPS.md`.
`harness`: the test and spec-tooling crates: lanes, step vocabulary, spec linter.
`invariant`: a rule in `spec/index.md` or a domain index, with the check that proves it.
`root`: tau's user file root, `~/.tau`; XDG config home overrides the config path.
`fork`: a child session that copies a range of the parent's blocks and their requests.
`steer`: send input to a running session, stored as a block in its log.
`roster`: the view listing a session's children with their states.
`anchor`: the five-character handle a read serves for one line of one file.
`outline`: a file's anchored declaration rows, bodies read on demand.
`observation`: a condensation of a session's oldest messages, filling a context slot.
`reflection`: a condensation of observations.
`tape`: a recorded model transcript that answers calls in place of a provider.
`layer`: one of two levels a config, context, or skill file sits at, global and project.
`tail`: the newest messages of a session, kept verbatim.
`project`: the directory a session works in, named by its root.
`gate state`: the column saying who runs a row: `run` today, `manual` by an agent, `M0` never.
`M0`: a gate the scaffold created and nothing implements, reported `skipped (M0)`.
`event time`: the moment core produced a block or emitted a stream event, in Unix milliseconds UTC.
`session start output`: the lines tau prints when a session opens, before any block.
`block`: one item of a conversation, stored as one row of the log; the store's unit.
`request`: one provider round-trip: the blocks it produced, its provider and model, usage and terminal.
`injection`: a block the runtime adds to the context, not text from a user.
`slot`: the context position a block names, such as a prompt part.
`user input`: text from a user that reaches the log outside a round-trip, stored as a block.
`usage`: a request's counts: input, cache read, cache write, output.
`terminal`: the reason a request ended, stamped on its row together with its counts.
`state`: a pair of log sequences naming the context a block was built from.
`stream`: the blocks one model call produces, closed at once by its provider.
`event stream`: core's in-memory channel to the frontends: a session's events, as core emits them.
`frontend`: a client of core's event stream: the desktop app `tau-app`, the ACP adapter `tau-acp`.
`message`: a block of user, assistant, tool or injected text in a log.
`markdown block`: a renderable range of transcript markdown — paragraph, list, fence, table; the renderer's block tree.
`fence`: a markdown block of fenced code, open until its closing line.
`resume`: read a session's stored log to continue it in a new process.
`payload`: a block's content: the text or result a single block carries.
`run`: a session's blocks, in the store's sequence order.
`store setup`: a fresh store's journal upgrade and migrations, the steps a second process waits out.
`write connection`: the connection that writes a store; core holds one per process.
`frame budget`: the time one render may take before it drops a frame.
`log`: the `blocks` table — every block, append-only, one row each.
`session`: one running agent's stored conversation: its row, its requests, its blocks.
`checkpoint`: WAL frames copied into the store file; `TRUNCATE` empties the WAL.
`context`: the blocks a request sends to the provider, named by a block's state pointers.
`JSONL`: the export format: one JSON object per line, one line per block.
