# Mastra Observational Memory — port reference

Verbatim copies of the Mastra OM sources this repo's `tau-core::om` module
ports (spec §4, ADR-0004). Apache-2.0 — see `LICENSE.md` (the repo root
`package.json` declares `Apache-2.0`; these files sit outside the
separately-licensed `ee/` directory).

Pinned at `mastra-ai/mastra` commit `5e4ddacde7de44602c099223179a78f544206b0f`
(`main`, 2026-09-15):

- `constants.ts` — threshold defaults, observation-context and continuation prompts
- `observer-agent.ts` — extraction instructions, output format, guidelines, system prompt, output parsing
- `reflector-agent.ts` — reflector system prompt, compression ladder, prompt builder, output parsing
- `thresholds.ts` — dynamic threshold, retention floor, projected message removal
- `observation-groups.ts` — observation-group wrap/parse/reconcile (recall bookkeeping)
- `string-utils.ts` — `safeSlice` (surrogate-safe truncation used by the line sanitizer)

The Rust port keeps the prompt text verbatim (fidelity-tested against these
files); the Rust module is pure (no I/O), so `randomBytes(8)` group ids become
deterministic content hashes.
