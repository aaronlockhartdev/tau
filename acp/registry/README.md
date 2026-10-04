# ACP registry entry

Files for the [ACP registry](https://github.com/agentclientprotocol/registry) entry for tau.

`agent.json` is a template: `release-acp.yml` substitutes `$TAG`/`$VERSION`/`$SHA_*` and
uploads the rendered file as the `agent.json` asset of each GitHub Release. `icon.svg`
is the 16×16 `currentColor` icon the registry requires.

Submitting the entry is a manual step: fork `agentclientprotocol/registry`, add a `tau/`
directory with `agent.json` (copied from the rendered release asset) and `icon.svg`, and
open a PR — registry CI validates the schema, launches the binary in a sandbox, and checks
the auth handshake.

The `TAU_BASE_URL`/`TAU_API_KEY`/`TAU_MODEL` contract (OpenAI-compatible endpoint) lives
in the `description` field, not an `env` block: a public placeholder `env` would be
exported verbatim into every deployment.
