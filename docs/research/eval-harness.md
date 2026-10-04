# Research: an in-repo evaluation rig for the tau agent harness

Researched 2026-10-02. Follow-on to research issue #51 (harness techniques with demonstrated Terminal-Bench edge). This doc answers a different question: how do existing agent-eval systems *implement* their evaluation infrastructure, and what does that evidence support for a tau-native rig? All external claims cite a primary source fetched in this session; the tau-specific facts are verified against this repo.

**Question.** How should tau build an in-repo evaluation rig that measures *harness* changes — tool dispatch, protocol shape, session integrity, context management, scaffold features — rather than just model capability? What task format, runner design, metrics, ablation support, CI integration, and cost controls does the published evidence support?

**Short answer.** Steal the *task anatomy* (instruction + environment setup + check script + optional oracle) that Terminal-Bench/Harbor, SWE-bench, and Aider all converge on, but run it **in-process against `tau-core` in a temp-dir workspace** instead of in per-task Docker containers. Score by a **check-script exit code**. Run **two legs**: a **deterministic leg** (mock-LLM known-good trajectory, harness invariants, PR-gated in nextest, free) and a **live leg** (real model, TB-style pass rate, nightly/local, cost-capped). Drive **ablation** through the existing TOML config layering (each scaffold feature is a config key) plus a **matrix runner** (feature on/off × model). This maps directly onto tau's existing mock-first acceptance discipline, `nextest`, the `justfile`, and in-process `tau-core` drive that `tau-test` already demonstrates.

**Revision 2026-10-03 (as shipped).** The final shape diverged from the short answer in three load-bearing ways, all by user decision: (1) the rig is **live-only** — the mock tier was cut because metrics only mean anything against the construct being measured (harness → real-model efficacy on TB tasks); deterministic harness *correctness* is the acceptance suites' job (ADR-0010), and the `tau-mock-llm` stays their test model; (2) **every trial runs inside the task's own container image** — the TB environment model, no native class; (3) **the dataset is fetched at runtime, never vendored** — the full Terminal-Bench 2.0 is a shallow clone of the upstream repo into `target/terminal-bench` (a no-op once present), with the curation — 86 selected tasks, 3 quarantined with reasons on record — in `eval/tb-selection.toml`. Images pull lazily per trial; each trial runs the upstream `tests/test.sh` protocol (tests staged at `/tests`, verdict at `/logs/verifier/reward.txt`). The `tier` concept and the native trial path are gone: `tau-eval live [--reps N] [--budget USD] [--filter SUBSTR]`, wrapped by `just eval`. The "deterministic leg" references below describe the design that was built and then cut.

---

## 1. How existing harnesses implement their eval infrastructure

The consistent finding across every system below: an eval rig is three decoupled pieces — a **self-contained task package**, a **runner that instantiates an isolated environment per trial and drives one agent in it**, and a **verifier whose exit status (or emitted reward) is the score**. Trajectory capture and cost accounting are recorded per trial. The differences are in isolation mechanism (Docker vs in-process), scoring granularity (binary vs reward), and how agents plug in (adapter interface).

### 1.1 Terminal-Bench 2.0 / Harbor

**Task anatomy.** A task is a directory of files, independent of the framework:

```
my-task/
├── instruction.md      # the prompt given to the agent
├── task.toml           # config + metadata (timeouts, cpus, memory, gpus, internet)
├── environment/
│   └── Dockerfile      # container environment (or docker-compose / Apptainer.def)
├── solution/
│   └── solve.sh        # reference ("oracle") solution
└── tests/
    └── test.sh         # verifier
```

Harbor's task-format doc describes a task as "an instruction, environment, and test script" and stresses tasks are "independent, isolated, reproducible pieces of code" with "no dependency on the Harbor framework" — they "can easily be plugged into any framework that supports the Harbor format" (https://harborframework.com/docs/task-format). The Terminal-Bench contribution guide shows the same layout and that `task.toml` carries per-component timeouts (`[agent] timeout_sec`, `[verifier] timeout_sec`), resource limits, and `allow_internet` (https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/CONTRIBUTING.md).

