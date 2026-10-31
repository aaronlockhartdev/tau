# Tau configuration reference

The complete key set of Tau's `config.toml`, as the code reads it. The schema
owner is `crates/tau-core/src/config/mod.rs`; the contract it settles is spec
§12 (key layout #35). Where a key is carried but has no consumer, this page says
so — a key that parses is not a key that does something.

## Files and layering

| Layer | Path | Read by |
|---|---|---|
| System | `~/.config/tau/config.toml` | `CoreBuilder::default_system()` (`harness/state.rs`) |
| Project | `{project root}/.tau/config.toml` | per workspace, on config lookup (`workspace_config`, `harness/state.rs`) |

- `HOME` must be set; without it there is no system dir and the core refuses to
  build (`home is not set; cannot locate the system config dir`).
- **Layering is section-level and wholesale.** A section present in the project
  file replaces that whole section from the system file — key-by-key merging
  never happens (`ConfigFile::merge_into`). `[providers]` is one such section: a
  project file that declares one provider hides every system provider, and
  every model id that workspace reaches is routed among the project file's
  entries.
- A missing file is not an error: an absent layer contributes nothing.
- Every struct is `#[serde(default, deny_unknown_fields)]`. **A typo is a load
  failure**, not a quiet ignore: the path and the TOML error are reported.
- Section-level defaults fill every omitted key, so a file may be one line.

The layers are **live** (#337): no restart and no reload command, the file is
the interface.

- Each layer's **directory** is watched, non-recursively, filtered on the file
  name `config.toml` — those dirs also carry sessions, agents, skills and plans,
  so only that file is config. An empty batch counts as config: the rescan that
  self-heals a lost event.
- A reload is **all-or-nothing per layer**: a clean read swaps that layer and
  drops the merges derived from it. A rejected edit applies **nothing** — the
  last good config stays live — and surfaces as the session-less system event
  `config_rejected { path, reason }` (ADR-0034: a failure belonging to no
  session is not a session diagnostic). A parse failure names the layer file;
  an I/O failure names none, and so does a validation failure — it is a property
  of the merged config, not of one file.
- A live session takes the new config at the **next turn boundary**, never
  mid-stream, re-deriving its turn from the provider its model id names. A model
  an edit removes sends its context gate to missing and records
  `gate_unavailable` **once** per transition.
- `provider_changed` is edge-triggered on the rows a model menu would newly see,
  so a save that changes nothing emits nothing.

Model ids: the model **id is the map key**, declared bare. The GUI's model menu
names a model `provider/key`. The structure plan's §6.7 is the one read of an id
(`Config::route_model`, `Config::model_key`): a prefix naming a configured
provider selects that provider and the remainder is its key; a bare key is
looked up among the providers that declare it. The route decides the `base_url`
a request goes to, the facts it is built from, and the provider a subagent
child is addressed at — never the provider the config's map sorts first. It
applies to every id the harness resolves a provider's facts from, including
`[generation].default_model` and `[om].om_model` (`Config::route_om`, #348): the
memory passes are addressed at the provider the id names and asked of it for the
prefix stripped. An `om_model` that addresses no endpoint is refused — the
passes stop and the record carries `om_failed { stage: "route" }` naming the
config key and the id — because the memory has no address of its own to borrow. See
[Provider resolution](#provider-resolution).

Agent types are **not** in this file: they are `agents/*.md` under
`~/.config/tau/agents/` and `{project}/.tau/agents/`, project winning on a name
collision (`agent_type.rs`). Presentation preferences are not here either — they
are an in-app per-user store (spec §12).

## `[providers.<name>]` — connection

One entry per OpenAI-compatible endpoint (ADR-0003: that protocol only; no
keychain, no auto-detection, spec §6).

| Key | Type | Default | Notes |
|---|---|---|---|
| `base_url` | string | `""` | API root, e.g. `http://127.0.0.1:1234/v1` |
| `key_env` | string | `""` | **name of an env var** holding the key; empty = no key sent |
| `models` | map | empty | model id = key; value is the table below |

Providers are a `BTreeMap`, and **a model id picks the provider it is addressed
at** (#336) — the entry its prefix names, or, for a bare key, the one provider
that declares it. The alphabetically-first entry is only the starting address:
the fallback a session with no recorded model opens on, and the provider a bare
key is looked up against at the open. See [Provider resolution](#provider-resolution).

### `[providers.<name>.models.<id>]` — model facts
`context_window` is **required**: a model entry that omits it, or writes `0`,
fails the config load (#338) — the context gate computes from that fact, and an
unbounded prompt is the failure the load refuses. The rest are optional; absent
means the default. Authoring them is the user's job (spec §6: fidelity is the
user's responsibility).

| Key | Type | Default | Notes |
|---|---|---|---|
| `context_window` | int (tokens) | **required** | the model's context window. Omitting it, or writing `0` (the undeclared sentinel), is a load failure |
| `max_tokens` | int (tokens) | 16384 | this model's output cap — a **hard ceiling** on the answer, never a reservation against the prompt: a request carries `min(max_tokens, context_window − prompt − 4096)`, so a wish at the window is shrunk, not refused |
| `reasoning` | bool | none | `false` means the effort is never sent for this model, even with a level configured — the server would reject it. `true` is carried, no consumer: the effort itself comes from `[thinking].level` / `levels` |
| `cost` | table | none | `{ input, output }` per 1M tokens, USD. **Carried, no consumer** — the GUI cost display is post-v0 |

Gate and clamp price one quantity in two constants (`provider/request.rs`):
`CONTEXT_GATE_MARGIN` = 4096 (the framing — system prompt, tool schemas, request
envelope) and `MIN_ANSWER_ROOM` = 4096 (the smallest reply worth sending). The cap
a request carries is `clamp_max_output` = `min(max_tokens, context_window − prompt
− 4096)`; the prompt's ceiling is `prompt_budget` = `context_window − 4096 −
4096`, a number **no cap can move** — `max_tokens` bounds the answer, not the
prompt, which is why `max_tokens = context_window` arms the gate and the clamp
shrinks the wish. The cap that reaches the wire is the thinking-adjusted one, so
a high thinking level spends the room the prompt leaves; it never moves the
prompt's ceiling. A prompt over the ceiling climbs the compaction ladder; one the
ladder cannot fit, and a window that leaves nothing once the margin and an
answer are taken (`context_window <= 8192`), both record
`gate_unavailable { context_room }` — a declared window is never asked to
answer 0 tokens — and a gate with a gap is a refusal, never an unbounded prompt
(ADR-0034). An absent `max_tokens` takes 16384 (`Provider::DEFAULT_MAX_OUTPUT`);
a model entry built in code that names no facts — the ACP env channel, a test
kit — carries 131072 / 16384 (`Provider::default_def`).

## Provider resolution

v0 has no dynamic providers (`ProviderAdd`/`Set`/`Delete` are
`unsupported: v0 providers are config-file-driven`), and **a model id routes to
the provider that owns it** (#336) — never to the provider the config's map
sorts first.

| Question | Answer |
|---|---|
| Which `base_url` does a request use? | the routed provider's |
| Where are a model's facts read from? | that provider's `models` map, keyed by §6.7's key (`Config::model_key_for`) |
| What may `default_model` name? | any model any provider declares: a prefixed id names its provider, a bare key is taken where it is declared |
| What does a menu switch do? | routes the new id against the provider the session is on (`switch_route`), then re-derives the turn from the owner it lands on |
| What does a subagent child use? | the provider its own model id routes to (`Config::route_child`); an id naming no provider keeps the parent's, which the spawn already resolved |
| Where does a child with no model of its own get one? | from its parent — the model the parent is on **when the child spawns** (#336), so a menu switch reaches children spawned after it, never a child already running |

The route (`Config::route_model`, applied by `open_route` / `switch_route` in
`harness/sessions.rs`):

- a prefix naming a configured provider → that provider owns the id;
- a prefix naming **no** configured provider → a refusal,
  `model names no configured provider '{prefix}'`, never a silent route to the
  first provider;
- a bare key exactly one provider declares → that provider;
- a bare key several providers declare, and a key none declares → the session
  stays on the provider it is already addressed at: the prefix is what
  disambiguates between providers, so the core never guesses across them.

A session opens on the model its record last named, else `default_model`, routed
by the same rule. Two providers declaring the same model key are told apart by
the prefix: the menu row is `zzz/model`, and picking `model` alone keeps the
provider the session is on. Scope a workspace's endpoints with the project layer
— `{workspace}/.tau/config.toml` replaces the `providers` section wholesale for
that workspace only (`workspace_config`):

```toml
# ~/work/api-project/.tau/config.toml — this workspace only
[providers.zzz]
base_url = "https://api.example.com/v1"
key_env = "EXAMPLE_API_KEY"

[providers.zzz.models.model-id-as-the-endpoint-names-it]
context_window = 200000
max_tokens = 32000
```
## `[generation]` — sampling

`None` means the server's own default; the key is simply omitted from the request.

| Key | Type | Default | Notes |
|---|---|---|---|
| `default_model` | string | `""` | any model any provider declares: a `provider/` prefix names that provider, a bare key is taken where it is declared (§6.7). An empty value, a prefix naming no provider, and a key no provider declares are the empty-value contract — the first provider's first model (`default_model`, `harness/sessions.rs`) — see [Provider resolution](#provider-resolution) |
| `temperature` | float | none | |
| `max_tokens` | int | none | the output cap for the session, before the thinking adjustment below; a model entry's own `max_tokens` wins, and the window clamps both per call — a ceiling on the answer, never a reservation against the prompt |
| `top_p` | float | none | |
| `frequency_penalty` | float | none | |
| `presence_penalty` | float | none | |

## `[thinking]` — reasoning effort

| Key | Type | Default | Notes |
|---|---|---|---|
| `level` | `off` `minimal` `low` `medium` `high` `xhigh` `max` | `off` | the session's default effort |
| `levels` | map model-id → level | empty | per-model override |
| `budgets` | map level-name → tokens | empty | thinking room reserved for the level |
| `summary` | bool | `false` | ask for a reasoning summary (`reasoning.summary`) |

The wire carries only `reasoning.effort` — the Responses API has no
`budget_tokens` — so a budget is spent by **raising the output cap** to
`base + budget`, capped at the model's `max_tokens` (`harness/thinking.rs`,
#36). `xhigh` and `max` fold to the `high` budget, so the keys `budgets` reads
are `minimal`, `low`, `medium`, `high`. Built-in budgets: minimal 1024,
low 2048, medium 8192, high 16384. A `level` of `off` leaves the cap untouched.
The window has the last word: the adjusted cap is clamped per call
(`CONTEXT_GATE_MARGIN`).

## `[cache]` — prompt-cache policy

| Key | Type | Default | Notes |
|---|---|---|---|
| `retention` | `none` `short` `long` | `none` | `short` = 24 h, `long` = 7 d. Anything but `none` turns on the session's stable cache key |
| `warming` | `off` `streaming` `idle` | `off` | **Carried, no consumer** (spec §12) — parsing it changes nothing |

## `[requests]` — resilience

| Key | Type | Default | Notes |
|---|---|---|---|
| `timeout_secs` | int (seconds) | 120 | the connect/headers deadline only |
| `idle_timeout_secs` | int (seconds) | 120 | the stream's **per-chunk** idle timeout, reset on every chunk. Never a total deadline — a total kills healthy long streams mid-body |
| `retries` | int | 2 | |
| `tool_batch_on_force` | `complete` `kill` | `complete` | what a force-send does to an in-flight tool batch (spec §7) |

## `[om]` — observational memory

The compaction model is global, not per-provider (spec §4).

| Key | Type | Default | Notes |
|---|---|---|---|
| `om_model` | string | `""` | empty = the session's own model. §6.7 applies (#348): a prefix naming a provider addresses the memory passes there and asks it for the stripped key; a bare key goes to the provider that declares it — the provider the session rides when several declare it, a contested key elsewhere being a refusal. An id that addresses no endpoint stops the passes and records `om_failed { stage: "route" }` — compaction stops, so the window gate can refuse the turn, and the config edit is the fix |
| `observe_threshold` | int | 30000 | |
| `reflect_threshold` | int | 40000 | |
| `buffer_increment` | int | 6000 | |
| `buffer_tokens` | int | 6000 | the async boundary trigger's token count; `0` disables that trigger |
| `retries` | table | `{8, 8, 8}` | `{ observe, buffer, reflect }` — retries *after* the first failure, per trigger point, before the cycle is abandoned (#86) |

The four thresholds are validated at the parse boundary: each must be ≤
`u32::MAX` (`LoadError::OmThreshold`, ticket #46). An oversized threshold is a
config error, never a launch panic.

## `[subagents]` — delegation

| Key | Type | Default | Notes |
|---|---|---|---|
| `max_depth` | int | 1 | |
| `max_concurrent` | int | unset | unset = unlimited children |
| `fleet_soft_bound` | int | 32 | total child handles before spawn-triggered reclamation evicts the oldest terminal children (#250) |
| `fleet_hard_cap` | int | 64 | the backstop the soft bound exists to stay under |

`fleet_soft_bound > fleet_hard_cap` is rejected at load
(`LoadError::FleetCapInversion`) — an inverted pair would make every spawn a cap
refusal.

## `[limits]` — size / safety

| Key | Type | Default | Notes |
|---|---|---|---|
| `max_request_bytes` | int (bytes) | unset | unset = unbounded. **Carried, no consumer** |
| `image` | table | below | image-read caps (#34's home) |

### `[limits.image]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `max_bytes` | int | 10485760 | enforced on a `read` of an image file |
| `max_pixels` | int | 2097152 | enforced: a read image above this is downscaled, with a note mapping wire coordinates back (#257) |
| `max_width` | int | 2048 | **Carried, no consumer** |
| `max_height` | int | 2048 | **Carried, no consumer** |
| `jpeg_quality` | int 1–100 | 80 | **Carried, no consumer** — nothing transcodes a read to JPEG |

## `[gates]` — the completion gate

A global, mechanical, user-configured check run at every qualifying turn-end
(spec §5.3). **The section's presence turns the gate on** — there is no
`off`/`advisory`/`enforce` dial; the user's only control is what the command is
(ADR-0007 supplement: a deliberate transparency strengthening).

| Key | Type | Default | Notes |
|---|---|---|---|
| `command` | string | `""` | a shell line, run with the user's privileges |
| `timeout_secs` | int (seconds) | 300 | the check's deadline |
| `max_retries` | int | 2 | failed repair turns before the session is marked `gate_failed` |

A `[gates]` section with a blank `command` is a **no-gate**, not a silent
no-op gate: `bash -c ""` exits 0, which would pass every qualifying turn while
the prompt claims verification (`merge_into`).

## `[hashline]` — pin-store GC

| Key | Type | Default | Notes |
|---|---|---|---|
| `retention_days` | int | 180 | delete pin records untouched within this many days; `0` disables age expiry (the missing-file sweep always runs) (#225) |
| `gc_on_start` | bool | `true` | run the session-start sweeps |

## ACP-mode environment

`tau acp` layers a synthetic provider named `acp-env` on top of these files when
`TAU_BASE_URL` **and** `TAU_MODEL` are both set; the key, if `TAU_API_KEY` is
set, is read from it at request time (`crates/tau-acp/src/config.rs`). That
provider joins the same `BTreeMap`, and its model id routes like any other
(#336). The channel names no model facts, so its entry declares the gate pair
131072 / 16384 (`Provider::default_def`) — #338 requires a window, and the
env-supplied model reaches it by prefix, `acp-env/<model>`.
`TAU_ENDPOINT` names the mock's address for the `accept-*` suites; the scoped
consumer of `TAU_ENDPOINT`/`TAU_MODEL` is the harbor/ACP benchmark runs
(ADR-0010, errata 2026-10-08 — the in-repo eval rig is retired).

```toml
# ~/.config/tau/config.toml
#
# Declare every endpoint a session reaches here, or in the workspace's own
# .tau/config.toml, which replaces [providers] for that workspace alone. A
# model id routes to the provider its prefix names, so a multi-provider file
# needs no ordering: the menu spells each row `provider/key`.

[providers.lmstudio]
base_url = "http://127.0.0.1:1234/v1"
key_env = ""                      # local endpoint, no key

[providers.lmstudio.models."gpt-5-mini"]
context_window = 131072
max_tokens = 16384
reasoning = true
cost = { input = 0.25, output = 2.0 }   # carried, no consumer
```
[providers.lmstudio.models."qwen3-coder"]
context_window = 262144
max_tokens = 32768

[generation]
# Routed by §6.7: the menu's spelling (provider/key) names the provider, a bare
# key is taken where it is declared.
default_model = "lmstudio/gpt-5-mini"
temperature = 0.0
max_tokens = 16384

[thinking]
level = "medium"
levels = { "qwen3-coder" = "off" }      # keyed by the bare model key
budgets = { high = 24576 }
summary = true

[cache]
retention = "short"

[requests]
timeout_secs = 120
idle_timeout_secs = 300
retries = 2
tool_batch_on_force = "complete"

[om]
# Sent to the endpoint verbatim: §6.7's prefix is not stripped for this key, so
# write the id the endpoint expects.
om_model = "qwen3-coder"
observe_threshold = 30000
reflect_threshold = 40000
buffer_increment = 6000
buffer_tokens = 6000
retries = { observe = 8, buffer = 8, reflect = 8 }

[subagents]
max_depth = 1
max_concurrent = 4
fleet_soft_bound = 32
fleet_hard_cap = 64

[limits]
[limits.image]
max_bytes = 10485760
max_pixels = 2097152

[gates]
command = "cargo test"
timeout_secs = 300
max_retries = 2

[hashline]
retention_days = 180
gc_on_start = true
```

A project layer that changes only the gate, and one that changes only the
endpoint:

```toml
# .tau/config.toml — replaces the system [gates] wholesale; everything else inherits
[gates]
command = "npm run test:unit"
```

```toml
# .tau/config.toml — replaces the system [providers] wholesale for this workspace
[providers.selfhosted]
base_url = "http://10.0.0.5:8080/v1"
key_env = ""

[providers.selfhosted.models.coder]
context_window = 131072
max_tokens = 16384

[generation]
default_model = "coder"
```

A project `[generation]` replaces the system one wholesale, so a project that
pins a model repeats the sampling keys it wants to keep. A project `[providers]`
that replaces a provider must carry its `context_window`: the window is
validated on the merged config (#338).

## Load failures

`LoadError` is the whole list; a config that fails to load is reported, never
partially applied.

- `parsing {path}: …` — TOML error, including any unknown key
- `reading config: …` — I/O error on a file that exists
- `{field} = {value} exceeds the u32 token limit (…)` — an `[om]` threshold
- `providers.{provider}.models.{model} declares no context_window; the context gate needs it` — a model entry that omits the window (#338)
- `providers.{provider}.models.{model} declares max_tokens = 0; the output cap must be positive` — a declared cap of `0` arms a gate that can answer nothing (#338)
- `subagents.fleet_soft_bound ({soft}) must not exceed fleet_hard_cap ({hard})`

The two gate facts name the entry they reject: `providers.<name>.models.<id>`
for a model entry, `generation` for `[generation].max_tokens`.

A validation failure is a property of the merged config, not of one file, so it
names no layer path (`config_rejected` carries `path: null`, #337).

A provider with no `models` entry, reached as a session's provider, is a runtime
`provider {name} has no models`, not a load error.
