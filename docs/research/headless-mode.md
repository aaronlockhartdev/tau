# Research: how top agent harnesses implement headless/autonomous mode (design input for `tau acp` headless)

Researched 2026-10-05. All claims are cited to primary sources fetched during this task (agent source
code, official docs, official protocol specs, official leaderboard metadata). Prompt text is quoted
verbatim from the cited files. Secondary write-ups were not used. Claims that could not be verified
against a primary source are marked **[unverified]**.

**Question.** What is the optimal design for a first-class headless/autonomous mode in `tau acp`,
based on how the top agent harnesses actually implement it? (Issue #72.)

**Short answer.** Every harness that scores well unattended shares four structural choices, none of
which `tau acp` currently has:

1. **A headless-specific prompt frame**, distinct from the interactive product prompt, that states: no
   user is present, keep working until the outcome is achieved, verify by execution, and declare
   completion explicitly (Cline `CLINE_SYSTEM_PROMPT_YOLO_MODE`, mini-swe-agent `instance_template`,
   Dirac `SYSTEM_PROMPT` + yolo line, Terminus-2 `terminus-json-plain.txt`).
2. **A harness/product-owned episode loop** in which "the model stopped calling tools" is *not*
   terminal: the run continues (Terminus-2 `for episode in range(...)`, mini-swe-agent `while True`,
   Cline `agent-runtime.ts` iteration loop, Claude Code/Codex/Cursor/Copilot product loops).
3. **An explicit completion declaration** (a `task_complete` flag, a `submit_and_exit`/`complete` tool
   call, a `COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT` command) rather than "the model went quiet"
   (Terminus-2, Cline, mini-swe-agent, Dirac).
4. **A closed question channel**: questions are auto-answered with an instruction to resolve the
   uncertainty with tools, or the asking tool is removed entirely (Dirac yolo auto-response, Copilot
   `--no-ask-user`, Claude Code `--permission-prompts none` removing `AskUserQuestion`).

For `tau acp` the recommended design (full synthesis in §12): add a headless mode to the ACP server
that (a) swaps the session's system-prompt frame, (b) wraps the existing model-owned turn in an
**ACP-layer episode loop** — when the core settles a turn without a completion declaration, the ACP
server sends a short, task-anchored continuation prompt instead of answering `session/prompt` — and
(c) answers a declared completion after one confirmation round, with a finite episode budget. The
loop lives in `tau-acp`, not `tau-core`, so the GUI product prompt and loop are untouched.

---

## 1. tau's problem (grounding)

The cap-free Terminal-Bench 2.1 baseline (tau-acp + qwen3.8-27b) shows ~40% of trial failures are
headless-adaptation failures, not capability failures:

1. **Confirmation-seeking** — model ends its turn asking a question; nobody answers in headless.
2. **Greeting degeneration** — model mid-trial emits "Hey! I'm Tau. What are we building today?" as
   if the session reset.
3. **Empty/instant abandon** — turn ends with no output and no tool calls.
4. **Spawn-and-abandon** — model delegates to a subagent, ends its own turn; child work discarded.
5. **Missing deliverable** — substantial work done, required file never written, turn ends.

Root cause in the code: the ACP path (`crates/tau-acp/`) reuses the interactive product's prompt and
a model-owned loop.

- The system prompt is assembled once at session creation in
  `crates/tau-core/src/harness/launch.rs:249` —
  `context::assemble("You are Tau, a coding agent.", &layers)` plus the skills catalog — the same
  prompt the GUI uses, where greeting and questions are *correct* behavior.
- `session/prompt` dispatches `Command::MessageSend` to the core
  (`crates/tau-acp/src/server.rs`, `session_prompt`), and the dedicated event pump settles the turn
  the instant the model's tool queue is empty after a `StreamEnd`
  (`crates/tau-acp/src/pump.rs`, `Event::Queue` branch). The ACP spec itself mandates this shape:
  "If there are no pending tool calls, the turn ends and the Agent **MUST** respond to the original
  `session/prompt` request with a `StopReason`" ([ACP v1 prompt-turn
  spec](https://agentclientprotocol.com/protocol/v1/prompt-turn.md), §Prompt Turn). There is no
  protocol mechanism for the agent to keep a single `session/prompt` alive past "model stopped
  calling tools" — any episode loop must therefore live **inside the agent (server) side**, exactly
  as the Cursor and Copilot ACP servers do.

The 73.0% qwen3.8-27b reference score uses Terminus-2, which structurally prevents all five failure
modes: harness-owned episode loop, JSON command-batch output with no conversation channel, task-frame
prompt, explicit `task_complete` flag.

---

## 2. Terminus-2 (local source: `/tmp/harbor-docs/src/harbor/agents/terminus_2/`)

The reference 73.0% harness. A pure headless benchmark agent: there is no interactive mode at all.

### a. Prompt frame — `templates/terminus-json-plain.txt` (verbatim, whole file)

```text
You are an AI assistant tasked with solving command-line tasks in a Linux environment. You will be
given a task description and the output from previously executed commands. Your goal is to solve the
task by providing batches of shell commands.

Format your response as JSON with the following structure:

{
  "analysis": "Analyze the current state based on the terminal output provided. What do you see?
  What has been accomplished? What still needs to be done?",
  "plan": "Describe your plan for the next steps. What commands will you run and why? Be specific
  about what you expect each command to accomplish.",
  "commands": [
    { "keystrokes": "ls -la\n", "duration": 0.1 },
    { "keystrokes": "cd project\n", "duration": 0.1 }
  ],
  "task_complete": true
}

Required fields:
- "analysis": Your analysis of the current situation
- "plan": Your plan for the next steps
- "commands": Array of command objects to execute

Optional fields:
- "task_complete": Boolean indicating if the task is complete (defaults to false if not present)
...
Task Description:
{instruction}

Current terminal state:
{terminal_state}
```

Notes: the frame is a *task frame* ("tasked with solving command-line tasks… Your goal is to solve
the task"), not a relationship frame. The only free-text the model may emit is the `analysis`/`plan`
JSON fields — there is **no conversation channel at all**, which structurally eliminates
confirmation-seeking and greeting degeneration. Extra prose around the JSON "will generate warnings
but be tolerated" (i.e. parsed away).

### b. Loop ownership — harness-driven episode loop

`terminus_2.py:1378`: `for episode in range(self._max_episodes):` — one episode = one LLM call →
parse JSON → send keystrokes to the tmux terminal → capture incremental output → next episode. When
the model ends a turn without `task_complete` (the default), nothing is "over": the next episode
simply begins with the new terminal state as the prompt (`terminus_2.py:1655-1663`:
`if is_task_complete: … else: prompt = observation`). The implicit continuation nudge *is* the
terminal state itself; there is no special "keep going" text.

### c. Completion semantics — explicit flag + two-step confirmation

`task_complete: true` is **not** immediately final. `terminus_2.py:626-636`
(`_get_completion_confirmation_message`) — on the *first* `task_complete: true`, the harness sends
back (verbatim, JSON flavor):

```text
Current terminal state:
{terminal_output}

Are you sure you want to mark the task as complete? This will trigger your solution to be graded
and you won't be able to make any further corrections. If so, include "task_complete": true in
your JSON response again.
```

The run only returns when the flag is true a **second time** (`terminus_2.py:1655-1661`: "Task is
confirmed complete (this is the second time task_complete was True), return" / "First completion
attempt - ask for confirmation and continue"). Cost: one extra model call; benefit: it catches
premature completion, which is the missing-deliverable failure class.

### d. Question channel

None exists. The model cannot ask; its only outputs are `analysis`, `plan`, `commands`. (The
summarization subagents *do* ask questions of each other — `terminus_2.py:969-978` — but that is
internal context management, not a user channel.)

### e. Permission/confirmation bypass

N/A — the agent owns the whole terminal in a sandboxed container; there is no permission system to
bypass.

### f. Continuation parameters

- `max_turns` (a.k.a. `max_episodes`): default **1,000,000** when unset
  (`terminus_2.py:440`), with a warning: "max_turns (f.k.a. max_episodes) artificially limited to
  {n}. Consider removing this limit for better task completion."
- Parse-error nudge (`terminus_2.py:1480-1484`, verbatim):
  `Previous response had parsing errors:\n{feedback}\n\nPlease fix these issues and provide a proper
  JSON response.`
- Command-timeout nudge (`templates/timeout.txt`, verbatim): "The previous command timed out after
  {timeout_sec} seconds … It is possible that the command is not yet finished executing. If that is
  the case, then do nothing. … Here is the current state of the terminal:"
- Output-length-exceeded nudge (`terminus_2.py:1257-1261`, verbatim): "ERROR!! NONE of the actions
  you just requested were performed because you exceeded {limit_str}. Your outputs must be less than
  {limit_str}. Re-issue this request, breaking it into chunks each of which is less than
  {limit_str}."
- Context management: proactive summarization at 8000 free tokens, then a three-subagent
  summary→questions→answers handoff; the handoff prompt re-anchors the task
  (`terminus_2.py:1073-1078`, verbatim): "Here are the answers the other agent provided. … Continue
  working on this task from where the previous agent left off. You can no longer ask questions.
  Please follow the spec to interact with the terminal."

---

## 3. Claude Code (`anthropics/claude-code`, v2.1.289)

GUI-first product; headless is a mode of the product (`claude -p`), not a separate harness.
Primary sources: official docs (code.claude.com) and the npm binary
(`@anthropic-ai/claude-code` 2.1.289, build 2026-10-03 — system-prompt strings extracted from the
binary; the full assembled prompt body is not extractable as one contiguous string from the
Bun-compiled binary, so only the verbatim fragments below are quoted).

### a. Prompt frame

The prompt's opening line, verbatim from the binary (three variants selected by output style):

```text
You are an interactive CLI tool that helps users with software engineering tasks. You should work
proactively and autonomously, executing immediately and minimizing interruptions.
```

Attribution header: `You are Claude Code, Anthropic's official CLI for Claude.` Headless sessions
get the same prompt — there is **no separate headless system prompt**. The headless surface is
configured by flags: `--append-system-prompt` (add) / `--system-prompt` (replace, per the
[CLI reference](https://code.claude.com/docs/en/cli-reference)) and `--bare` ("reduces startup
time by skipping auto-discovery of hooks, skills, custom commands, subagents, installed plugins,
MCP servers, auto memory, and CLAUDE.md"; "the recommended mode for scripted and SDK calls" —
[headless docs](https://code.claude.com/docs/en/headless)).

### b. Loop ownership — product-owned; **no auto-continue**

The agent loop is the product's (the same loop the Agent SDK exposes). In `-p` mode the run ends
when the model returns its final message: there is **no nudge and no re-prompt** — an early stop
is the end of the run. This is the deliberate opposite of Terminus-2: Claude Code bets on the
prompt ("work proactively and autonomously") plus strong models rather than a harness loop.

### c. Completion semantics

Final assistant message with no further tool calls; the CLI exits 0 and the `result` message (or
`--output-format json`/`stream-json`) carries it. `--max-turns` "Limit the number of agentic turns
(print mode only). Exits with an error when the limit is reached. No limit by default."
([CLI reference](https://code.claude.com/docs/en/cli-reference)).

### d. Question channel — removed, not answered

The `AskUserQuestion` tool is the clarifying-question channel. In unattended runs:

- `--permission-prompts none`: "your run doesn't consult the host or wait on it. Anything that
  would prompt is denied … and the run continues. … Claude Code removes the tools that need an
  answer from a person, such as `AskUserQuestion`, so Claude can't call them."
- SDK `dontAsk` mode: "Any call that would otherwise prompt is denied. … `canUseTool` is never
  called" ([Agent SDK permissions](https://code.claude.com/docs/en/agent-sdk/permissions)).

So the question channel is **deleted** in headless, and the model is told not to retry denied
requests.

### e. Permission bypass

`bypassPermissions` mode / `--dangerously-skip-permissions` ("equivalent"): "disables permission
prompts and safety checks so tool calls execute immediately, including writes to protected paths";
"Only use this mode in isolated environments like containers, VMs, or dev containers without
internet access"; refused under root/sudo ("--dangerously-skip-permissions cannot be used with
root/sudo privileges for security reasons"); "no dialog is shown" in non-interactive mode.
([permission-modes docs](https://code.claude.com/docs/en/permission-modes)). `--permission-mode
dontAsk` is the locked-down-CI alternative (deny instead of prompt).

### f. Continuation parameters

No auto-continuations, no nudge text. Relevant knobs: `--max-turns` (above); background work at
exit — "If Claude starts a background subagent or workflow, `claude -p` instead stays open until
that work completes, because its result is part of the final output. … the wait ends after
**10 minutes** of continuous idle waiting" (`CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS`) — a direct
answer to the spawn-and-abandon failure class: the product refuses to let the run exit while
subagent work is still in flight.

---

## 4. Cursor (`cursor-agent` / `agent` CLI)

Closed source; primary source is the official docs (cursor.com/docs/cli), which are unusually
detailed about the ACP server. The actual system prompt text is **not published**
**[unverified: prompt wording]**.

### a. Prompt frame

Not published. The docs describe headless as the same agent with print + permission flags
([headless docs](https://cursor.com/docs/cli/headless.md), [using docs](https://cursor.com/docs/cli/using.md):
"Use `-p` or `--print` to run Agent in non-interactive mode").

### b. Loop ownership — product-owned

One `session/prompt` runs to completion inside the CLI (the ACP server keeps the request pending
while the agent works; see d/e below). No auto-continue semantics are documented.

### c. Completion semantics

The ACP `session/prompt` response `stopReason` (their ACP server is the reference implementation;
[ACP docs](https://cursor.com/docs/cli/acp.md) show `[stopReason=${result.stopReason}]` in the
sample client).

### d. Question channel — blocking extension, client must answer

ACP sessions expose `cursor/ask_question` as a **blocking** extension method: "The agent waits for
a response before continuing. Your client must reply with a JSON-RPC response." The response
outcome includes `{ "outcome": "skipped"; reason?: string }` and `"cancelled"` — i.e. a headless
client's answer to a question is *skip/cancel*, not an auto-answer. ("If your client does not
answer permission requests, tool execution can block.")

### e. Permission bypass

`--yolo` is "Alias for `--force`"; `--force`: "Force allow commands unless explicitly denied"
([parameters reference](https://cursor.com/docs/cli/reference/permissions.md) +
[parameters](https://cursor.com/docs/cli/reference/parameters.md)). "Cursor has full write access
in non-interactive mode." ([using docs](https://cursor.com/docs/cli/using.md)). Granular
`permissions.allow/deny` tokens (`Shell(git)`, `Write(src/**)`, …) remain available for locked-down
runs; "Deny rules take precedence over allow rules."

### f. Continuation parameters

Not documented (no max-continuations, no nudge text published) **[unverified]**.

---

## 5. GitHub Copilot CLI (`github/copilot-cli`)

The public repo is an install wrapper + changelog; the CLI ships as a binary. Primary sources:
official docs (docs.github.com) and the repo changelog. Prompt wording is **not published**
**[unverified: prompt wording]**.

### a. Prompt frame

Not published. Headless is documented purely as flags.

### b. Loop ownership — product-owned

`-p PROMPT, --prompt=PROMPT`: "Execute a prompt programmatically **(exits after completion)**. The
exit summary includes a `copilot --resume=SESSION-ID` hint for continuing the session."
([command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)).
Continuation across "turns" is left to the caller re-invoking with `--resume` — there is no
documented auto-continue.

### c. Completion semantics

The process exits when the run completes (`-p … exits after completion`); `--output-format json`
emits JSONL with a final record; `--share=PATH` exports the session transcript for audit.

### d. Question channel — tool disabled

`--no-ask-user`: "**Prevent the agent from pausing to seek additional user input.**"
(programmatic reference) / "Disable the ask_user tool (**the agent works autonomously without
asking questions**)." (command reference). The official programmatic-usage doc lists it among the
recommended tips for scripts/CI: "Use `--no-ask-user` to prevent the agent from attempting to ask
clarifying questions."

### e. Permission bypass

`--yolo`: "Enable all permissions (equivalent to `--allow-all`)." where `--allow-all` =
"Equivalent to `--allow-all-tools --allow-all-paths --allow-all-urls`." Enterprise kill-switch:
"when `permissions.disableBypassPermissionsMode` is set to `"disable"`, all of the command line
options that allow all permissions … are suppressed at startup." Permission modes:
`default | assisted | allow-all`. Local sandboxing is documented separately
([understanding local
sandboxing](https://docs.github.com/en/copilot/concepts/agents/copilot-cli/understanding-local-sandboxing)).

### f. Continuation parameters

Not documented **[unverified]**. (The changelog confirms `/yolo` and `--yolo` "now behave
identically" and yolo state persists across `/restart` — interactive-surface detail only.)

---

## 6. OpenAI Codex (`openai/codex`)

Open source (Rust). Non-interactive mode is `codex exec`; the agent loop is product-owned in
`codex-rs/core`.

### a. Prompt frame

Model instructions are per-model template files, e.g.
[`codex-rs/core/gpt-5.2-codex_prompt.md`](https://github.com/openai/codex/blob/main/codex-rs/core/gpt-5.2-codex_prompt.md)
(verbatim opening): "You are Codex, based on GPT-5. You are running as a coding agent in the Codex
CLI on a user's computer." The prompt is written for an interactive teammate ("Ask only when
needed"; "If this happens, STOP IMMEDIATELY and ask the user how they would like to proceed") —
there is **no separate headless prompt**; headless-ness is expressed via sandbox/approval
settings, not prompt text.

### b. Loop ownership — product-owned

`codex-rs/core/src/session/turn.rs`, `run_turn` (line 163; loop at line 426): "Takes initial turn
input and runs a loop where, at each sampling request, …" — the loop keeps sampling while the
model returns tool calls and **breaks when the model produces a final non-tool response**. No
auto-continue, no nudge: a final message ends the turn and `codex exec` exits.

### c. Completion semantics

Final agent message = done. With `--json`, stdout is JSONL with `turn.started` / `turn.completed`
/ `turn.failed` events; `--output-schema` can force the final message to conform to a JSON Schema;
`-o <path>` writes the final message to a file. ([non-interactive
docs](https://developers.openai.com/codex/noninteractive.md))

### d. Question channel

None in `exec` mode — the model simply answers; mid-turn questions have no channel (the
interactive TUI is the only surface where user input exists).

### e. Permission bypass

Sandbox + approval policy are pre-set flags; there is no prompt in `exec`. `codex exec` defaults
to a **read-only** sandbox; `--sandbox workspace-write` / `--sandbox danger-full-access` widen it
("`danger-full-access` only in a controlled environment (for example, an isolated CI runner or
container)"); `--full-auto` is kept as a deprecated compatibility flag. The approval policy is
reflected in the prompt so the model stops requesting escapes —
`codex-rs/prompts/templates/permissions/approval_policy/never.md` (verbatim, whole file):
"Approval policy is currently never. Do not provide the `sandbox_permissions` for any reason,
commands will be rejected." `codex exec` also "requires commands to run inside a Git repository to
prevent destructive changes" (overridable with `--skip-git-repo-check`).

### f. Continuation parameters

No auto-continuations. Context overflow is handled by compaction (summary) inside the loop, not by
re-prompting; `--ignore-user-config` / `--ignore-rules` make runs deterministic in CI.

---

## 7. Cline (`cline/cline`)

Open source. The VS Code extension and the new CLI share one agent core (`sdk/packages/core`,
`sdk/packages/agents`). Cline is the strongest example of a GUI-first product that built a
**dedicated headless prompt** plus a **nudge-based continuation loop**.

### a. Prompt frame — dedicated yolo prompt (verbatim, `sdk/packages/shared/src/prompt/system/yolo.ts`)

```text
You are Cline, a careful and helpful coding agent that works in the background.
You are tasked to solve an issue reported by the user who you cannot communicate with directly.
Your goal is to utilize the tools at your disposal to investigate and answer the question
according to user's instructions with the aim to verify that the issue is resolved.

RULES:
...
- Be proactive and avoid overthinking. Once the next action is clear, execute it. Do not seek
  perfect certainty, repeatedly compare alternatives, or revisit settled decisions without new
  evidence. Use focused tool calls to resolve uncertainty.
- If repeated fixes fail without new evidence, stop making similar edits. Test your assumptions
  with a focused check or minimal reproduction, then adjust your approach based on the result.
...
IMPORTANT:
- Verify by execution, never by assumption. Before considering any task done, gather concrete
  evidence from your own tool output that every requirement is satisfied:
    - If a test suite, tests, or assertions are provided or referenced, run them and confirm they
      pass. ...
    - If no tests are provided, construct your own verification: actually run the program, script,
      or command you produced; confirm every required output file exists at the exact path
      requested; and confirm its contents match the expected format, data types, and values
      described in the task. Read the output back to confirm.
- Treat "this should work", "assume it works", or "probably correct" as a signal that you have NOT
  verified yet — go run the check instead of finishing.
- Do not consider the task complete until you have observed evidence that all stated requirements
  are met.
- Always includes tool calls in your response until the task is completed. You should only end the
  task when all the requirements are met by calling the 'submit_and_exit' tool.
- When you call 'submit_and_exit', set 'verified' to true only if your tool output shows the
  requirements are met; otherwise set it to false.
- Response without the submit_and_exit tool call will considered not completed and the task will
  continue.
```

Contrast with the interactive act-mode prompt
(`sdk/packages/shared/src/prompt/system/act.ts`), which *invites* questions ("If you need more
information, … ask for clarification instead of making assumptions") and treats a tool-call-free
response as completion ("Response without tool calls will considered as completed with final
answer"). The yolo variant is a genuinely different contract: background, no user, verify,
explicit terminal tool.

### b. Loop ownership — product-owned with nudge-based auto-continue

`sdk/packages/agents/src/agent-runtime.ts`, `execute()`: `while (maxIterations === undefined ||
iteration < maxIterations)`. When a turn yields **no tool calls**:

- if `completionPolicy.requireCompletionTool` (yolo mode): the harness injects a `[SYSTEM]` user
  message and **continues** the loop (verbatim, `agent-runtime.ts:753-760`):
  `[SYSTEM] This run is not complete until you call one of these terminal completion tools:
  submit_and_exit. Continue working if requirements are not met. If the task is complete, call the
  appropriate terminal completion tool now.`
- otherwise: the run completes with that final message.

Empty responses are *failures*, not silent stops: `throw new Error("Model returned empty
response")` (content-filter flavor: `CONTENT_FILTER_EMPTY_TURN_MESSAGE`). A `max-tokens` turn with
no tool call triggers a recovery/retry instead.

### c. Completion semantics

The `submit_and_exit` tool call (a tool with `lifecycle.completesRun === true`) ends the run —
`finishRun("completed", …)` in `agent-runtime.ts`. In yolo mode a stop *without* that call is not
completion (see b).

### d. Question channel

In yolo mode there is no user to answer; the CLI/zen surfaces simply never surface question cards
(see e). The interactive surfaces show a question card (`requireFeedback: true`) and can be
declined ("The user declined to answer the follow-up question.").

### e. Permission/confirmation bypass + subagent policy

`-y, --yolo` (CLI README, verbatim): "Skip tool approval prompts, enable `submit_and_exit`, and
**disable spawn/team tools by default**." The zen (headless CI) profile: "Because there is no
human in the loop once the CLI exits, zen sessions run with full tool auto-approval (same
semantics as `--yolo`). `spawn`/`team` tools are disabled by default for safety, consistent with
yolo-mode defaults." — i.e. Cline answers spawn-and-abandon by **not offering the spawn tools in
headless at all**.

### f. Continuation parameters

- `maxIterations` (unbounded by default; exceeding it throws `Agent runtime exceeded
  maxIterations (N)`).
- The two `[SYSTEM]` nudges above, plus a **team-obligation guard** injected whenever the model
  tries to stop while delegated work is in flight (verbatim,
  `sdk/packages/core/src/runtime/orchestration/runtime-builder.ts:847-860`):
  `[SYSTEM] You still have team obligations. Unfinished tasks: … Active runs: … Use
  team_run_task to delegate work, or team_task with action=complete to mark tasks done, or
  team_await_runs to wait for active runs. Do NOT stop until all tasks are completed.`
- `--retries N` — "Override consecutive internal mistake (retry) limit (default: 3)" (CLI README).

---

## 8. mini-swe-agent (`SWE-agent/mini-swe-agent`)

The ~100-line reference minimal agent. Note: the canonical repo is
[`SWE-agent/mini-swe-agent`](https://github.com/SWE-agent/mini-swe-agent) (the issue's
`emmanuel-fermine/mini-swe-agent` redirects to it; the package moved to `src/minisweagent/`).
Headless is the *only* mode; the Harbor adapter
(`/tmp/harbor-docs/src/harbor/agents/installed/mini_swe_agent.py`) runs
`mini-swe-agent --yolo --model=… --task=… --output=… --exit-immediately`.

### a. Prompt frame (verbatim, `src/minisweagent/config/mini.yaml`)

System prompt (entire): `You are a helpful assistant that can interact with a computer.`

The per-task **instance template** (second message) carries the autonomy frame (key clauses):

```text
Please solve this issue: {{task}}

You can execute bash commands and edit files to implement the necessary changes.

## Recommended Workflow
...
6. Submit your changes and finish your work by issuing the following command:
   `echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT`.
   Do not combine it with any other command.
   <important>After this command, you cannot continue working on this task.</important>

## Command Execution Rules
...
**CRITICAL REQUIREMENTS:**
- Your response SHOULD include reasoning text explaining what you're doing
- Your response MUST include AT LEAST ONE bash tool call
```

### b. Loop ownership — harness-driven infinite loop

`src/minisweagent/agents/default.py:96`: `while True: self.step()` — the run ends **only** on an
`exit` message, produced by: the submit command being observed in a bash output, `LimitsExceeded`
(step/cost limit), `TimeExceeded`, or `RepeatedFormatError`. A model stop is never terminal by
itself.

### c. Completion semantics

The `echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT` sentinel command (detected in command output) is
the only clean completion path; the trajectory records `exit_status` + `submission`.

### d. Question channel

None — a single bash tool and a task statement; the format-error nudge (verbatim,
`mini.yaml` `format_error_template`) re-anchors the contract instead:

```text
Tool call error:
<error>{{error}}</error>
Here is general guidance on how to submit correct toolcalls:
Every response needs to use the 'bash' tool at least once to execute commands.
...
If you want to end the task, please issue the following command:
`echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT` without any other command.
```

A truncation-specific variant: "Your previous response reached the output token limit
(finish_reason=…) before you produced a tool call, so it was cut off. Respond more concisely and
finish with exactly one bash tool call. If you need to think more, do so briefly."

### e. Permission/confirmation bypass

`-y, --yolo`: "Run without confirmation" → `agent.mode = "yolo"` (default mode is `confirm`, which
prompts before each command); `--exit-immediately`: "Exit immediately when the agent wants to
finish instead of prompting." (`src/minisweagent/run/mini.py:61-65`)

### f. Continuation parameters

`step_limit` (0 = unlimited), `cost_limit` (default $3.00), `wall_time_limit_seconds` (0 = none),
`max_consecutive_format_errors` (default 3) — all in `AgentConfig`
(`src/minisweagent/agents/default.py:21-38`).

---

## 9. Dirac — identification + findings

**Identification.** "Dirac" on the Terminal-Bench 2 leaderboard is **not** a KRAFTON AI project.
The official leaderboard dataset
(`harborframework/terminal-bench-2-leaderboard` on Hugging Face) contains the submission
`submissions/terminal-bench/2.0/Dirac__Gemini-3-Flash-Preview/` whose `metadata.yaml` (verbatim)
reads:

```yaml
agent_url: "https://github.com/dirac-run/dirac"
agent_display_name: "Dirac"
agent_org_display_name: "Dirac Delta"
models:
  - model_name: "gemini-3-flash-preview"
    model_provider: "google"
    ...
```

So the pinned repo is **[github.com/dirac-run/dirac](https://github.com/dirac-run/dirac)**
("Dirac Delta", dirac.run): a TypeScript, Cline-derived coding agent — "Coding Agent singularly
focused [on] efficiency and context curation … Uses Hash Anchored edits, massively parallel
operations, AST manipulation." The KRAFTON association in the issue traces to
`stanford-iris-lab/meta-harness-tbench2-artifact` (whose README cites
[Terminus-KIRA](https://github.com/krafton-ai/KIRA) "by KRAFTON AI"); KRAFTON AI's own agent on
the same leaderboard is `Terminus-KIRA__*` — a *different* entry. **There is no project named
Dirac that is KRAFTON AI's agent harness; Dirac is Dirac Delta's.** (KRAFTON's harness is KIRA,
out of scope here.)

### a. Prompt frame (verbatim, `src/core/prompts/system-prompt/template.ts`)

```text
You are Dirac, an AI agent for software-engineering tasks.

OPERATING PRINCIPLES

1. Understand the outcome the user is trying to achieve and use it as the primary measure of
   success. Honor explicit requirements and applicable instructions; continue until that outcome
   is achieved or a blocker prevents it.
2. Validate material assumptions, verify outcomes, account for relevant edge cases, and never
   claim unsupported results.
3. Take the shortest reliable path. Avoid unnecessary work and never sacrifice correctness for
   speed.
4. Trust successful tool results and current file state. Do not repeat reads or checks without a
   task-relevant reason; tools report stale state.

TOOL USE

- Minimize round trips by grouping independent operations into one tool call ...
- Use `respond` for user-facing communication: `progress` for meaningful updates during longer
  work, `question` when you need information from the user, `plan` for Plan-mode answers or
  proposals, and `complete` for final Act-mode results. Do not send plain assistant text; every
  response must include a tool call.
```

Yolo/headless adds one line (template.ts, `yoloModeToggled` branch, verbatim):
`- Autonomous mode: keep resource use reasonable.`

The "every response must include a tool call" constraint plus the `respond` tool family
(`progress`/`question`/`plan`/`complete`) is the structural answer to empty-stop and
missing-deliverable: a plain-text stop is contractually invalid, and completion is a typed
operation.

### b. Loop ownership — product-owned

`src/core/task/TaskRequestLoop.ts`, `recursivelyMakeDiracRequests` — the Cline-derived recursive
request loop (steering, compaction, checkpoints); `src/core/task/index.ts:1134` drives it with
`while (!this.taskState.abort)`. The loop ends on abort, mistake-limit, committed completion, or
waiting for the user.

### c. Completion semantics

`respond: complete` in Act mode (`CompletionResponseOperation.ts`): commits the completion
(`commitAttemptCompletion`); if `doubleCheckCompletionEnabled`, the completion first goes through a
**verification pass** — a verification subagent, or an in-loop prompt (verbatim,
`CompletionResponseOperation.ts:35-41,101`):

```text
Verification Required: User wants you to fully verify your solution before submitting.

<initial_task>
...
1. All requested changes have been made (verify using a test script/`execute_command` when possible)
2. No steps were skipped or partially completed
3. Edge cases and error handling are addressed
4. The solution matches what was asked for, not just what was convenient
5. Output files contain exactly what was specified - no extra columns, fields, debug output, or
   commentary
6. If the task specifies numerical thresholds or accuracy targets, verify your result meets the
   criteria. If close but not passing, iterate rather than declaring completion
```

i.e. a two-step completion, in the same spirit as Terminus-2's confirmation round.

### d. Question channel — auto-answered in yolo (verbatim)

`src/core/task/tools/modules/respond/QuestionResponseOperation.ts:21-29`: when
`yoloModeToggled`, a `respond: question` does **not** block for a user; it is auto-responded with:

```text
[YOLO MODE: User input is not available in non-interactive mode. You must use available tools
(read_file, list_files, search_files, etc.) to gather the information you need instead of asking
the user. Proceed with using tools to find the answer to your question: "{question}"]
```

This is the cleanest "question channel" design in the set: the question is acknowledged, the model
is told there is no user, and it is redirected to self-serve the answer with tools — no tool
removal, no hang.

### e. Permission/confirmation bypass

Yolo mode toggles auto-approval (`yoloModeToggled` flows through config/auto-approval settings);
the CLI/extension surface is where the toggle lives. **[unverified: exact CLI flag spelling for
headless use — the TBench submission's run config was not inspected]**

### f. Continuation parameters

Not exposed as continuation knobs (no documented max-continuations/nudge budget) **[unverified]**.

---

## 10. Secondary (brief)

**OpenHands** — `openhands --headless -t "task"`: "Headless mode always runs in `always-approve`
mode. The agent will execute all actions without any confirmation. This cannot be changed —
`--llm-approve` is not available in headless mode." ([OpenHands CLI headless
docs](https://docs.openhands.dev/openhands/usage/cli/headless.md)); `--json` streams JSONL action/
observation events.

**Aider** — interactive pair-programming chat; no documented headless/autonomous mode was found in
its primary docs during this task **[unverified: not checked exhaustively]**.

**Goose** — has a non-interactive `goose run` path, but it was not investigated in depth
**[unverified]**.

---

## 11. Comparison matrix

| Harness | a. Headless prompt frame | b. Loop ownership / early-stop behavior | c. Completion semantics | d. Question channel in headless | e. Permission bypass | f. Continuation parameters |
|---|---|---|---|---|---|---|
| **Terminus-2** | Dedicated task frame; JSON-only output, no conversation channel | **Harness** `for episode in range(max_episodes)`; early stop ⇒ next episode with terminal state (implicit nudge) | Explicit `task_complete` flag, **two-step** (confirmation round) | None exists | N/A (owns the terminal) | max_episodes default 1,000,000; parse/timeout/length nudge texts; 3-subagent summarization handoff |
| **Claude Code** | Product prompt + `--append-system-prompt`; no dedicated headless prompt | **Product**; final message ⇒ run ends; **no auto-continue** | Final message; `--max-turns` (error on hit) | **Removed**: `AskUserQuestion` dropped under `--permission-prompts none` / `dontAsk` | `bypassPermissions` / `--dangerously-skip-permissions` (container-only, non-root) | None; `--max-turns`; 10-min wait for background subagents at exit |
| **Cursor** | Not published **[unverified]** | **Product**; one ACP prompt runs to completion | ACP `stopReason` on `session/prompt` | **Blocking** `cursor/ask_question`; headless client answers `skipped`/`cancelled` (or hangs) | `--yolo` = `--force` ("force allow commands unless explicitly denied"); full write access in non-interactive mode | Not published **[unverified]** |
| **Copilot CLI** | Not published **[unverified]** | **Product**; `-p … exits after completion`; caller re-invokes with `--resume` | Process exit + final JSONL record | **Tool disabled**: `--no-ask-user` ("the agent works autonomously without asking questions") | `--yolo` = `--allow-all` (tools+paths+urls); `disableBypassPermissionsMode` kill-switch | Not documented **[unverified]** |
| **Codex** | Per-model product prompt (interactive-flavored); no headless variant | **Product** (`run_turn` loop); final non-tool message ⇒ break; **no auto-continue** | Final message; `turn.completed` JSONL; `--output-schema` | None in `exec` mode | `--sandbox workspace-write\|danger-full-access`; `--full-auto` (deprecated); approval policy injected into prompt ("Do not provide the `sandbox_permissions` for any reason") | None; compaction on context overflow |
| **Cline** | **Dedicated yolo prompt** (background, no user, verify-by-execution, `submit_and_exit`) | **Product** with **nudge-based auto-continue**: no tool calls + required completion tool ⇒ `[SYSTEM]` reminder + continue; empty response ⇒ error | `submit_and_exit` tool call (`completesRun`) | No channel in yolo (cards are interactive-only) | `--yolo`: skip approvals + enable `submit_and_exit` + **disable spawn/team tools** | `maxIterations` (unbounded default); 2 nudge texts (completion reminder, team-obligation guard); `--retries` (default 3) |
| **mini-swe-agent** | Minimal system + task **instance template** (workflow, MUST include a bash call, submit sentinel) | **Harness** `while True`; run ends only on exit message | `echo COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT` sentinel in output | None; format-error nudge re-anchors the contract | `--yolo` = no per-command confirmation; `--exit-immediately` | `step_limit`/`cost_limit`/`wall_time_limit`/`max_consecutive_format_errors` (3) |
| **Dirac** | Product prompt + yolo line; "every response must include a tool call"; `respond` tool family | **Product** (Cline-derived recursive loop) | `respond: complete` + optional **double-check verification** pass (two-step) | **Auto-answered in yolo**: "[YOLO MODE: User input is not available … use tools … to find the answer to your question]" | Yolo toggle = full auto-approval **[flag spelling unverified]** | Not exposed **[unverified]** |

Structural clusters:

- **Benchmark harnesses (Terminus-2, mini-swe-agent)**: harness-owned unbounded episode loop,
  structured output with no conversation channel, explicit completion flag, two-step confirmation
  (Terminus-2) or sentinel command (mini-swe).
- **GUI-first products (Claude Code, Cursor, Codex, Copilot, Cline, Dirac)**: product-owned loop;
  headless = flag-gated permission bypass + question channel closed (removed, disabled, blocking,
  or auto-answered); only Cline and Dirac go further with a dedicated headless prompt and
  nudge/auto-continue semantics.

---

## 12. Synthesis: recommended design for `tau acp` headless mode

### 12.1 The five failure modes, mapped to mechanisms

| Failure mode | Mechanism that prevents it | Precedent |
|---|---|---|
| 1. Confirmation-seeking | Question channel auto-answered with a "no user; resolve with tools" response; never a hang, never a removal that the model can't predict | Dirac yolo auto-response; Copilot `--no-ask-user`; Claude Code tool removal |
| 2. Greeting degeneration | Headless prompt frame that asserts the task and forbids re-introductions; continuation prompts that **re-state the task frame**, not bare state | Terminus-2 task frame + terminal state; Cline yolo frame; (anti-example: bare-state re-prompt reads as a session reset) |
| 3. Empty/instant abandon | "Model stopped calling tools" is non-terminal: the episode loop continues; empty response is a nudge event, not a settle | Terminus-2 loop; Cline (`[SYSTEM]` reminder + continue; empty ⇒ error after budget) |
| 4. Spawn-and-abandon | Completion is blocked (or the run is auto-continued) while subagent children are in flight; optionally spawn tools disabled in headless | Claude Code 10-min background-subagent wait; Cline team-obligation guard / spawn tools disabled in yolo |
| 5. Missing deliverable | Verification clause in the prompt + two-step completion confirmation | Cline verify-by-execution; Terminus-2 confirmation round; Dirac double-check |

### 12.2 Where the continuation loop lives: the ACP layer

**Recommendation: implement the episode loop in `tau-acp` (the ACP server), not `tau-core`.**

Rationale:

1. **Protocol shape.** ACP mandates that a turn ends when there are no pending tool calls and the
   agent answers the *original* `session/prompt` ([ACP prompt-turn
   spec](https://agentclientprotocol.com/protocol/v1/prompt-turn.md)). A headless run is "one
   client prompt → one final answer", with the agent internally doing N episodes — exactly what
   the Cursor ACP server and Copilot ACP server do (one pending `session/prompt` while the agent
   works). An ACP-layer loop keeps the client contract intact: Harbor sends one prompt and gets
   one `stopReason: "end_turn"` when the task is genuinely complete.
2. **Blast radius.** `tau-core`'s turn loop is shared with the GUI, where model-owned turns are
   correct. A core-level "auto-continue" policy would leak into the product. A `tau-acp`-local
   loop changes only the headless surface (this is also the split the ticket's root-cause
   analysis implies: "the ACP path reuses the interactive GUI product's … model-owned loop").
3. **Settle signal already exists.** `pump.rs` already computes the settle outcome
   (`Outcome::EndTurn` on empty `Event::Queue` after `StreamEnd`) per session. The episode loop
   hooks precisely there: in headless mode, `Outcome::EndTurn` without a completion declaration
   becomes "send continuation prompt N+1" instead of "answer `session/prompt`".

Mechanics (concrete):

- `session/new` (or a `session`-level flag, e.g. `configOptions`/`_meta` extension — tau-acp
  already has `_meta` provider plumbing in `config.rs`) marks the session **headless**.
- On headless session creation, `tau-acp` requests the core build the session with the **headless
  prompt frame** (§12.3) instead of the interactive one. The core's prompt assembly
  (`launch.rs:249`) already takes the base prompt as a parameter — a one-argument change, no core
  loop change.
- The ACP server keeps a per-session `episode` counter. When the pump settles a turn:
  - completion declared (see §12.4) and not yet confirmed → send the confirmation prompt,
    episode += 1;
  - completion confirmed (second declaration) → answer `session/prompt` with
    `stopReason: "end_turn"`;
  - no completion, episode < `max_episodes` → detect the stop reason (empty stop / question
    detected / plain-text stop) and send the matching continuation prompt (§12.5), episode += 1;
  - episode ≥ `max_episodes` → answer `session/prompt` with `end_turn` (the run's last assistant
    text is the result; the harness/eval side can also read the transcript).
- Subagent gate: while the core reports in-flight subagent children for the session, a
  completion declaration is held (queued) and the loop continues — Cline's team-obligation guard
  in protocol shape.

Alternative considered — core-level autonomy policy (Cline-style nudge inside the turn loop):
rejected for v0 because it changes shared core behavior and because the ACP layer already owns the
settle decision; it remains the right home *if* the GUI ever wants the same behavior.

### 12.3 Draft headless prompt frame

Base prompt replacement for headless sessions (keeps the existing context-file layers and skills
catalog; only the base + frame change). Modeled on Cline's yolo prompt (background/no-user/verify),
Terminus-2's task frame, and Dirac's outcome/verification principles:

```text
You are Tau, an autonomous coding agent running headless. There is no user at the other end of
this session: you will not receive answers to questions, and no human will review your work
before it is graded.

The task is your only contract. Keep working until the task's outcome is achieved, or until a
concrete blocker makes it impossible (then state the blocker and stop).

RULES:
- Never open a turn with a greeting, self-introduction, or small talk. Never end a turn by
  asking the user a question or requesting confirmation. If you are uncertain, choose the most
  reasonable interpretation, state the assumption in your final summary, and proceed.
- Do not end a turn with plain text while work remains. A turn with no tool calls and no
  completion declaration is treated as incomplete and the run will continue.
- Verify by execution, never by assumption. Before declaring the task complete, gather concrete
  evidence from your own tool output that every requirement is met: run the program or tests,
  confirm each required output file exists at the exact requested path, and read the output back
  to confirm its contents.
- If you delegate work to a subagent, do not declare completion until the delegated work has
  landed and you have verified its results yourself.
- When (and only when) all requirements are verified, declare completion with the
  task_complete flag in your final response.
```

Deliberate wording choices, each with a precedent:

- "There is no user at the other end" (Cline: "the user who you cannot communicate with
  directly") — states the channel fact up front; it is what makes the auto-answer nudge in
  §12.5 coherent instead of surprising.
- "Never end a turn by asking … a question" + "choose the most reasonable interpretation" (Dirac
  auto-response) — converts confirmation-seeking into a documented assumption.
- "A turn with no tool calls and no completion declaration is treated as incomplete and the run
  will continue" (Cline yolo: "Response without the submit_and_exit tool call will considered not
  completed and the task will continue") — tells the model the loop it is in, so an early stop is
  self-correcting instead of fatal.
- "Verify by execution, never by assumption" (Cline, verbatim) — the missing-deliverable guard.
- Subagent clause (Cline team guard / Claude Code background wait) — the spawn-and-abandon guard,
  in prompt form, backed by the §12.2 gate in code.

### 12.4 Completion semantics

- **Declaration**: an explicit, machine-checkable marker in the final assistant text of a
  tool-free turn. Two candidate shapes: (i) a `task_complete` JSON field in a fenced block
  (Terminus-2 shape, cheap to parse, works with any chat model); (ii) a `task_complete` tool.
  Recommend (i) for v0: it needs no new tool in the core and the ACP layer can regex-check the
  settled assistant entry.
- **Two-step confirmation** (Terminus-2 shape): first declaration → ACP server sends
  "Are you sure you want to mark the task as complete? This will trigger your solution to be
  graded and you won't be able to make any further corrections. If so, declare task_complete
  again." Second declaration → done. One extra model call per trial; this is the single cheapest
  structural fix for premature completion / missing deliverables, and it is the one detail the
  73.0% reference harness has that none of the GUI-first products replicate.
- **No implicit completion**: a tool-free turn *without* the marker is never completion (this is
  what converts failure modes 3 and 5 from "trial over" into "episode N+1").

### 12.5 Continuation prompts (nudge texts)

The ACP layer composes the continuation prompt; each variant **re-anchors the task** (the
greeting-degeneration countermeasure — never send bare state):

- **Generic early stop** (no tools, no completion, no question detected):
  `The task is not complete. Task: {task}. Continue from the current state; do not re-introduce
  yourself and do not ask questions.`
- **Question detected** (final text ends in/contains a direct question — v0: cheap heuristic on
  the settled assistant text; refine later): Dirac-shaped auto-answer:
  `There is no user to answer questions in this session. Resolve "{question}" yourself with your
  tools (search, read, run), pick the most reasonable option, and continue the task: {task}.`
- **Empty stop** (no output, no tools):
  `Your previous response was empty. Continue the task: {task}.`
- **Parse/format issues** (if a structured completion marker is malformed): Terminus-2-shaped:
  `Previous response had parsing errors: {feedback}. Fix these issues and provide a proper
  response.`

### 12.6 Concrete parameters (recommended defaults)

| Parameter | Value | Rationale / precedent |
|---|---|---|
| `max_episodes` | **100** (configurable; 0 = unlimited) | Terminus-2 defaults to 1,000,000 (unbounded) — fine for a benchmark harness, wrong default for a product. 100 × typical episode cost is already far beyond any sane task; the cap is a safety valve, not a target. |
| Completion confirmation rounds | **1** (two-step) | Terminus-2. |
| Max consecutive *empty* stops | **3** → then settle the run as failed | mini-swe-agent `max_consecutive_format_errors = 3`; Cline treats empty as an error. Prevents a degenerate model from burning the whole episode budget on empty turns. |
| Max consecutive *identical* continuation nudges | **3** → settle | New (no direct precedent; Cline's unbounded nudge loop is the anti-pattern this caps — see §12.7). |
| Subagent gate | hold completion until children settle; **no extra timeout** beyond child completion | Cline team guard + Claude Code's bounded wait; a timeout risks re-creating spawn-and-abandon. |
| Question auto-answer | always on in headless | Dirac/Copilot/Claude Code all close the channel; tau's failure data says blocking on it is the top failure class. |
| Spawn tools in headless | **enabled, gated** (not disabled) | Cline disables spawn in yolo, but tau's eval load includes legitimate delegation; the gate + prompt clause handle the failure mode while keeping the capability. Revisit if the gate proves insufficient. |

### 12.7 Risks and anti-patterns

1. **Nudge loops.** Cline's design re-injects its `[SYSTEM]` completion reminder on *every*
   tool-free turn with no visible cap in the yolo path (only `maxIterations`, unbounded by
   default). A model that keeps stopping will ping-pong the nudge forever, burning budget with no
   progress. **Mitigation**: the consecutive-identical-nudge cap in §12.6, and nudges that vary
   content (state + task) so a stuck pattern is visible in the transcript.
2. **Re-prompt confusion (the greeting trap).** If a continuation prompt is just "here is the
   current state", a weak model reads it as a fresh session and re-greets (tau failure mode 2 is
   exactly this). **Mitigation**: every continuation re-states the task and the
   no-re-introduction rule (§12.5); Terminus-2 avoids the trap only because its model contract
   (JSON batches) leaves no room for a greeting.
3. **Completion-marker gaming.** A two-step confirmation makes a model that learns "declare
   complete twice" trivially pass — but that is *correct* behavior (it did complete, twice). The
   real risk is the inverse: a strict parser rejecting a valid declaration. **Mitigation**: parse
   leniently, log every confirmation round in the session transcript.
4. **Subagent interaction.** The gate must treat a *crashed* child as settled (otherwise one
   dead child hangs the run); and a child that itself declares completion must not be confused
   with the parent's completion — the completion marker is parent-session-scoped. Cline's
   `team_await_runs` and Claude Code's 10-minute idle ceiling are the two reference points; tau
   should take Cline's semantics (wait for settlement) and Claude Code's ceiling only as a
   last-resort alarm log, not a cutoff.
5. **Protocol drift.** The episode loop must not emit a `session/prompt` response until the real
   end; intermediate episodes surface as ordinary `session/update` notifications (assistant
   chunks + tool calls), which the ACP spec already covers. Cancellation (`session/cancel`) must
   abort the episode loop, not just the in-flight episode — the existing `cancel_requested`
   plumbing in `sessions.rs` extends naturally.
6. **Prompt-frame leakage.** The headless frame must apply to the *parent* session only if we
   want children to keep the interactive frame — decide explicitly (recommendation: children
   inherit the headless frame; a child that greets is the same bug one level down).

### 12.8 What NOT to copy

- **Claude Code/Codex "no auto-continue"**: correct for interactive products with strong models;
  it is precisely what produces tau's failure modes on qwen3.8-27b. The benchmark data (73.0%
  Terminus-2 vs the 40%-headless-failure baseline) is the argument.
- **Terminus-2's 1,000,000-episode default**: a benchmark harness can afford unbounded; a product
  cannot.
- **Cursor's blocking `ask_question` in ACP**: "if your client does not answer … tool execution
  can block" is a hang by design — the worst possible question-channel semantics for unattended
  runs.

---

## 13. Sources

**Local (fetched/checked out during this task)**
- Terminus-2: `/tmp/harbor-docs/src/harbor/agents/terminus_2/` — `terminus_2.py` (loop L1378,
  completion confirmation L626-650 & L1529-1541 & L1655-1663, parse-error nudge L1480-1484,
  output-limit nudge L1257-1261, handoff prompt L1073-1078, default max episodes L440),
  `templates/terminus-json-plain.txt`, `templates/timeout.txt`;
  `/tmp/harbor-docs/src/harbor/agents/installed/mini_swe_agent.py` (adapter: `--yolo …
  --exit-immediately`).
- Cline (`cline/cline`, `main`): `sdk/packages/shared/src/prompt/system/yolo.ts` (verbatim yolo
  prompt), `…/system/act.ts` (interactive prompt), `sdk/packages/agents/src/agent-runtime.ts`
  (L753-760 completion reminder; loop; empty-response error),
  `sdk/packages/core/src/runtime/orchestration/runtime-builder.ts` (L847-860 team guard),
  `apps/cli/README.md` (`--yolo`, `--retries`, zen profile).
- mini-swe-agent (`SWE-agent/mini-swe-agent`, `main`): `src/minisweagent/config/mini.yaml`
  (system + instance templates, format-error template), `src/minisweagent/agents/default.py`
  (L96 `while True`, `AgentConfig` limits), `src/minisweagent/run/mini.py` (L61-65 `--yolo`,
  `--exit-immediately`).
- Dirac (`dirac-run/dirac`, `master`): `src/core/prompts/system-prompt/template.ts` (verbatim
  prompt), `src/core/task/tools/modules/respond/QuestionResponseOperation.ts` (L21-29 yolo
  auto-response), `…/CompletionResponseOperation.ts` (double-check verification),
  `src/core/task/TaskRequestLoop.ts` + `src/core/task/index.ts` (loop); leaderboard
  `metadata.yaml` pinning `agent_url` (Hugging Face
  `harborframework/terminal-bench-2-leaderboard`).
- Codex (`openai/codex`, `main`): `docs/exec.md` → `developers.openai.com/codex/noninteractive.md`
  (full non-interactive doc), `codex-rs/core/gpt-5.2-codex_prompt.md` (model prompt),
  `codex-rs/prompts/templates/permissions/**` (approval-policy/sandbox prompt injections),
  `codex-rs/core/src/session/turn.rs` (`run_turn` loop).
- Claude Code: npm `@anthropic-ai/claude-code` 2.1.289 binary (verbatim prompt opening lines +
  attribution header); official docs `code.claude.com/docs/en/{headless,permission-modes,cli-reference,agent-sdk/permissions,agent-sdk/user-input}.md`.
- Cursor: official docs `cursor.com/docs/cli/{headless,using,acp,reference/permissions,reference/parameters}.md`.
- Copilot CLI: official docs `docs.github.com/en/copilot/{how-tos/copilot-cli/automate-copilot-cli/run-cli-programmatically,reference/copilot-cli-reference/cli-command-reference,reference/copilot-cli-reference/cli-programmatic-reference,concepts/agents/copilot-cli/understanding-local-sandboxing}.md`; `github/copilot-cli` (README, changelog).
- OpenHands: `docs.openhands.dev/openhands/usage/cli/headless.md`.
- ACP spec: `agentclientprotocol.com/protocol/v1/prompt-turn.md`.
- tau: `crates/tau-acp/src/{server,pump,config,sessions}.rs`, `crates/tau-core/src/harness/launch.rs`.

**Unverified claims** (explicitly marked inline): Cursor and Copilot CLI prompt wording (closed
source); Cursor/Copilot continuation parameters (undocumented); Dirac's exact headless CLI flag
spelling; Aider/Goose secondary notes (not investigated in depth).
