# 7. v0 security model: transparency, not enforcement

Date: 2026-09-17

## Status
Accepted — inline user decision during build-map charting (2026-09-17), resolving spec §11 / U1.

## Decision
tau v0 has **no enforcement security model**: no sandbox, no permission popups, no trust flow, no per-tool allow/deny policy, no server auth. Tools execute with the user's privileges on the whole host filesystem. The **one enforced boundary** is that sub-agent output is marked **untrusted** before the parent model reads it. Security in v0 is **transparency**: the GUI shows exactly what is running (per-tool cards with arguments and output, per-tool kill buttons, force semantics, the sub-agent tree with full lifecycle states), and the user's escape hatches are stop/force/kill. The protocol keeps the HITL request/response *shape* with zero live request types — a **post-v0 security ticket** decides what enforcement comes back (per-tool policy, live HITL types such as command approval, containerized distribution).

## Rationale
- pi's stance: no permission popups; the industry answer to bash danger is **containerization, not popups** (research #5) — and because tau-core is a library, a containerized deployment of tau is trivially available to users who want it, without tau building one.
- A per-tool policy or an approval popup adds config surface and model-facing complexity to a version whose point is a minimal core — and v0 has **no HITL at all** (#10), so a live "approve dangerous command" type has nowhere to hang in v0.
- v0's trust boundary is the *user's machine and the user's judgment*; tau's job is to make what is running perfectly legible and instantly stoppable.

## Consequences
- Spec §11 is resolved (no longer UNRESOLVED): no sandboxing, no tool policy, no server auth in v0.
- The GUI transparency affordances (spec §9) **are** the security feature, not an add-on.
- `tau serve` (post-v0) ships auth/TLS together with the server (#13).
- The post-v0 security ticket's candidate list, in order: per-tool allow/deny policy (smallest, highest value), live HITL request types, containerized distribution.

## Considered
- **Per-tool allow/deny policy in config** — rejected *for v0* (adds config + enforcement logic to the minimal core); kept as the first candidate of the post-v0 security ticket.
- **A live "approve dangerous commands" HITL type in v0** — rejected: v0 has no HITL surface at all (#10); that is the security ticket's job.
- **Mandating containerization** — rejected: a deployment choice, not a product one; the library core makes it available.
