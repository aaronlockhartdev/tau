//! The provider wire log (#98): the exact outgoing request body and the raw
//! returned response body, emitted as ordinary `tracing` `trace!` events.
//! Replaces the bespoke file-append logger. No feature gates these — a
//! production build (`tracing/max_level_off`) compiles them out entirely, and a
//! diagnostic build (`max_level_trace`) plus a `RUST_LOG`-driven subscriber
//! routes them to its output. The binary's `max_level` is the only control.
//!
//! On top of the verbatim trace dump, a parseable JSONL view sits at `debug!`
//! (`ResponseRequest::wire_request_jsonl`, `TurnResult::wire_response_jsonl`):
//! one JSON object per line, request and response paired by a shared id
//! (`new_id`), so `RUST_LOG=tau_core=debug` captures the raw model I/O
//! without the trace-level verbosity.
use rand::RngExt;

use super::ResponseRequest;

/// Emit the outgoing request body (serialized verbatim) as a trace event, so
/// the exact wire bytes are recoverable from the log.
pub(super) fn request(request: &ResponseRequest) {
    // Serialized as a `trace!` field (not a `let`) so a `max_level_off` build
    // compiles the whole call — serialization included — out.
    tracing::trace!(body = %serde_json::to_string(request).unwrap_or_default(), "llm request");
}

/// Emit the raw returned response body (the SSE bytes, verbatim) as a trace
/// event — this is what distinguishes a server-side empty completion from a
/// client-side parse drop.
pub(super) fn response(body: &str) {
    tracing::trace!(%body, "llm response");
}

/// A per-attempt correlation id pairing a request with its response in the
/// JSONL log: 16 random bits, hex. Collisions are possible but harmless — a
/// pair is read by adjacency and `dir`, never keyed on the id alone.
pub(super) fn new_id() -> String {
    format!("{:04x}", rand::rng().random_range(0..u16::MAX))
}
