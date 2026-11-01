//! Retry for the OM provider round-trips (parity with mastra's `withRetry`,
//! `docs/research/om-async-parity.md` §6): exponential backoff 1 s → 120 s
//! with ±20 % jitter, transient-class failures only. The count is
//! configurable per trigger point (ticket #86).

use std::time::Duration;

use rand::RngExt;

use crate::om_integration::{NoopSink, OmError, OmStage};
use crate::provider::{ProviderError, ResponseRequest, TurnProviderRef, TurnResult};

/// The transient class (mastra §6.2, adapted to [`ProviderError`]):
/// transport failures at any phase (connect setup, request write, the body read
/// that dies mid-transfer), a dead stream (idle timeout), and 408/425/429/5xx
/// statuses. A request that fails to build, any other 4xx, a malformed stream,
/// and session-storage errors are permanent: one attempt.
#[must_use]
pub fn is_transient(e: &OmError) -> bool {
    match e {
        OmError::Provider(_, ProviderError::Request(e)) => {
            // The request died in flight and the model never answered, at any
            // phase: connect setup, request write, or the body read that dies
            // mid-transfer (`error reading a body from connection`, what a
            // pre-body drop reaches `stream.rs` as). Mastra's markers —
            // "terminated", "socket hang up", "connection closed", "fetch
            // failed" — are these, and #353's report is the mid-body one:
            // 269 s in, the connection closed, the pass ended on one attempt.
            e.is_connect() || e.is_timeout() || e.is_request() || e.is_body() || e.is_decode()
        }
        OmError::Provider(_, ProviderError::IdleTimeout) => true,
        OmError::Provider(_, ProviderError::Status { status, .. }) => {
            (500..=599).contains(status) || matches!(status, 408 | 425 | 429)
        }
        OmError::Provider(_, ProviderError::MalformedStream(_)) | OmError::Session(_) => false,
    }
}

