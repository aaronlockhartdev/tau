//! Debug build: append every outgoing LLM request to a per-process log.
//!
//! Gated behind the `log-llm-requests` feature and the `TAU_LLM_REQUEST_LOG`
//! env var (a file path). Each line is `{"n": <seq>, "ts": <epoch_ms>,
//! "req": <the exact request body>}` — one per send, so retries are visible.
//! A no-op (nothing written) when the env var is unset.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use super::ResponseRequest;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// The `TAU_LLM_REQUEST_LOG` value, if set to a non-empty path.
fn path() -> Option<String> {
    std::env::var("TAU_LLM_REQUEST_LOG")
        .ok()
        .filter(|s| !s.is_empty())
}

/// One logged request: a sequence number, a timestamp, and the exact body.
#[derive(Serialize)]
struct Line<'a> {
    n: u64,
    ts: u128,
    req: &'a ResponseRequest,
}

/// Append one outgoing request to the per-process log. Best-effort: any
/// failure (no path, serialize error, can't open the file) is swallowed —
/// logging must never affect the turn.
pub fn log_request(request: &ResponseRequest) {
    let Some(p) = path() else {
        return;
    };
    let n = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let Ok(line) = serde_json::to_string(&Line {
        n,
        ts,
        req: request,
    }) else {
        return;
    };
    if let Some(parent) = std::path::Path::new(&p).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    else {
        return;
    };
    let _ = writeln!(f, "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{InputEntry, InputMessage};

    fn req() -> ResponseRequest {
        ResponseRequest::new(
            "test-model",
            Some("you are a test"),
            vec![InputEntry::Message(InputMessage {
                role: "user".into(),
                content: "hello world".into(),
            })],
        )
    }

    /// Both phases in one test: `env::set_var`/`remove_var` are process-global
    /// (and `unsafe` on edition 2024), so a single test avoids the parallel-env
    /// race two separate tests would create.
    #[test]
    fn logs_the_raw_request_and_noops_without_the_env_var() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/llm-requests.jsonl");

        // Phase 1: env set → the raw request body is appended, verbatim.
        unsafe { std::env::set_var("TAU_LLM_REQUEST_LOG", path.to_str().unwrap()) };
        log_request(&req());
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("\"n\":1"));
        assert!(body.contains("\"you are a test\""));
        assert!(body.contains("\"hello world\""));
        assert!(body.contains("\"req\":{"));

        // Phase 2: env unset → nothing is written.
        unsafe { std::env::remove_var("TAU_LLM_REQUEST_LOG") };
        let absent = dir.path().join("absent.jsonl");
        log_request(&req());
        assert!(!absent.exists());
    }
}
