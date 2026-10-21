# Tool results are two-channel: a model-facing `output` and a GUI-only `ui_detail`

**Status**: accepted (2026-10-20)

`ToolResult` carried a single `output` channel that went into the model's context *and* rendered in the GUI transcript. The SOTA edit tooling (`pi-hashline-edit-pro`) makes a hard model/UI split **on the edit response**: the model sees a one-line ack with line counts; the diff of its own change rides a UI-only `details`/`structuredContent` channel that never enters the context. Every successful edit in tau made the model re-read its own diff — real context cost over a long session, and the inverse of the pro fork's highest-scoring response contract.

**Decision**: add a second, UI-only channel to the tool result —

```rust
pub struct ToolResult {
    pub output: ToolOutput,          // model-facing (assembled into context)
    pub ui_detail: Option<ToolOutput>, // GUI-only; never assembled
    pub ok: bool,
    pub cancelled: bool,
}
```

- **Type**: `ui_detail` mirrors `ToolOutput` (reusing the enum, not a new `UiDetail` type), so a tool can carry an image to the UI and richer tools can later.
- **Wire**: optional field on the tool payload (`ToolPayload.ui_detail`, `skip_serializing_if`): a tool that does not split serializes it absent, and a session line written before the field existed deserializes to `None`.
- **The model-side guarantee is structural, not conventional**: the context-assembly path (`input_items` → `CallOutput::from_payload`) reads `payload["output"]` and *only* `payload["output"]`. A unit test pins both directions: the field is present on the wire entry (positive) and absent from the assembled input item (negative).
- **Success-only**: errors stay single-channel — the diagnostic *is* the model-facing output, and the GUI shows the same text with an error tint. `ui_detail` on an error adds nothing.
- **The channel removes model-authored artifacts from context, never information the model is acquiring.** `output` remains the channel for anything the model needs to act on; a tool uses `ui_detail` only for content the model already knows (it just wrote it) or has no further use for (the diff of a change it directed).

**First consumers** (the pro-fork parity): `edit` success → `Successfully edited <path>. Added N line(s), removed M line(s).` + demotion/warning notices to the model; the bounded before/after diff (with fresh anchors) to `ui_detail`. `write` → the #184 ack to the model; the written content to `ui_detail`. `read` → the full hash-anchored page to the model, **byte-identical to before**; an anchor-stripped twin (same rows and hints, `HASH│` prefixes dropped) to `ui_detail` — the strip happens in the core where the anchored rows are built, so the GUI needs no line-prefix parsing. `bash` does not split.

**Consequences**: the GUI card renders `ui_detail` in place of `output` when present (`toolOutput()` preference; `output` is the fallback), so the live and final cards of a split tool render from the same channel — in-flight streaming of a split tool (write) grows `ui_detail`, not `output`, and the coalesced-upserts-then-final-replaces behavior is unchanged. The TS mirror carries `ui_detail` on the tool entry. The diff view's `parseEditDiff` contract (`edited <path> (lines …)`) is unchanged: it now parses the `ui_detail` text. Out of scope and deferred: structured UI payloads (the pro fork's JSON `details` envelope, patch generation, the fidelity-hint engine) — `ui_detail` is `ToolOutput` (text today); a UI-only enrichment for `bash` (long-output collapse).
