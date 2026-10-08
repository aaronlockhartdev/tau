//! The provider wire log (#98): the exact outgoing request body and the raw
//! returned response body, emitted as ordinary `tracing` `trace!` events.
//! Replaces the bespoke file-append logger. No feature gates these — a
//! production build (`tracing/max_level_off`) compiles them out entirely, and a
//! diagnostic build (`max_level_trace`) plus a `RUST_LOG`-driven subscriber
//! routes them to its output. The binary's `max_level` is the only control.

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

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_test::traced_test;

    fn sample() -> ResponseRequest {
        ResponseRequest::new("test-model", Some("you are a test"), vec![])
    }

    /// The request event carries the serialized body at trace level.
    #[test]
    #[traced_test]
    fn request_emits_the_body() {
        request(&sample());
        assert!(logs_contain("llm request"));
        assert!(logs_contain("you are a test"));
    }

    /// The response event carries the raw body at trace level.
    #[test]
    #[traced_test]
    fn response_emits_the_body() {
        response("data: {\"id\":\"x\"}\n\n");
        assert!(logs_contain("llm response"));
        assert!(logs_contain("data:"));
    }
}
