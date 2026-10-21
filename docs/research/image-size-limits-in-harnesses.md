# Research: how other agent harnesses handle per-provider image size limits

Researched 2026-10-21. The goal: before we design Tau's per-endpoint
`max_image_pixels` knob + downscale-in-read-tool fix (+ a possible
provider/model preset table), understand what the field actually does in the
wild. We convert images to base64 JPEG data URLs and send them as `input_image`
parts in OpenAI-Responses-style requests, and we found that a vLLM server
**silently resamples** images above a pixel budget (2,097,152 px = 2048 tokens
× 1024 px/token), degrading vision quality with no error.

For each harness we ask: (a) what size limits it enforces client-side,
(b) where those limits are configured, (c) how it handles the gap between a
provider limit and an oversized client image (refuse / downscale / pass
through), and (d) whether it ships a per-model/per-provider preset table.

## Method

Cloned the open-source harnesses at `main` and read the image path directly; fetched the provider docs. Closed-source tools (Claude Code, Cursor, Windsurf) are assessed from public repos/docs only and marked as such. Every claim cites its source.

## Findings per source

### Anthropic API (the provider contract, not a harness)

The Claude API documents image limits as a set of **server-side** rules
(https://docs.claude.com/en/docs/build-with-claude/vision):

- Max dimensions per image: **8000×8000 px**. Above 20 images in one request a
  stricter per-image dimension limit applies to every image, and images beyond
  it are **rejected with an `invalid_request_error`**.
- Max size per image (base64): **10 MB** on the Claude API, **5 MB** on
  Bedrock/Vertex, 10 MB on claude.ai. Request size cap **32 MB** (standard
  endpoints).
- **Resolution tiers** (this is the per-model table on the provider side):
  *High-resolution* (Claude 4.7+) = max long edge **2576 px**, 4784 visual
  tokens; *Standard* (all other models) = max long edge **1568 px**, 1568
  tokens.
- Crucially, oversized images are **downscaled by default**: "When an image is
  downsized, Claude scales it to the largest size that fits the tier's limits
  while preserving its aspect ratio." And: "To have any other oversized image
  **rejected with an error instead of downscaled**, set the image block's
  `transformations` field."

So the provider's default is *silent server-side downscale*, with an opt-in to
*reject*. This is the exact behavior class our vLLM server exhibits.

### Claude Code (Anthropic) — closed source

`anthropics/claude-code` (https://github.com/anthropics/claude-code) ships
**no client image-handling source** — the repo is docs, plugins, examples, and
a changelog (1369 TS/JS/Py files, none on the image send path; no
`image`/`screenshot`/`vision` paths). It is a thin client for the Anthropic
API, so it inherits the provider's **default server-side downscale** with no
client-side resize, byte cap, or per-model table of its own. We cannot
inspect its internal image path from source.

### Aider

Pass-through with a **token-cost estimate only** — no resize of the image that
is sent.

- `get_images_message` base64-encodes the file **as-is** and sends it with a
  hardcoded `"detail": "high"` (
  https://github.com/Aider-AI/aider/blob/main/aider/coders/base_coder.py#L817).
  No pixel/byte/dimension cap.
- `token_count_for_image` (
  https://github.com/Aider-AI/aider/blob/main/aider/models.py#L672) *does*
  scale dimensions — to a 2048 px max edge, then 768 px short side, then
  512-px tiles × 170 tokens + 85 — but **only to estimate the token cost** for
  context budgeting. The scaling never touches the payload that goes on the
  wire.
- Vision is gated by a per-model **`supports_vision` boolean** from its
  `models.json` — a capability flag, not a size table.

So Aider: **pass through**; the only size logic is a client-side token
*estimate*.

### OpenHands (All Hands AI)

Pure **pass-through**, no client-side limits. The attachment path is a
`FileReader.readAsDataURL` with no resize and no cap
(https://github.com/All-Hands-AI/OpenHands/blob/main/src/utils/convert-image-to-base-64.ts).
`send-message-with-attachments.ts` embeds that data URL into the LLM call.
No pixel/byte/dimension handling anywhere in the image path (the only
"resize" hits in the repo are UI `ResizeObserver`s).

### SWE-agent

**Pass-through** for the send, plus a **context-compression** path that drops
images. `ImageParsingHistoryProcessor` parses embedded `data:...;base64`
markdown into `image_url` parts verbatim — no resize
(https://github.com/SWE-agent/SWE-agent/blob/main/sweagent/agent/history_processors.py#L340).
A separate history processor replaces old observations with a placeholder that
counts omitted images ("N images omitted") — that is *token* budgeting, not
size handling. No PIL/resize on the send path.

### Cline (VS Code agent) — the clearest **refuse** example

Cline enforces **byte-based** limits client-side and **refuses** (omits)
oversized images — it does **not** downscale.

- Defaults are hardcoded in the shared SDK (
  https://github.com/cline/cline/blob/main/sdk/packages/shared/src/llms/media.ts#L80):
  `DEFAULT_MAX_IMAGE_BASE64_BYTES = 5 MiB` (encoded),
  `DEFAULT_MAX_IMAGE_DECODED_BYTES = 6 MiB`,
  `DEFAULT_MAX_TOTAL_MEDIA_BYTES = 8 MiB`.
- `reserveImageMediaBytes` returns a typed failure
  (`encoded_limit` / `decoded_limit` / `total_limit`) and calls
  `recordOmittedImage` — the image is **dropped**, not resized.
- The limits are **configurable via `MediaBudgetOptions`** params (per-call
  overrides) but there is **no per-endpoint or per-model table** — they are
  global byte constants.
- The user-facing surface mirrors this as a hard refuse: "Each image must be
  3.75 MB or smaller", "Attachments must be 6 MB or smaller in total", max 5
  images (
  https://github.com/cline/cline/blob/main/apps/examples/desktop-app/webview/lib/image-attachments.ts).
- No `sharp`/`canvas`/`drawImage` in the media pipeline — confirming refuse,
  not downscale.

### Continue (continue.dev) — the clearest **downscale** example

Continue is the one harness that **downscales client-side**, and it also keeps
a byte refuse-gate.

- `IMAGE_RESOLUTION = 1024`; `getDataUrlForFile` resizes to fit a 1024×1024
  box (`scale = min(1024/w, 1024/h)`) via a canvas, then
  `canvas.toDataURL("image/jpeg", 0.7)` (
  https://github.com/continuedev/continue/blob/main/gui/src/components/mainInput/TipTapEditor/utils/imageUtils.ts#L3).
- `handleImageFile` **refuses** anything not jpg/png/gif/svg/webp **or ≥ 10
  MB** with a toast ("Images need to be in jpg or png format and less than
  10MB in size").
- The 1024 px target and 10 MB cap are **hardcoded constants**, not per-endpoint
  or per-model. The core LLM adapters (e.g. `Anthropic.ts`) just re-encode the
  data URL and pass it through.

So Continue: **downscale to a fixed 1024 px** + **refuse above 10 MB**, both
global constants.

### Cursor / Windsurf — closed source

No public source or documented client-side image pipeline. Both are
proprietary; we treat their image handling as **unknown** and exclude them from
the pattern count. (Note only — no primary source to cite.)

### LangChain

**Pass-through; its image utilities were removed.** `langchain_core/utils/image.py`
(https://github.com/langchain-ai/langchain/blob/main/libs/core/langchain_core/utils/image.py)
now raises for `encode_image`/`image_to_data_url`: "removed for security
reasons." The image document loader is a deprecated shim to
`UnstructuredImageLoader`. No pixel/byte caps in the core — whatever image you
hand it is passed to the chat model untouched.

### LlamaIndex

**Pass-through, no cap.** `img_2_b64` (
https://github.com/run-llama/llama_index/blob/main/llama-index-core/llama_index/core/img_utils.py#L11)
is a bare PIL→JPEG→base64 with no size limit; `ImageReader` converts to RGB
and base64-encodes with no resize. No client-side pixel/byte enforcement.

### OpenAI (the provider contract — the reference preset table)

OpenAI documents image limits as **server-side resizing rules + a per-model
sizing table** (https://platform.openai.com/docs/guides/images):

- The `detail` param (`low` / `high` / `original` / `auto`, default `auto`)
  controls **server-side** preprocessing. For `high`/`auto`: "Scale down to
  fit in a 2048px × 2048px square, maintaining aspect ratio. Smaller images
  are not enlarged. If the shortest side exceeds 768px, scale it down to
  768px… Count the 512px squares needed to cover the image. Each square uses
  the model's tile tokens."
- File types: PNG, JPEG, WEBP, non-animated GIF. **Request size: up to 512 MB
  total payload** per request; **up to 1,500 images** per request; up to
  30,000 patches per image.
- **Per-model table** (the table we'd mirror): e.g. `gpt-5.1 / gpt-4.1 /
  gpt-4o / gpt-4o-mini` support `low/high/auto` (low = fixed token count;
  high/auto = tile-based rules); the `gpt-5.4` family adds `original` with a
  6000-px max dimension and a 10,000-patch budget. This is a genuine
  per-model preset table — but it lives in **OpenAI's docs**, not in any
  harness's source.

### vLLM (the server that silently resamples)

The pixel budget is **per-model** and the resample is **not in the user
docs** — it is emergent from each model's HF image processor.

- The one documented knob is a **count** limit, not a pixel budget:
  `--limit-mm-per-prompt.image N` (
  https://github.com/vllm-project/vllm/blob/main/docs/features/multimodal_inputs.md#L656).
  The `features/multimodal_inputs.md` doc contains **zero** mentions of
  resample/downscale/pixel budget.
- The actual pixel budget is `min_pixels`/`max_pixels` read off each model's
  image processor (e.g. `llava_onevision2.py` defaults `max_pixels` to
  1,605,632), and `smart_resize` (imported from
  `transformers…/image_processing_qwen2_vl`) resizes the image to fit
  `[min_pixels, max_pixels]` (
  https://github.com/vllm-project/vllm/blob/main/vllm/model_executor/models/llava_onevision2.py#L1355).
  This is the silent resample we hit: it is per-model, server-side, and
  undocumented at the API level.

## Synthesis

**Is per-endpoint config the dominant pattern? No.** The dominant pattern is
**pass-through**: Aider, OpenHands, SWE-agent, LangChain, LlamaIndex, and
Claude Code all send the image bytes as-is and rely on the *provider's*
server-side behavior (which, for Anthropic and OpenAI, is itself a silent
downscale). The two harnesses that act client-side at all — Cline and Continue
— use **hardcoded global constants**, not per-endpoint or per-model config.
No surveyed harness keys its limit to the specific endpoint/model it is
talking to. Per-endpoint `max_image_pixels` is therefore *more* sophisticated
than anything in the field, not a copy of an existing pattern.

**Do any harnesses ship a per-model preset table in source? No.** The per-model
image tables exist only on the **provider** side (OpenAI's detail/patch table,
Anthropic's resolution tiers) — never in a harness's code. The closest a
harness gets is Aider's per-model `supports_vision` boolean, which is a
capability flag, not a size table. A per-model preset table in Tau's source
would be novel; if we add one, the reference tables to mirror are OpenAI's
(per-model detail/patch) and Anthropic's (per-model resolution tier), and the
vLLM `max_pixels` per model.

**Do any downscale client-side vs refuse? Both, and they're the two outliers.**
Continue **downscales** (to a fixed 1024 px JPEG) and is the only true
client-side resizer. Cline **refuses** (omits over-budget images by bytes) and
never resizes. Everyone else passes through. Notably, the *provider* default
behind all of them is downscale (Anthropic, OpenAI, vLLM all silently
resample), with only Anthropic offering an opt-in to reject.

**What this means for Tau's field.** The vLLM resample we hit is the same
behavior class as the Anthropic/OpenAI server-side downscale — a per-model
pixel budget the client cannot see. Two design conclusions follow:

- A **global** constant (Cline's byte cap, Continue's 1024 px) is the weak
  pattern: it does not track the server's actual per-model budget, so it either
  over-restricts small-budget models or under-protects large ones. Tau's
  per-endpoint `max_image_pixels` is better *if* it can be set per
  endpoint/model to match that model's `max_pixels`.
- **Downscale-in-read-tool** is the Continue-aligned choice (preserve the image,
  adapt it to the budget) versus Cline's refuse (drop it). Since the server
  would resample anyway, downscaling to the budget on the client reproduces the
  server's result *with control over quality/aspect* and surfaces it explicitly
  — the right default. Refuse should be the opt-in (mirroring Anthropic's
  `transformations` reject), for the case where a degraded image is worse than
  none.

In short: the field is mostly pass-through, with two hardcoded-constant
outliers (one downscale, one refuse) and no per-endpoint or per-model tables.
Tau's per-endpoint `max_image_pixels` + client downscale + optional refuse is a
net-new, more correct combination — and the only place the per-model tables
actually live is the provider docs.