/// Run one OM provider round-trip, retrying transient failures `retries`
/// times after the first attempt (`retries + 1` attempts total). Backoff is
/// 1, 2, 4, … seconds capped at 120, each ±20 % jittered. Each attempt gets
/// a fresh sink: a failed attempt must not leave partial content the next
/// one appends to.
pub async fn call_with_retry(
    provider: &TurnProviderRef,
    request: &ResponseRequest,
    stage: OmStage,
    retries: u32,
) -> Result<TurnResult, OmError> {
    let mut n = 0u32;
    // The stage rides the wire log's request line (the #98 debug view): an OM
    // round-trip logs as `observe`/`buffer`/`reflect`, a main turn as
    // unlabeled. Debug-only — the `clone` is the cost, so it's skipped
    // wherever `debug!` is compiled out (a release build sets
    // `release_max_level_error`).
    let label = stage.as_str();
    let labeled_clone = if tracing::level_enabled!(tracing::Level::DEBUG) {
        Some(request.clone().with_wire_label(label))
    } else {
        None
    };
    let labeled = labeled_clone.as_ref().unwrap_or(request);
    loop {
        let mut sink = NoopSink;
        match provider
            .call(labeled, &mut sink)
            .await
            .map_err(|e| OmError::Provider(stage, e))
        {
            Ok(result) => return Ok(result),
            Err(e) if n < retries && is_transient(&e) => {
                n += 1;
                let base = (1u32 << (n - 1).min(7)).min(120);
                let jitter = rand::rng().random::<f64>();
                let delay = Duration::from_secs_f64(f64::from(base) * (0.8 + 0.4 * jitter));
                // Every retry is visible: the pass is late by the backoff, and
                // a `debug!` line hid a dead OM route for a whole incident (#338).
                tracing::warn!(
                    "om {label}: transient failure ({e}); retry {n}/{retries} in {delay:?}"
                );
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::provider::{ProviderTurn, TurnProvider, TurnSink};

    fn status(code: u16) -> OmError {
        OmError::Provider(
            OmStage::Observe,
            ProviderError::Status {
                status: code,
                body: String::new(),
            },
        )
    }

    /// Serves `Ok` after `total_fails` scripted failures; `calls` counts
    /// every attempt.
    struct Flaky {
        fails: AtomicUsize,
        total_fails: usize,
        kind: FailKind,
    }

    #[derive(Clone, Copy)]
    enum FailKind {
        Idle,
        Status(u16),
    }

    impl TurnProvider for Flaky {
        fn call<'a>(
            &self,
            _request: &ResponseRequest,
            _sink: &'a mut dyn TurnSink,
        ) -> ProviderTurn<'a> {
            let fails = self.fails.fetch_add(1, Ordering::SeqCst);
            let total = self.total_fails;
            let kind = self.kind;
            Box::pin(async move {
                if fails < total {
                    let err = match kind {
                        FailKind::Idle => ProviderError::IdleTimeout,
                        FailKind::Status(code) => ProviderError::Status {
                            status: code,
                            body: String::new(),
                        },
                    };
                    Err(err)
                } else {
                    Ok(TurnResult::default())
                }
            })
        }
    }

    #[test]
    fn transient_classification() {
        assert!(is_transient(&OmError::Provider(
            OmStage::Observe,
            ProviderError::IdleTimeout
        )));
        for code in [408u16, 425, 429, 500, 503, 599] {
            assert!(is_transient(&status(code)), "{code} is transient");
        }
        for code in [400u16, 404, 413] {
            assert!(!is_transient(&status(code)), "{code} is permanent");
        }
        assert!(!is_transient(&OmError::Provider(
            OmStage::Observe,
            ProviderError::MalformedStream("x".into())
        )));
    }

    /// A request that died in flight is transient at every transport phase. The
    /// phases come from real sockets: reqwest builds no `Error` by hand.
    #[tokio::test]
    async fn a_transport_failure_at_any_phase_is_transient() {
        // The head arrives, the body is truncated: the read dies mid-transfer,
        // and that is what a pre-body drop reaches the turn as — #353's report
        // (`269 s … connection closed`, one attempt, no retry).
        let truncated = request_dying_mid_body().await;
        assert!(
            truncated.is_decode() || truncated.is_body(),
            "the fixture stopped truncating: {truncated}"
        );
        assert!(is_transient(&OmError::Provider(
            OmStage::Reflect,
            ProviderError::Request(truncated)
        )));

        // The socket dies without answering: a transport failure, at whichever
        // phase reqwest names it — connect setup or the request write.
        let refused = request_dropped_without_answering().await;
        assert!(
            refused.is_connect() || refused.is_request() || refused.is_body(),
            "the fixture stopped dropping: {refused}"
        );
        assert!(is_transient(&OmError::Provider(
            OmStage::Observe,
            ProviderError::Request(refused)
        )));

        // A request that never builds is a config bug: one attempt.
        let builder = reqwest::Client::new()
            .get("not a url")
            .send()
            .await
            .expect_err("a url with no scheme never builds");
        assert!(builder.is_builder(), "{builder}");
        assert!(!is_transient(&OmError::Provider(
            OmStage::Observe,
            ProviderError::Request(builder)
        )));
    }

    /// POSTs to a listener that answers the head and truncates the body.
    async fn request_dying_mid_body() -> reqwest::Error {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let addr = listener.local_addr().expect("the bound address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                // Read the request to its head terminator: answering while the
                // request is still in flight is a send failure, not a cut body.
                let mut seen = Vec::new();
                let mut buf = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match std::io::Read::read(&mut stream, &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = std::io::Write::write_all(
                    &mut stream,
                    b"HTTP/1.1 200 OK\r\ncontent-length: 200\r\n\r\ndata: part",
                );
                let _ = std::io::Write::flush(&mut stream);
                // The promised body never arrives: the socket outlives the write
                // just long enough for the head to be read, then dies.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
        let mut response = reqwest::Client::new()
            .post(format!("http://{addr}/v1/responses"))
            .json(&serde_json::json!({ "model": "m", "input": "observe" }))
            .send()
            .await
            .expect("the head parses");
        loop {
            match response.chunk().await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    panic!("the truncated body ended clean; the fixture stopped truncating")
                }
                Err(e) => return e,
            }
        }
    }

    /// POSTs to a listener that accepts and drops the socket without answering.
    async fn request_dropped_without_answering() -> reqwest::Error {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
        let addr = listener.local_addr().expect("the bound address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                drop(stream);
            }
        });
        reqwest::Client::new()
            .post(format!("http://{addr}/v1/responses"))
            .json(&serde_json::json!({ "model": "m", "input": "observe" }))
            .send()
            .await
            .expect_err("a socket closed at the handshake never answers")
    }

    #[tokio::test]
    async fn retries_transient_until_success() {
        let p = Arc::new(Flaky {
            fails: AtomicUsize::new(0),
            total_fails: 2,
            kind: FailKind::Idle,
        });
        let dyn_p: Arc<dyn TurnProvider> = p.clone();
        let out = call_with_retry(
            &dyn_p,
            &ResponseRequest::new("m".to_string(), None, vec![]),
            OmStage::Observe,
            3,
        )
        .await;
        assert!(out.is_ok());
        assert_eq!(p.fails.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn exhausts_then_returns_the_last_error() {
        let p = Arc::new(Flaky {
            fails: AtomicUsize::new(0),
            total_fails: 100,
            kind: FailKind::Idle,
        });
        let dyn_p: Arc<dyn TurnProvider> = p.clone();
        let out = call_with_retry(
            &dyn_p,
            &ResponseRequest::new("m".to_string(), None, vec![]),
            OmStage::Reflect,
            2,
        )
        .await;
        assert!(matches!(
            out,
            Err(OmError::Provider(
                OmStage::Reflect,
                ProviderError::IdleTimeout
            ))
        ));
        assert_eq!(p.fails.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_transient_fails_fast() {
        let p = Arc::new(Flaky {
            fails: AtomicUsize::new(0),
            total_fails: 100,
            kind: FailKind::Status(400),
        });
        let dyn_p: Arc<dyn TurnProvider> = p.clone();
        let out = call_with_retry(
            &dyn_p,
            &ResponseRequest::new("m".to_string(), None, vec![]),
            OmStage::Buffer,
            8,
        )
        .await;
        assert!(matches!(
            out,
            Err(OmError::Provider(
                OmStage::Buffer,
                ProviderError::Status { status: 400, .. }
            ))
        ));
        assert_eq!(p.fails.load(Ordering::SeqCst), 1);
    }
}