**Scoring.** The verifier is `tests/test.sh`, which "must produce a numerical reward" at `/logs/verifier/reward.txt` (or `reward.json` for multi-dimensional/labeled rewards) (https://harborframework.com/docs/task-format). The paper confirms tests "verify that all outcomes described in the instruction have been achieved by testing properties of the final container state; they do not test the agent's commands or console output" — an outcome-driven, approach-agnostic score (https://arxiv.org/html/2601.11868v1).

**Harness-adapter interface.** Agents plug in through a Python `BaseAgent` ABC with the abstract methods `name()`, `version()`, `setup(environment)`, and `run(instruction, ...)` (https://raw.githubusercontent.com/harbor-framework/harbor/main/src/harbor/agents/base.py). Harbor ships pre-integrations for `claude-code`, `codex`, `mini-swe-agent`, `terminus-2`, `oracle`, `nop`, and ~40 more, selected by `-a <agent> -m <model>` (https://harborframework.com/docs/agents). Two agents are load-bearing for rig design: **`oracle`** runs the reference `solve.sh` (used to confirm a task is solvable in your sandbox), and **`nop`** does nothing (a control that *must fail* — a task `nop` passes is broken). Capability flags (ATIF trajectory, resume, MCP servers, skills) are declared per agent (https://harborframework.com/docs/agents). The mini-swe-agent adapter is a thin wrapper that normalizes the agent's message log into the ATIF schema (https://raw.githubusercontent.com/harbor-framework/harbor/main/src/harbor/agents/installed/mini_swe_agent.py).

**Trajectory capture.** Harbor's standard trajectory format is **ATIF** (Agent Trajectory Interchange Format): a JSON spec logging the complete interaction history — messages, reasoning, tool calls, observations — plus per-step metrics (`prompt_tokens`, `completion_tokens`, `cached_tokens`, `cost_usd`, optional token-ids and logprobs) and `final_metrics` (`total_prompt_tokens`, `total_completion_tokens`, `total_cost_usd`, `total_steps`). It explicitly supports `subagent_trajectories` for embedding child-agent traces in one file (https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md). This is the reference for what a trajectory artifact should record: the dialogue *and* the per-step cost/token accounting *and* subagent nesting.

**Scale + cost.** Runs are parallel container jobs (`--n-concurrent`, `--env modal/daytona/docker`); the paper reports 32–100 containers in parallel and a full 89-task run costing "$1 to $100 depending on the model's price," most tasks <20 min, worst case ~2h / ~100M tokens on one task (https://arxiv.org/html/2601.11868v1 §3.4, §4.1).

**Quality gates (worth stealing).** Before a task is accepted: the oracle solution must pass, a no-op agent must fail, an adversarial "exploit" agent is run to catch cheat paths, and static checks ensure the Dockerfile doesn't leak tests/solutions (https://arxiv.org/html/2601.11868v1 Appendix B).

### 1.2 SWE-bench (verified) + mini-swe-agent

**Task contract.** A SWE-bench instance is a fixed record: `repo` + `base_commit` (the codebase, materialized from a mirror by checking out the base commit), `problem_statement` (natural-language issue), `test_patch` (unseen tests), `patch` (reference solution), `FAIL_TO_PASS` (list), `PASS_TO_PASS` (list), `env_install_commit` (https://arxiv.org/html/2310.06770 Appendix A.2, Table 9). The agent's job is to emit a **model patch**; the harness applies it and runs the tests.

**Scoring (F2P/P2P).** `FAIL_TO_PASS` = "tests that change in status from fail to pass"; `PASS_TO_PASS` = "tests that change in status from pass to pass" (https://arxiv.org/html/2310.06770). An instance *resolves* only if **all** F2P pass **and** **all** P2P keep passing — P2P is the regression guard that catches a patch which fixes the issue but breaks unrelated behavior. Dataset construction enforces ≥1 F2P per instance and a median of 51 P2P (https://arxiv.org/html/2310.06770 §2). This two-list split — *target tests* vs *regression tests* — is the key scoring idea to import.

**Containerized harness.** Since 2024-06 the harness is "fully containerized … using Docker for more reproducible evaluations" (https://github.com/swe-bench/swe-bench README News). The CLI: `swebench eval verified -p <preds> --run-id <id> -j <workers>`, and `swebench infer verified -m <model>` generates predictions with mini-SWE-agent (https://github.com/swe-bench/swe-bench README Usage).

**mini-swe-agent batch infra.** mini-swe-agent is ~100 lines of Python for the agent class; it "supports local environments, docker/podman, singularity/apptainer, bubblewrap, contree" (https://raw.githubusercontent.com/SWE-agent/Mini-SWE-Agent/main/README.md). Its batch path is `mini-extra swebench --model X --subset verified --split test --workers N`: batch mode "runs on all task instances in parallel," a separate `swebench-single` runs one instance interactively for debugging, and it writes a `preds.json` that the evaluation step consumes (https://mini-swe-agent.com/latest/usage/swebench/). Notable rig mechanics: `--slice 0:5` and `--filter <regex>` select subsets, `--redo-existing` re-runs, and **resume is by re-running** — "the completed instances are inferred from `preds.json`" (https://mini-swe-agent.com/latest/usage/swebench/). That "preds file is the checkpoint" pattern is a cheap, robust way to make long batch runs resumable.

### 1.3 DeepSWE (neutral-harness pinning)

DeepSWE's central methodological decision is the one most relevant to a *harness* eval: **pin one neutral harness across all models.** "Every run uses `mini-swe-agent`, the harness the SWE-bench authors built. We hold it fixed across every model so the leaderboard reflects model capability, not the scaffolding around it." The rationale: native products (Codex CLI, Claude Code, …) "each ship with editing primitives the model was trained on (`apply_patch` on GPT, `str_replace_based_edit_tool` on Claude) and a system prompt tuned for that specific model. Including any of that would mean the leaderboard reflects scaffolding choices as much as model capability." `mini-swe-agent` is "model-agnostic by design: every model gets the same `bash` tool and the same shared prompt" (https://deepswe.datacurve.ai/blog/deepswe). They validated the pin with a pilot running each model under both the neutral and its native harness — "mini-swe-agent matches or beats native harnesses on the same tasks" (e.g. claude-opus-4.7: 50% mini vs 40% Claude Code) (https://deepswe.datacurve.ai/blog/deepswe).

Other transferable practices: every task pins "to an immutable commit hash so runs are reproducible"; each task ships three artifacts (prompt, executable verifier, reference solution used only at review, "never used at grading time"); the verifier "asserts through public APIs and observable outputs, not through private helpers"; every verifier is run 3× at authoring to flag flakiness; and trials with "API errors, timeouts, and other transient harness failures" are **excluded from each denominator** so infra noise isn't scored as model failure (https://deepswe.datacurve.ai/blog/deepswe). Leaderboard metrics per model: pass rate ± CI, avg cost, output tokens, steps (https://deepswe.datacurve.ai/).

**The inversion for tau.** DeepSWE pins the harness to isolate the *model*. tau's rig does the inverse: pin the *task + verifier* and vary the *harness config* (scaffold features) to isolate harness contributions. Same machinery, opposite axis of variation.

### 1.4 Aider polyglot

225 Exercism exercises across 6 languages, deliberately the hardest subset ("solved by 3 or fewer" of 7 reference models) to keep frontier scores in a 5–50% window with headroom (https://aider.chat/2024/12/21/polyglot.html). Run "inside a docker container" for safety; `benchmark.py --model X --edit-format diff --threads 10 --exercises-dir polyglot-benchmark` (https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md). The report is a YAML record whose headline stats are `pass_rate_1` and `pass_rate_2` — "the percent of the tasks which had all tests passing," one entry "depending on the value of the `--tries` parameter" — i.e. **score per attempt** (first-shot vs after the agent is allowed to react to test feedback). It also records `percent_cases_well_formed`, malformed/syntax-error counts, `exhausted_context_windows`, `test_timeouts`, `seconds_per_case`, and `total_cost` (https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md). The per-attempt pass-rate split is a useful metric: it separates "gets it right cold" from "can self-repair given test output."

### 1.5 Anthropic / OpenAI published internal-eval practice

**Anthropic — evaluating agents** (https://www.anthropic.com/engineering/multi-agent-research-system):
- "Start evaluating immediately with small samples" — ~20 queries early on; large effect sizes are visible in a few test cases, so don't delay building evals.
- **End-state evaluation** for state-mutating multi-turn agents: "evaluate whether it achieved the correct final state" rather than prescribing intermediate steps; "break evaluation into discrete checkpoints where specific state changes should have occurred."
- LLM-as-judge scales when a single call scores a rubric 0.0–1.0 + pass/fail; human eval still catches what automation misses.

**Anthropic — evaluating tools** (https://www.anthropic.com/engineering/writing-tools-for-agents):
- "Building an evaluation allows you to systematically measure the performance of your tools."
- Generate many eval tasks grounded in real use; pair each prompt with a **verifiable** outcome (exact-match or LLM judge); "avoid overly strict verifiers that reject correct responses due to spurious differences."
- Run programmatically with direct API calls, "one loop for each evaluation task."
- **Metrics beyond accuracy:** "total runtime of individual tool calls and tasks, the total number of tool calls, the total token consumption, and tool errors."
- Use **held-out test sets** "to ensure we did not overfit to our 'training' evaluations."
- Prompt-engineer **error responses** to be "specific and actionable … rather than opaque error codes or tracebacks" — directly relevant to the exit-127 enrichment decision in §1.7.

**OpenAI — practical guide to building agents** (https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf): step 01 is "Set up evals to establish a performance baseline"; the model-selection method is "build your agent prototype with the most capable model for every task to establish a performance baseline. From there, try swapping in smaller models to see if they still achieve acceptable results." Baseline-first, then ablate downward.

**Synthesis of the ablation discipline.** All four sources converge on: (a) a small, verifiable, end-state-scored task set; (b) per-trial accounting of tokens/cost/turns/errors; (c) a baseline configuration; (d) held-out tasks to prevent overfitting; (e) one variable changed at a time. That is exactly the "every harness feature is a hypothesis; no delta on model upgrade → delete" rule, made operational.

### 1.6 Open-source frameworks worth stealing structure from

Picked the two most structurally relevant (a task-package reference and a pipeline/SDK-pinning reference); AgentBench was considered but is a multi-environment *benchmark suite* rather than a rig, so it contributes less to implementation.

- **Original t-bench** (`harbor-framework/terminal-bench-1`) — the ancestor task package: `Dockerfile`, `docker-compose.yaml`, `task.yaml`, `run-tests.sh`, `solution.sh`, `tests/` per task (https://api.github.com/repos/harbor-framework/terminal-bench-1/contents/original-tasks/adaptive-rejection-sampler). Confirms the instruction+tests+oracle+environment quartet is the stable, reusable core that survived the Harbor rewrite.
- **OpenHands benchmarks** (`OpenHands/benchmarks`) — a *separate repo* holding "standardized evaluation pipelines for testing agent capabilities across various real-world tasks," one directory per benchmark (`swebench`, `gaia`, `commit0`, …) each with `config.py`, `run_infer.py`, `eval_infer.py`, `prompts/` (https://github.com/OpenHands/benchmarks). Two structural ideas: (1) **pin the agent SDK to a reproducible commit via a git submodule** so the eval always runs against known agent code; (2) a **JSON LLM config file** (`.llm_config/`) decoupling model selection from the pipeline. For tau the analogue is: the rig is a workspace crate, and the "SDK commit pin" is trivially satisfied because the rig compiles `tau-core` from the same tree it tests.

### 1.7 Micro-analysis: exit-127 / "command not found" failures

**What the public data shows.** The only fetched source with a *quantified* command-failure taxonomy is the Terminal-Bench 2.0 paper's command-level error analysis: an LLM-as-judge (GPT-5) reviews individual command input/output pairs from recorded Terminus 2 trajectories. Command error rates range 9.2% (Grok 4) to 26.7% (GPT-OSS-120B) of all commands. Of all command failures, **"command failures calling executables that are not installed or not in PATH are the most frequent (24.1% of all failures), followed by failures when running executables (9.6%)"** (https://arxiv.org/html/2601.11868v1 §4.5).

**The observed split — and its absence.** The paper's taxonomy (Appendix E.2) deliberately *merges* the two cases the tau decision cares about into one bucket: "Command not found on PATH: Shell cannot locate the requested executable because it is **not installed or not in PATH**; this does not include explicit path invocations" (https://arxiv.org/html/2601.11868v1 Appendix E.2). A distinct subcategory, "Executable missing at specified path," covers the explicit-path case only. So the published 24.1% is a **combined** not-installed + not-in-PATH figure; **no fetched public source breaks that bucket into the two sub-cases.** I checked the one public trajectory dataset (`yoonholee/terminalbench-trajectories`, 52.1k rows) — its per-step `steps` field is null in the served rows, so it carries no command-level detail to re-bucket (https://huggingface.co/datasets/yoonholee/terminalbench-trajectories).

**Implication for the tau bash-tool decision.** The external evidence establishes the *magnitude* (a quarter of all command failures are lookup failures — the single largest command-failure class) but not the *not-installed vs not-in-PATH* ratio, because the one public taxonomy that measured it chose to collapse them. That gap is exactly what a tau-controlled bash tool can close: since tau owns the tool, its error output can disambiguate "binary absent from the system" from "binary present but not on this shell's PATH" (e.g. by probing the known install locations / package managers before emitting the failure), turning the largest single command-failure class into an actionable, self-explanatory message — the "specific and actionable" error-response principle from §1.5. **The rig is what makes the split measurable at all**: the deterministic leg can run a purpose-built "missing command" task against the mock and assert which message the bash tool emits, and the live leg can re-bucket real trajectories. Until the rig exists, the honest answer to "what is the split?" is: *not publicly quantified; TB 2.0 reports the combined 24.1%.*

---

## 2. Proposal: the tau in-repo eval rig

Grounded in these verified repo facts: `tau-core` is a standalone Rust library with no Tauri dependency (its deps are serde/serde_json/tokio/reqwest/toml/zstd/xxhash/notify + `tau-protocol`; `crates/tau-core/Cargo.toml`); `tau-mock-llm` is a deterministic Responses-API mock served from hash-pinned scenario files in `fixtures/e2e-mocks/` (`crates/tau-mock-llm/Cargo.toml`, `justfile`); the acceptance driver `tau-test` already drives `tau-core` **in-process** (`AgentSession` + `Provider::with_model(endpoint, model)` + `SessionStore`) against the deterministic mock (`crates/tau-test/src/main.rs`); tests run under `nextest` (`justfile` `test` → `cargo nextest run --workspace`); config is TOML with section-level layering of system `~/.config/tau/config.toml` under project `.tau/config.toml` (`crates/tau-core/src/config.rs`); a workspace is `(id, name, cwd)` (`crates/tau-protocol/src/snapshot.rs`); sessions are JSONL under `{project}/.tau/sessions/` (`crates/tau-core/src/session.rs`, `subagent.rs`).

### 2.1 Task format

A tau task is a small directory, borrowing the Harbor/t-bench/SWE-bench anatomy (instruction + environment + check + oracle) but sized for hand-authoring and local temp-dir execution (no Docker required by default):

```
eval/tasks/<task-id>/
├── task.toml         # id, tags, timeout_sec, workdir seed, expected invariants
├── instruction.md    # the prompt given to the agent (the only file the agent sees)
├── setup.sh          # builds the initial workspace state in the temp dir (idempotent)
│                     #   — or a seed/ dir copied in instead, for trivial tasks
├── check.sh          # runs in the workspace after the agent finishes; exit 0 = pass
└── oracle/           # optional
    └── solve.sh      # known-good solution; run by the rig to confirm the task is solvable
```

Design choices, each tied to a source:
- **`check.sh` exit code is the primary score** (0 = pass, non-zero = fail), matching Harbor's "test script … produce a reward" reduced to the binary case, and Aider/SWE-bench's "all tests passing" (https://harborframework.com/docs/task-format, https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md). An optional `reward.txt` (0.0–1.0) written by `check.sh` gives partial credit where a binary is too coarse — Harbor's `reward.json` multi-dimensional case (https://harborframework.com/docs/task-format).
- **`setup.sh` instead of `Dockerfile`** by default: tau runs in a temp dir on the host, so the environment is "whatever `setup.sh` makes," not a container image. This keeps tasks hand-writable and the rig dependency-free. A task that genuinely needs an isolated/container environment can mark itself `environment = "docker"` in `task.toml` and the rig builds a container — but that is an opt-in extension, not the default path (the default path is what Harbor calls the task's core: instruction + tests, framework-agnostic — https://harborframework.com/docs/task-format).
- **`oracle/solve.sh`** imports Harbor's oracle/nop quality gates: the rig runs the oracle to confirm the task *is* solvable (oracle must pass) and runs a no-op to confirm the task *isn't* trivially passing (nop must fail) (https://arxiv.org/html/2601.11868v1 Appendix B, https://harborframework.com/docs/agents).
- **`task.toml`** carries the per-component timeouts and tags that Harbor's `task.toml` and SWE-bench's instance record both use (`[agent] timeout_sec`, `[verifier] timeout_sec`; `FAIL_TO_PASS`/`PASS_TO_PASS`-style expected outcomes) (https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/CONTRIBUTING.md, https://arxiv.org/html/2310.06770).
- **Target + regression split.** `check.sh` (or `task.toml`) distinguishes *target* assertions (the behavior the instruction asks for — the F2P analogue) from *regression* assertions (pre-existing behavior that must keep holding — the P2P analogue), so a task can fail specifically because the harness broke an unrelated invariant (https://arxiv.org/html/2310.06770).

### 2.2 Runner design

A new `tau-eval` crate (a Rust bin, plus a `#[cfg(test)]` surface for the deterministic leg) that, per trial:

1. **Temp-dir workspace.** `tempfile` makes a fresh dir; the rig materializes a tau workspace `(id, name, cwd)` with `cwd` = that dir (matching the `Workspace` triple, `crates/tau-protocol/src/snapshot.rs`), runs `setup.sh` with `cwd` set to it, and points the project `.tau/` root there so the session JSONL lands in a per-trial `{tmp}/.tau/sessions/` (the existing session path, `crates/tau-core/src/session.rs`). One workspace per trial is the "independent, isolated, reproducible" property Harbor requires (https://harborframework.com/docs/task-format) and the per-task-container property SWE-bench/Aider get from Docker, achieved here with a temp dir.
2. **In-process core drive.** The rig drives `tau-core` the way `tau-test` already does — `AgentSession` + `Provider::with_model(endpoint, model)` + `SessionStore` in the same process (https://github.com/aaronlockhartdev/tau `crates/tau-test/src/main.rs`). No subprocess, no Tauri: `tau-core` has no Tauri types, so a plain Rust bin links it directly. The provider endpoint is the rig's one knob for leg selection (mock URL vs live `TAU_ENDPOINT`).
3. **Scoring = check-script exit.** After the agent turn(s) complete (or the `[agent] timeout_sec` fires), the rig runs `check.sh` in the workspace and records its exit status (and optional `reward.txt`) as the trial score.
4. **Concurrency.** A bounded `tokio` task pool, one trial per task, mirroring the worker-pool pattern every external rig uses (`mini-extra swebench --workers N`, `benchmark.py --threads N`, `harbor run --n-concurrent N`) (https://mini-swe-agent.com/latest/usage/swebench/, https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md, https://arxiv.org/html/2601.11868v1 §3.4). Default small (e.g. 4); the deterministic leg can run higher because trials are cheap.
5. **Artifact capture, per trial.**
   - **Trajectory:** the session JSONL the core already writes to `{tmp}/.tau/sessions/` — copied into the trial's artifact dir. This is tau's native analogue of ATIF's "complete interaction history … messages … tool executions … environment feedback" (https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md); a later step can normalize it to an ATIF-shaped record (steps + per-step tokens/cost + `subagent_trajectories`) for cross-tool comparison.
   - **Tokens + cost:** from the provider responses the core already accounts (the mock returns deterministic usage; a live endpoint returns real usage), aggregated to per-trial `total_prompt_tokens` / `total_completion_tokens` / `total_cost_usd` — the ATIF `final_metrics` set (https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md).
   - **Wall time:** harness-measured per trial.
   - **Score + invariants:** the `check.sh` exit/reward plus the deterministic-leg invariant results (§2.3).
   - All written to a per-trial `results.json` and appended to a run-level `preds.json`-style checkpoint file, so an interrupted batch run resumes by skipping completed task-ids — mini-swe-agent's "completed instances are inferred from `preds.json`" pattern (https://mini-swe-agent.com/latest/usage/swebench/).
6. **Transient-failure exclusion.** Trials that die on infra error (provider 5xx, timeout, harness panic) are marked `error` and **excluded from pass-rate denominators**, per DeepSWE's "excludes trials with API errors, timeouts, and other transient harness failures from each denominator" (https://deepswe.datacurve.ai/blog/deepswe) — so a flaky network isn't scored as a harness regression.

### 2.3 Two legs

**Leg 1 — deterministic (PR-gated, free).** The mock-LLM scenario in `fixtures/e2e-mocks/` is a *known-good trajectory*: a fixed script of model responses. The rig runs a task against it and asserts **harness invariants**, not task outcomes:
- **Protocol** — every request/response the core emits is well-formed against the `tau-protocol` types (the "percent_cases_well_formed" invariant, Aider — https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md).
- **Session integrity** — the session JSONL is well-formed, append-only-consistent, and round-trips through `SessionStore` (the end-state/integrity checkpoint idea, Anthropic — https://www.anthropic.com/engineering/multi-agent-research-system).
- **Tool dispatch** — the tools the scripted model requested are the tools the core actually invoked, with the right arguments and results fed back (the "did the agent call the expected tools" check, Anthropic tool eval — https://www.anthropic.com/engineering/writing-tools-for-agents).
- **No-panic / determinism** — the run completes without panic and, because the model is deterministic, a repeated run is byte-identical at the invariant level.

This leg is the tau analogue of the existing mock-first acceptance suites: "a red is a code problem, never a network/model problem." It runs inside `nextest` (a `tau-eval` test target), so it is **free, fast, and gates PR merges** in the existing `just test` / CI test job with no new infrastructure. This is the high-value first piece because it turns "did my harness change break the protocol/session/dispatch" from a manual dogfood into an automated, CI-blocking assertion.

**Leg 2 — live (on-demand, cost-capped).** Point the same runner at a real model and score a TB-style task set by `check.sh` pass rate. The model configuration is the **user's own tau config** — the app's `config::load` path (system `~/.config/tau/config.toml` under project `.tau/` layering, `crates/tau-core/src/config.rs`) — not a rig-invented `TAU_ENDPOINT`/`TAU_MODEL`: model configuration is more than endpoint + name (provider set, per-model facts, defaults), and harness efficacy is only meaningful under the configuration the user actually runs. (The acceptance seam that once read those env vars was retired 2026-10-02, ADR-0010; the live leg is their only remaining consumer, and it consumes the user config directly.) This is the DeepSWE/Aider/SWE-bench "benchmark" leg: measures real capability, is non-deterministic, and is therefore **not PR-gated** — it runs as a specifically invocable experiment, with a cost cap (§2.6). Its output is the pass-rate + metric table that the ablation matrix consumes.

The two legs share the *entire* runner; only the provider endpoint and the assertion set differ. That shared plumbing is what makes the rig cheap to build once.

### 2.4 Metrics

Per trial, and aggregated per (task × model × config) cell:

| Metric | Definition | Harness feature it maps to |
|---|---|---|
| **Pass rate** | fraction of non-error trials with `check.sh` exit 0 (F2P∧P2P) | headline harness effectiveness; the one number a model-upgrade comparison uses |
| **Tokens** (prompt / completion / cached) | from provider usage, per ATIF `final_metrics` | context management / compaction (OM), prompt size, tool-result truncation |
| **Cost** | `total_cost_usd` per trial | efficiency of the same context/truncation features, in money terms |
| **Turns / episodes** | agent loop iterations per trial | agentic-loop efficiency, tool-batching policy. (Treated as an *efficiency* metric, not a quality one — TB found turn count and token count do **not** correlate with success: r=−0.028 and r=−0.170, both p>0.5 — https://arxiv.org/html/2601.11868v1 Appendix G.) |
| **Rework rate** | fraction of tool calls that are a retry of a prior failed call (parsed from the trajectory) | self-repair / verification loops, and directly the exit-127 error-enrichment decision (§1.7) |
| **Gate-fail rate** | (deterministic leg) fraction of known-good scenarios that fail an invariant | harness *regression* signal — the metric that actually blocks a PR |
| **Well-formed rate** | fraction of protocol messages validating against `tau-protocol` | protocol stability across a refactor |
| **Error (excluded) rate** | fraction of trials dropped as transient infra failures | runner/sandbox reliability, not model quality |

This is the Anthropic "accuracy + runtime + tool calls + token consumption + tool errors" set (https://www.anthropic.com/engineering/writing-tools-for-agents) plus Aider's per-attempt and well-formed stats (https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md), reshaped so each column is owned by a specific harness feature — the precondition for the ablation rule.

### 2.5 Ablation support

**Per-scaffold-feature config flags.** tau's config is already TOML with section-level layering (system `~/.config/tau/` under project `.tau/`, `crates/tau-core/src/config.rs`). Each scaffold feature (e.g. a compaction strategy, a tool-batching policy, a self-verification loop, the bash error-enrichment) is therefore already a config key. The rig adds nothing new here: an ablation is "run the same task × model with config key K on vs off." Because layering is section-level, the rig can inject a per-run override file that shadows one key — the same mechanism a project `.tau/config.toml` already uses — so a matrix cell is just a synthetic config.

**Matrix runner.** A `tau-eval matrix` subcommand (and a `justfile` target) that takes a feature list and a model list and expands their Cartesian product into trials:

```
just eval-matrix FEATURE=om_compaction MODELS=mock,anthic/opus
# → cells: {feature:on, feature:off} × {model_a, model_b}, each = a run of the task set
# → emits a per-cell table: pass rate, tokens, cost, turns, rework, gate-fail
```

This is the operational form of the standing discipline "every harness feature is a hypothesis; no delta on model upgrade → delete": the matrix produces the on/off delta *on the current model*, and re-running the same matrix after a model upgrade shows whether the feature still buys anything. It is DeepSWE's fixed-harness comparison and OpenAI's "establish a baseline, then swap" (https://deepswe.datacurve.ai/blog/deepswe, https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf) applied to the *harness* axis instead of the model axis. A held-out task subset (Anthropic's overfitting guard, https://www.anthropic.com/engineering/writing-tools-for-agents) is reserved for confirmatory runs so a feature isn't tuned to the visible tasks.

### 2.6 CI integration and cost controls

**Deterministic leg (PR).** Runs in the existing `nextest` job via a `tau-eval` test target — no new CI surface, no API keys, no cost. A red invariant blocks the merge. This is the only leg that touches PRs.

**Live leg (on-demand, cost-capped).** A specifically invocable experiment (`just eval-live`) that:
- loads the user's tau config via the app's own `config::load` (system + project layering) — the benchmark measures the harness under the user's real model configuration; no rig-specific endpoint env,
- runs a bounded task subset (`--slice`/`--filter`-style selection, mini-swe-agent — https://mini-swe-agent.com/latest/usage/swebench/) against 1–2 models,
- is **cost-capped** three ways: a hard `budget_usd` for the run (runner stops scheduling new trials and reports partial results when hit), a per-trial `[agent] timeout_sec` and token cap (Harbor's per-component timeouts — https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/CONTRIBUTING.md), and a max-trials bound. The cap makes a runaway loop (the TB worst case of ~2h / ~100M tokens on one task — https://arxiv.org/html/2601.11868v1 §4.1) a bounded, reported event instead of an open-ended charge.
- posts the per-cell metric table as a CI artifact/comment. Nightly, not per-PR, because it is non-deterministic and paid — the same reason no external system gates merges on a live benchmark.

### 2.7 Terminal-Bench task import

**License (verified from the repo).** The `harbor-framework/terminal-bench` repo is **Apache-2.0** (https://api.github.com/repos/harbor-framework/terminal-bench → `license.spdx_id = "Apache-2.0"`; https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/LICENSE). Individual contributions carry the same grant — e.g. the `atrx-vep-crispr` task's `LICENSE.md` is an explicit "license … under the terms of the Apache License, Version 2.0" from Scale AI (https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/tasks/atrx-vep-crispr/LICENSE.md). Apache-2.0 is permissive: it permits copying tasks into tau **provided** the license notice, copyright, and attribution are retained in each imported file. (tau itself is AGPL-3.0; importing Apache-2.0 *into* an AGPL codebase is license-compatible — the combined work is distributed under AGPL — so there is no license conflict in the import direction.)

**Import proposal (the full set, transformed to the local format).** Import the **complete Terminal-Bench 2.0 set — 89 curated tasks, the latest release** (`harbor-framework/terminal-bench-2`, schema v1.1) — so the pass rate is anchored to the full external distribution:
- **Adapt** each task to its environment class, mechanically from the `task.toml` fields: CPU-only tasks whose `environment/Dockerfile` is a plain base image + `apt`/package installs + copied data files become a temp-dir `setup.sh` (the default path); the `gpus = 0` and modest `cpus`/`memory_mb` fields make the filter mechanical — https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/CONTRIBUTING.md.
- **Transform** each task directory: `instruction.md` → `instruction.md` (verbatim); `environment/Dockerfile` → `setup.sh` that reproduces the image's userland state (package installs + data files) in the temp dir; `tests/test.sh` (+ `tests/` helpers) → `check.sh`; `solution/solve.sh` → `oracle/solve.sh`; `task.toml` timeouts/resources → `task.toml`. The outcome-driven verifier semantics carry over directly (tests check final state, not the agent's commands — https://arxiv.org/html/2601.11868v1).
- **Preserve** the canary string and the Apache-2.0 `LICENSE`/notice in every imported file (the canary is in "each file in our repository" for training-corpus decontamination — https://arxiv.org/html/2601.11868v1 §5; keeping it is both required by the spirit of the dataset and harmless to eval use).
- **Route** tasks that require GPU, multi-container topologies, long-running training, or network-proprietary services to the per-task opt-in `environment = "docker"` path (docker per-task for reproducibility, not a global default) — they stay in the set, marked, and run only where docker is available.
- **Verify** each imported task with the oracle/nop gates (oracle passes, nop fails) before it joins the live-leg set — the same acceptance bar TB applies (https://arxiv.org/html/2601.11868v1 Appendix B).

This gives the live leg a realistic, externally-anchored task distribution (the thing DeepSWE/Aider/SWE-bench all use to make pass rates mean something) while keeping the deterministic leg fully self-authored.

### 2.8 First slice

**Ship first: the deterministic leg, end to end, with the shared runner.** Concretely:
1. `tau-eval` crate: the runner (§2.2) — temp-dir workspace, in-process `tau-core` drive against the mock, `check.sh` scoring, per-trial artifact capture (session JSONL + tokens + wall time + `results.json` + run checkpoint).
2. The task format (§2.1) with **2–3 hand-written tasks** that exercise the core tools and one that exercises a known scaffold feature.
3. A `#[cfg(test)]` / nextest target running the **deterministic leg** (§2.3 leg 1): known-good mock scenario → assert protocol well-formedness, session integrity, and tool dispatch; a red blocks the PR.
4. A `just eval` target wiring it into the existing `justfile`.

**Rationale.** This slice is the minimal rig that is *already useful and already CI-gating*: it validates the entire shared plumbing (task format, temp-dir isolation, in-process drive, scoring, artifact capture) at **zero model cost and zero flakiness**, because the model is the deterministic mock — the same property that makes tau's existing acceptance suites "a red is a code problem, never a network problem." Every later piece layers onto it without rework: the live leg reuses the runner with a real endpoint + cost cap; the ablation matrix reuses the config-override mechanism; the TB import reuses the task format and the oracle/nop gates. Building the live leg or the matrix first would mean building the expensive, non-deterministic, non-gating half before the cheap, deterministic, gating half that de-risks it — inverting the baseline-first discipline both Anthropic and OpenAI prescribe (https://www.anthropic.com/engineering/multi-agent-research-system, https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf).

---

## Sources

Fetched this session:
- Terminal-Bench 2.0 paper: https://arxiv.org/abs/2601.11868 · https://arxiv.org/html/2601.11868v1
- Harbor / Terminal-Bench repo: https://github.com/harbor-framework/terminal-bench · https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/CONTRIBUTING.md · https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/LICENSE · https://raw.githubusercontent.com/harbor-framework/terminal-bench/main/tasks/atrx-vep-crispr/LICENSE.md · license via https://api.github.com/repos/harbor-framework/terminal-bench
- Harbor docs: https://harborframework.com/docs/task-format · https://harborframework.com/docs/running-tbench · https://harborframework.com/docs/agents
- ATIF spec: https://github.com/harbor-framework/harbor/blob/main/rfcs/0001-trajectory-format.md · agent interface: https://raw.githubusercontent.com/harbor-framework/harbor/main/src/harbor/agents/base.py · mini-swe-agent adapter: https://raw.githubusercontent.com/harbor-framework/harbor/main/src/harbor/agents/installed/mini_swe_agent.py
- SWE-bench: https://github.com/swe-bench/swe-bench · https://arxiv.org/abs/2310.06770 · https://arxiv.org/html/2310.06770 · https://swebench.com/SWE-bench/guides/evaluation/
- mini-swe-agent: https://raw.githubusercontent.com/SWE-agent/Mini-SWE-Agent/main/README.md · https://mini-swe-agent.com/latest/usage/swebench/
- DeepSWE: https://deepswe.datacurve.ai/ · https://deepswe.datacurve.ai/blog/deepswe · https://deepswe.datacurve.ai/blog/deepswe-v1-1
- Aider polyglot: https://aider.chat/2024/12/21/polyglot.html · https://github.com/Aider-AI/polyglot-benchmark · https://raw.githubusercontent.com/Aider-AI/aider/main/benchmark/README.md · https://aider.chat/docs/leaderboards/
- Anthropic: https://www.anthropic.com/engineering/multi-agent-research-system · https://www.anthropic.com/engineering/writing-tools-for-agents
- OpenAI: https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf
- Original t-bench: https://github.com/harbor-framework/terminal-bench-1 · https://api.github.com/repos/harbor-framework/terminal-bench-1/contents/original-tasks/adaptive-rejection-sampler
- OpenHands: https://github.com/OpenHands/benchmarks
- Trajectory datasets (checked for the §1.7 split): https://huggingface.co/datasets/yoonholee/terminalbench-trajectories · https://huggingface.co/datasets/harborframework/terminal-bench-2.0

tau repo (verified in-tree): `Cargo.toml` (workspace members, AGPL-3.0) · `crates/tau-core/Cargo.toml` (no Tauri dep) · `crates/tau-mock-llm/Cargo.toml` · `fixtures/e2e-mocks/` · `justfile` (`test` → nextest; mock-LLM acceptance) · `crates/tau-core/src/config.rs` (TOML layering) · `crates/tau-protocol/src/snapshot.rs` (`Workspace{id,name,cwd}`) · `crates/tau-core/src/session.rs` + `subagent.rs` (`.tau/sessions/` JSONL) · `crates/tau-test/src/main.rs` (in-process drive, mock-only).
