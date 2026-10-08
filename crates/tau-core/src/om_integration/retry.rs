//! Retry for the OM provider round-trips (parity with mastra's `withRetry`,
//! `docs/research/om-async-parity.md` §6): exponential backoff 1 s → 120 s
//! with ±20 % jitter, transient-class failures only. The count is
//! configurable per trigger point (ticket #86).

use std::time::Duration;

use rand::RngExt;

use crate::om_integration::{NoopSink, OmError};
use crate::provider::{ProviderError, ResponseRequest, TurnProviderRef, TurnResult};

/// The transient class (mastra §6.2, adapted to [`ProviderError`]):
/// transport failures (connect/timeout), a dead stream (idle timeout), and
/// 408/425/429/5xx statuses. Any other 4xx, a malformed stream, and
/// session-storage errors are permanent: one attempt.
#[must_use]
pub fn is_transient(e: &OmError) -> bool {
    match e {
        OmError::Provider(ProviderError::Request(e)) => e.is_connect() || e.is_timeout(),
        OmError::Provider(ProviderError::IdleTimeout) => true,
        OmError::Provider(ProviderError::Status { status, .. }) => {
            (500..=599).contains(status) || matches!(status, 408 | 425 | 429)
        }
        OmError::Provider(ProviderError::MalformedStream(_)) | OmError::Session(_) => false,
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
    label: &'static str,
    retries: u32,
) -> Result<TurnResult, OmError> {
    let mut n = 0u32;
    loop {
        let mut sink = NoopSink;
        match provider
            .call(request, &mut sink)
            .await
            .map_err(OmError::Provider)
        {
            Ok(result) => return Ok(result),
            Err(e) if n < retries && is_transient(&e) => {
                n += 1;
                let base = (1u32 << (n - 1).min(7)).min(120);
                let jitter = rand::rng().random::<f64>();
                let delay = Duration::from_secs_f64(f64::from(base) * (0.8 + 0.4 * jitter));
                tracing::debug!(
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
        OmError::Provider(ProviderError::Status {
            status: code,
            body: String::new(),
        })
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
        assert!(is_transient(&OmError::Provider(ProviderError::IdleTimeout)));
        for code in [408u16, 425, 429, 500, 503, 599] {
            assert!(is_transient(&status(code)), "{code} is transient");
        }
        for code in [400u16, 404, 413] {
            assert!(!is_transient(&status(code)), "{code} is permanent");
        }
        assert!(!is_transient(&OmError::Provider(
            ProviderError::MalformedStream("x".into())
        )));
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
            "test",
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
            "test",
            2,
        )
        .await;
        assert!(matches!(
            out,
            Err(OmError::Provider(ProviderError::IdleTimeout))
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
            "test",
            8,
        )
        .await;
        assert!(matches!(
            out,
            Err(OmError::Provider(ProviderError::Status { status: 400, .. }))
        ));
        assert_eq!(p.fails.load(Ordering::SeqCst), 1);
    }
}
