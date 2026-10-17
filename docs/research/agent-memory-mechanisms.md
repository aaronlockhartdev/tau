# Persistent memory for coding agents — a synthesis (2025–2026)

**Status**: research notes (2026-10-06) · **Provenance**: primary sources — vendor engineering posts, arXiv papers, and the named open-source tools. Every claim carries its source.

This is one of the most active research fronts in agentic coding right now. The field has moved well past "dump a `CLAUDE.md` file into the prompt," and several genuinely novel mechanisms have emerged. Here is what the research supports, organized from the foundational insight up to the cutting edge — and what it means for tau.

## The core insight everyone has converged on

The single most important finding across the Anthropic C-compiler project, the SIx Harness operational record, and the "Code as Agent Harness" survey: **persistent state belongs on disk, not in the context window.**

The C-compiler project ([Anthropic, Feb 2026](https://www.anthropic.com/engineering/building-c-compiler)) ran 16 parallel Claude agents over nearly 2,000 Claude Code sessions and $20,000 in API costs, producing a 100,000-line Rust C compiler that builds Linux 6.9. Each agent "is dropped into a fresh container with no context," and the harness's central trick is instructing the model to "maintain extensive READMEs and progress files … updated frequently with the current status" — so every fresh session rebuilds its context from filesystem artifacts.

The SIx Harness record ([arXiv 2609.05510](https://arxiv.org/abs/2609.05510)) is the months-scale companion: one continuous Claude Code session line since January 2026 driving a 633,446-line codebase, its memory subsystem continuously instrumented (78,933 hook invocations; 85 recorded failures, none silent).

The survey ([Code as Agent Harness, arXiv 2605.18747](https://arxiv.org/abs/2605.18747)) states the general form: the harness infrastructure "persists and compacts state," while mutable task state — repository evidence, execution traces, validation results, memories — lives outside the model.

The context window is now understood as *working memory* for a single task, not a place to hoard knowledge. Cross-session knowledge is externalized to artifacts the agent re-loads on demand. This reframe is what makes everything below possible.

## The taxonomy

The four-tier memory model most of these systems separate — **working** (current task trajectory), **episodic** (raw session traces/tool calls), **semantic** (distilled codebase facts), **procedural** (reusable skills and validated approaches) — is the **CoALA framework** (Sumers et al., Princeton, 2023, [arXiv 2309.02455](https://arxiv.org/abs/2309.02455)). The 2026 harness survey organizes the same space by functional role (working / semantic / experiential / long-term / multi-agent, [arXiv 2605.18747 §3.2](https://arxiv.org/abs/2605.18747)). Production lineage runs through MemGPT/Letta's OS metaphor (core / archival / recall) and the 2025–2026 stack: Mem0, Zep, A-MEM, HippoRAG, MemOS.

Crucially, the model doesn't just accumulate; entries get *promoted* between tiers and *forgotten*.

## The genuinely novel mechanisms

**1. Agent-writable declarative memory.**
The `CLAUDE.md` / `AGENTS.md` pattern, with a twist: **the agent writes back to it autonomously.** Claude Code's auto-memory (introduced in **v2.1.59**, Feb 2026; [docs](https://code.claude.com/docs/en/memory)) lets the model save four typed kinds of notes — `user`, `feedback`, `project`, `reference` — to a `MEMORY.md` index (first 200 lines / 25 KB loaded every session) with topic files beyond it. It explicitly skips anything derivable from the codebase or already in `CLAUDE.md`, and sub-agents can keep their own. The file becomes a living procedural memory, not static config. The same pattern appears at Anthropic as "structured note-taking" in their [context-engineering guidance](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) (Oct 2025).

**2. Anti-recurrence stores (the most novel operational finding).**
SIx Harness introduces *anti-recurrence records* — capturing not just decisions made but **dead-ends hit**, so the agent doesn't re-walk a failed path. Their instrumented record: 154 captured decisions + 186 captured dead-ends, triaged down to 59/47 *confirmed* entries, with a pending queue and operator triage (confirm / reject / retitle) between. The reject rate is the point: "an anti-recurrence store that ingests everything becomes the noise channel." The motivating incident is instructive: three consecutive sessions each re-proposed building a subsystem that already existed dormant in the codebase, *with the design documentation injected into context each time* — "retrieved-but-unused" is now an isolated benchmark failure sub-mode ([Mem2Act 2026], cited in SIx). The fix was an inventory document plus a `DO-NOT-REBUILD` injection, not better retrieval.

**3. Temporal knowledge graphs (solving the "stale memory" problem).**
Zep's **Graphiti** (bi-temporal) and **Cognee** ([`SearchType.TEMPORAL`](https://docs.cognee.ai/guides/temporal-knowledge-graph)) encode *when* a fact was true, not just *what* it is. A flat vector store can't tell you an API signature changed last month; a temporal graph can. This matters enormously for codebases where facts decay — and a whole 2026 benchmark wave now measures it: staleness/validity suites (STALE, ATMA, TemporalValidity), a dual metric penalizing both recall failure *and* reuse of invalidated memory (FAMA), and bi-temporal ledgers that retire stale values with no similarity threshold ([arXiv 2606.26511](https://arxiv.org/pdf/2606.26511)). State of the art: [Mem0's 2026 report](https://mem0.ai/blog/state-of-ai-agent-memory-2026) shows the largest algorithmic gains on *temporal* queries (+29.6) and multi-hop (+23.1), with staleness in high-relevance memories the named open problem.

**4. "Compaction-surviving" resume anchors.**
Because months-long session lines cross compaction boundaries constantly — SIx recorded 90 crossings in its instrumented final two months alone, "a rate implying a few hundred over the period" — the novel trick is a **one-line-per-memory index file with a resume anchor that gets re-read after every compaction**, plus first-class "invariant" files that are re-injected at boundaries. SIx principle 6 states it plainly: *"Continuity must be an artifact, not a habit. Anything that must survive compaction gets a file the workflow forcibly revisits: resume anchors read first, invariants re-read at boundaries, append-only ledgers whose tails are read on resume."* Compaction is a routine event to design around, not a data-loss accident. There is now a serving-side line too: [Parallel Context Compaction](https://arxiv.org/abs/2605.23296) (Penn State, 2026) treats compaction throughput as a first-class systems problem.

**5. Git-as-memory (version-controlled memory).**
**GCC (Git-Context-Controller)** ([arXiv 2508.00031](https://arxiv.org/abs/2508.00031), Aug 2025) treats agent context as a persistent, navigable memory workspace with explicit `COMMIT`, `BRANCH`, `MERGE`, and `CONTEXT` operations — memory auditable, diffable, and reviewable like the code it describes. Related: **Codified Context** ([arXiv 2602.20478](https://arxiv.org/abs/2602.20478), Feb 2026) builds a three-component infrastructure on a 108,000-line C# system: a hot-memory constitution (conventions, retrieval hooks, orchestration protocols), 19 specialized domain-expert agents, and tiered machine-readable specs as inter-session coordination documents. The same codified-context pattern shows up in Google's Conductor for Gemini CLI.

**6. Reinforcement-learning-driven memory (the true frontier).**
**MemRL** ([arXiv 2601.03192](https://arxiv.org/abs/2601.03192), Jan 2026) and **MemEvolve** ([arXiv 2512.18746](https://arxiv.org/abs/2512.18746), Dec 2025, ICML'26) close the loop that flat systems leave open: the agent learns *what* to remember/forget from outcome signals (non-parametric RL over episodic memory with value-aware retrieval and utility-driven curation), and MemEvolve even evolves the memory *architecture* itself — jointly evolving memory content and management strategy. This is the difference between "retrieve-and-use" (what production tools do) and "learn-and-update."

**7. Skill curation as procedural memory.**
**Voyager** ([arXiv 2305.16291](https://arxiv.org/abs/2305.16291)) is the canonical form: the agent generates code-based skills, validates them by execution, and stores verified skills in a persistent library indexed by natural-language descriptions. **Cradle** ([arXiv 2403.03186](https://arxiv.org/abs/2403.03186)) extends it with explicit memory management linking skills to episodic context. The 2026 SoK on agentic skills ([arXiv 2602.20867](https://arxiv.org/abs/2602.20867)) anchors the "skills are the new npm" movement academically. Skills (e.g. `SKILL.md`) are now seen as the right home for procedures, keeping the main context lean.

## The counter-evidence

Before the retrieval and caveats sections, the counter-weight — because it constrains how much the curation machinery above is actually worth:

- **Guardrails Beat Guidance** ([arXiv 2604.11088](https://arxiv.org/abs/2604.11088), Apr 2026). The first large-scale controlled study of rule files: 679 files, 25,532 rules, 5,000+ Claude Code runs (Opus 4.6) on SWE-bench Verified. Headline: **random rules improve task performance as much as expert-curated ones** (both +13.8pp on a discriminative subset). Gains are largely *content-independent* — random, shuffled, and mismatched-domain rule files all match curated ones, pointing to a context-priming mechanism. And every *individually beneficial* rule is a **negative constraint** ("do not refactor unrelated code") while every individually harmful one is a positive directive ("follow code style"). The operational principle: *constrain what the agent must not do, rather than prescribing what it should.*
- **Written by AI, Managed by AI** ([arXiv 2606.19121](https://arxiv.org/abs/2606.19121), 2026). 391 consecutive LLM sessions on one real project. Names **"Index Sickness"**: *accumulating* symbolic rules in standing prompts degraded long-project performance; the working fix was a **minimal baseline log** — a compaction-surviving artifact, not more standing constraint.
- **Evaluating AGENTS.md** ([arXiv 2602.11988](https://arxiv.org/abs/2602.11988), ETH SRI Lab, 2026). "Instructions in the context files are well followed by coding agents; repository overviews, although popular and recommended by model providers, are not helpful."
- **Agent READMEs** ([arXiv 2511.12884](https://arxiv.org/abs/2511.12884), v2 Aug 2026) — the first large-scale empirical study of 2,303 agent context files from 1,925 repos — confirms the context file is now a studied artifact genre with measurable maintenance dynamics.

Read together: the static context-file tier is a *priming and constraint* surface, best kept short, instruction-focused, and negatively phrased — and the elaborate curation tiers earn their keep only where they prevent *repeated concrete errors*, not where they accumulate general guidance.

## What retrieval looks like now

The research consensus is **hybrid retrieval with RRF fusion**: BM25 (exact) + vector (semantic) + graph (structural). Notably, there's a strong contrarian current arguing that **deterministic/exact token matching beats fuzzy embeddings for code conventions**:

- **SIx's "boring storage, honest degradation"** stance: one SQLite file per project holding an FTS5 (BM25) index and a sqlite-vec table, fully local; if the embedding leg fails, search silently degrades to BM25-only. They explicitly did *not* adopt a graph store, an OS-metaphor memory manager, or a temporal knowledge graph: "at single-project scale, our failures were never 'the data model was insufficiently expressive' and always 'some part of the pipeline was quietly wrong.'"
- **deja-vu** ([github.com/vshulcz/deja-vu](https://github.com/vshulcz/deja-vu)): a single Go binary that inverts the session logs coding agents already write — "no model, no embeddings, no server" — answering searches in 7–9 ms over gigabytes of history. **Cass** (Rust) takes the same corpus with optional embeddings. The product-level lesson: the session logs on disk *are* the memory corpus.

## The honest caveats the research keeps flagging

- **The compounding gap is unsolved.** Even with `CLAUDE.md` + memory files, practitioners report the agent *doesn't measurably improve* at a codebase over time. Databricks frames the open question precisely — *memory scaling*: "does the agent get better as it accumulates more information?" ([Memory scaling for AI agents, Apr 2026](https://www.databricks.com/blog/memory-scaling-ai-agents)). Most systems retrieve; few genuinely learn.
- **Forgetting is the underrated operation.** That same Databricks study showed agents retrieving from memory of *earlier incorrect runs* reused the wrong answers with *more* confidence — "memory had given the wrong answer the appearance of established precedent."
- **Memory is now a supply-chain risk.** **AgentPoison** ([arXiv 2407.12784](https://arxiv.org/abs/2407.12784), NeurIPS 2024) demonstrated the first backdoor attack on agents via poisoned long-term memory/RAG knowledge bases; **MINJA** (2025) extended it to memory-injection jailbreaks. A poisoned entry retrieved as "trusted context" in a future session can override within-session robustness — stale/hallucinated memories propagate like bad `node_modules` dependencies. The attack surface is live in shipping tools: Claude Code's changelog records neutralizing "invisible characters and tags that imitate Claude Code's own markup" in `MEMORY.md` and recalled notes before they reach the model.
- **The contrarian baseline is strong.** A flat `MEMORY.md` (read at session start, appended at session end) covers the large majority of cases — and the strongest operational record in the field (SIx) is a deliberately boring SQLite + FTS5 setup, while the nearest-neighbor months-long study (WrittenByAI) found that *heavier* standing scaffolding degraded performance. The elaborate tiers above are justified by named, repeated failure modes — not by architecture.

## Practical takeaway

The research-supported ladder, with the evaluation rung the field itself admits it's missing:

0. **Measure before you build.** The field's own confession (SIx §6: "N=1, no control arm, self-reported") is that almost nobody has a controlled measurement of whether memory helps. A repeated-task suite with memory **on vs. off** as the control arm — SIx's pre-registered A1 ablation (injection off for 14 days; *re-proposal incidents* mechanically defined) is a turnkey design — is the cheapest way to find out whether your rung 1 is even working.
1. **Start flat**: a `MEMORY.md`/`AGENTS.md` you (and the agent) read/append each session — captures conventions, architecture, naming, decisions. Keep it short, instruction-focused, negatively phrased; no auto-generated repository overviews.
2. **Add structure**: tier it (facts vs. traces vs. reusable skills), and capture *dead-ends*, not just wins — with a triage gate between capture and confirmation.
3. **Add retrieval**: start lexical — BM25/FTS5 over the logs already on disk; add the vector leg only with a named query the flat index fails.
4. **Add time + safety** only when you hit concrete, nameable retrieval failures: temporal graph for staleness, curation/decay and write-side provenance for poison prevention.

The moat, per the research, is **not** the model and **not** the prompt — it's reliable cross-session memory with low-latency retrieval *and eval coverage*. The teams that get the *simplest* version passing their evals win; the elaborate graph-DB infrastructure is largely premature until you can name the exact query your flat file fails on.

## Application to tau

### What tau already embodies

| Research mechanism | Tau's mechanism | Source |
|---|---|---|
| Persistent state on disk (the core insight) | Sessions as append-only JSONL (per-line CRC, sidecar blobs, manual archives); plan files at `.tau/plans/{session-id}.md`; `OmRecord` stored in the session file | ADR-0005; spec §3, §5.3 |
| Compaction-surviving anchors (SIx principle 6) | Plan file **re-injected at each turn boundary and after compaction**; **resume contract** re-injected on compaction; OM log + recent raw window as the standing context shape | spec §4, §5.3; CONTEXT.md |
| Episodic memory, recoverable on demand | Raw entries never deleted; `recall` pages from an observation group's range back to raw entries (browse-only in v0) | ADR-0004 |
| Procedural memory as files | Agent types (`.md` + frontmatter, system/project hierarchy) and context files (`AGENTS.md`/`CLAUDE.md` walked up from cwd, per-directory override) | spec §5.5, §10 |
| Compaction as first-class, measured infrastructure | OM (Observer/Reflector, async buffered, per-branch records); spec §4's standing mandate to measure "rework rate / token spend / cache-hit rate before and after"; `accept-om` suite | ADR-0004; ADR-0010 |
| Precision over recall for injected context (SIx principle 3) | Context files are the *only* always-injected memory; nothing else is auto-injected | spec §10 |

In research terms, tau is already at ladder rung 1 *with* rung-2 hygiene: every continuity-critical artifact is a file the workflow forcibly revisits, and compaction is a measured, acceptance-tested event rather than a summarization accident. The OM/`recall` split is the Observational-Memory instantiation of "episodic memory on disk, distilled observations in context."

### The gaps, in research-justified priority order

**G1 — No cross-session project memory (the biggest gap).** OM is per-session; nothing distilled survives between sessions except the static, human-written context files. A post-v0 `.tau/memory/` project store, deliberately boring (one SQLite per project: FTS5 first, vector leg optional — SIx §3.1), with:
- an agent-writable surface (the auto-memory pattern: typed notes `user`/`feedback`/`project`/`reference`, index file capped ~200 lines, skip-what-the-code-already-says rule);
- **operator-triaged promotion** (candidates in a pending queue; confirm/reject/retitle before the confirmed record — the reject rate is the feature);
- load-time neutralization of markup (the auto-memory attack surface above).

**G2 — No anti-recurrence records.** Decisions + dead-ends with a triage queue is the field's most novel operational finding, and tau has the triage surface for free: the plan pane and the user are the operator. The **completion gate** is a ready-made capture point — a `gate_failed` with its failure output is a dead-end candidate; a gate pass is a decision candidate. This directly attacks SIx's motivating incident (three sessions rebuilding an existing dormant subsystem while its design docs sat in context). The cheap first version: an inventory / DO-NOT-REBUILD section in the plan file's template, injected with the plan — no new store required.

**G3 — Retrieval over what's already on disk.** ADR-0004's designed-in `search` mode for `recall` is the extension point. The contrarian evidence (deja-vu, SIx's BM25 leg, the "exact identifiers beat fuzzy semantics" thesis) says **start lexical**: FTS5 over the session JSONL corpus already on disk is a zero-schema-change, zero-new-infrastructure index — and deja-vu/Cass show it is a product surface users want ("search the session history already on disk"). The vector leg arrives only with a named query the flat index fails.

**G4 — The untrusted boundary must cover memory writes.** v0's one enforced boundary is marking sub-agent output untrusted before the parent's model reads it. The same rule must apply to memory writes: sub-agent writes to project memory are untrusted-by-default (or disallowed) — a poisoned child session is exactly the AgentPoison/MINJA threat model, arriving through the one channel v0 deliberately leaves open.

**G5 — Context-file discipline (no new mechanism; research-backed curation).** Three findings converge on how tau's context files should be *written*:
- *Evaluating AGENTS.md*: instructions are well followed; **repository overviews are not helpful** → do not seed auto-generated overviews into `AGENTS.md`;
- *Guardrails Beat Guidance*: beneficial rules are **negative constraints**; gains are content-independent priming → prefer "do not X" form; keep files short;
- *WrittenByAI*: standing symbolic constraint that *accumulates* in every prompt degrades long-project performance (Index Sickness) → context files must be curated, not appended-to blindly; a staleness/prompt-audit affordance (Claude Code's `/doctor prompt-audit` is a good model) earns its place.

**G6 — The eval rung the field lacks (tau's realistic differentiator).** The research's own confession is "N=1, no control arm, self-reported." Tau's acceptance discipline — mock-LLM-only, hash-pinned fixtures, CI-gated (ADR-0010) — is precisely what that field cannot do. A memory eval leg is cheap in this rig: a repeated-task fixture (same task, N consecutive sessions) with memory **on vs. off** as the control arm, measuring rework rate / rediscovery rate / token spend. SIx's pre-registered A1 protocol is a turnkey design to shrink to fixture scale. This is where "eval coverage" — the claimed moat — becomes concrete.

### What the research says not to build (yet)

- **Temporal knowledge graphs** (Graphiti/Cognee): SIx explicitly rejected them at project scale — "our failures were never 'the data model was insufficiently expressive' and always 'some part of the pipeline was quietly wrong'." Revisit only with a named staleness query the flat store fails.
- **RL-driven memory curation** (MemRL/MemEvolve): frontier research with benchmark results, no operational production evidence for a single-user harness.
- **Graph DB / OS-metaphor memory managers**: the "boring storage, honest degradation" verdict — "every component sophisticated enough to fail interestingly will."
- **A full skill-curation module**: skills are deliberately out of v0 scope, and the evidence (Guardrails' content-independence; WrittenByAI's accumulation penalty) says curation machinery is premature until a named retrieval failure exists. Agent types already cover the procedural tier.

### One-line synthesis for the roadmap

Tau already implements the research's foundational insight (state on disk, compaction as measured infrastructure, continuity as re-injected artifacts); the post-v0 memory work is **G1–G3 in that order** (a boring triaged project store, anti-recurrence records captured at the completion gate, lexical `recall` search), with **G4** as a hard invariant on the untrusted boundary, **G5** as writing-discipline for context files, and **G6** as the moat — the only rung the rest of the field has admitted it cannot build.

---

## Sources

- Anthropic: [Building a C compiler with a team of parallel Claudes](https://www.anthropic.com/engineering/building-c-compiler) (Feb 2026) · [Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) (Oct 2025)
- [Memory as Infrastructure (SIx Harness), arXiv 2609.05510](https://arxiv.org/abs/2609.05510) · kit: [mike-m6online/SIx_Harness](https://github.com/mike-m6online/SIx_Harness)
- [Code as Agent Harness, arXiv 2605.18747](https://arxiv.org/abs/2605.18747) · [CoALA, arXiv 2309.02455](https://arxiv.org/abs/2309.02455)
- Claude Code: [memory docs](https://code.claude.com/docs/en/memory) · [changelog](https://code.claude.com/docs/en/changelog) (auto-memory introduced v2.1.59, 2026-02-26)
- [Git-Context-Controller, arXiv 2508.00031](https://arxiv.org/abs/2508.00031) · [Codified Context, arXiv 2602.20478](https://arxiv.org/abs/2602.20478)
- [MemRL, arXiv 2601.03192](https://arxiv.org/abs/2601.03192) · [MemEvolve, arXiv 2512.18746](https://arxiv.org/abs/2512.18746)
- [Voyager, arXiv 2305.16291](https://arxiv.org/abs/2305.16291) · [Cradle, arXiv 2403.03186](https://arxiv.org/abs/2403.03186) · [SoK: Agentic Skills, arXiv 2602.20867](https://arxiv.org/abs/2602.20867)
- [Guardrails Beat Guidance, arXiv 2604.11088](https://arxiv.org/abs/2604.11088) · [Written by AI, Managed by AI, arXiv 2606.19121](https://arxiv.org/abs/2606.19121) · [Evaluating AGENTS.md, arXiv 2602.11988](https://arxiv.org/abs/2602.11988) · [Agent READMEs, arXiv 2511.12884](https://arxiv.org/abs/2511.12884)
- [Databricks, Memory scaling for AI agents](https://www.databricks.com/blog/memory-scaling-ai-agents) (Apr 2026) · [Mem0, State of AI Agent Memory 2026](https://mem0.ai/blog/state-of-ai-agent-memory-2026)
- [deja-vu](https://github.com/vshulcz/deja-vu) · [Cognee temporal KG docs](https://docs.cognee.ai/guides/temporal-knowledge-graph) · [Parallel Context Compaction, arXiv 2605.23296](https://arxiv.org/abs/2605.23296) · [Temporal Validity in Retrieval Memory, arXiv 2606.26511](https://arxiv.org/pdf/2606.26511)
- [AgentPoison, arXiv 2407.12784](https://arxiv.org/abs/2407.12784) (NeurIPS 2024)
- Repo: `CONTEXT.md` · `docs/spec/v0.md` (§3, §4, §5.3, §5.5, §10) · `docs/adr/0004` · `docs/adr/0005` · `docs/adr/0006` · ADR-0010
