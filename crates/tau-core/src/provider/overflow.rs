//! The over-window 400 (spec §4, #338): the signatures that name a context
//! overflow, and the provider's own arithmetic when the body carries it.
//!
//! The classifier is the gate's last line: a 400 it does not recognize is
//! recorded and propagated, and the request that produced it died on the wire.
//! The gateway shape that killed the subagent in the #338 incident is
//!
//! `request exceeds the context window: prompt (205014 tokens) leaves no room
//! to answer in the context (204800); requests are never truncated`
//!
//! — none of the strings the original #256 classifier matched.

use super::ProviderError;

use crate::provider::CONTEXT_GATE_MARGIN;

/// The phrases that name a context overflow (the #256 set, broadened for the
/// gateway shape, #338). Case-insensitive: bodies arrive in every casing.
const OVERFLOW_SIGNATURES: [&str; 6] = [
    "context length",
    "context window",
    "context_length_exceeded",
    "leaves no room",
    "too long",
    "input_tokens",
];

/// Whether a status/body names a context overflow.
#[must_use]
pub(crate) fn is_overflow_body(status: u16, body: &str) -> bool {
    if status != 400 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    OVERFLOW_SIGNATURES.iter().any(|s| lower.contains(s))
}

/// The provider's own numbers from an over-window 400 (#338): what it charged
/// the prompt and the window it measured against. A body with no numbers — the
/// plain `"context length exceeded"` shape — reports nothing, and the retry
/// falls back to halving its own cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextReport {
    pub prompt_tokens: u64,
    pub window_tokens: u64,
}

impl ContextReport {
    /// The room the provider's arithmetic leaves for an answer once the
    /// framing margin is taken: the cap the repriced retry carries.
    #[must_use]
    pub fn room(&self) -> u64 {
        self.window_tokens
            .saturating_sub(self.prompt_tokens)
            .saturating_sub(u64::from(CONTEXT_GATE_MARGIN))
    }
}

/// The words that say which side of the arithmetic a number names. Shapes
/// differ in order — the gateway states the prompt first, `OpenAI` the window
/// first — so proximity decides, not position in the body.
/// The phrases that name a number as the prompt's token count. `request` is
/// deliberately absent: the gateway's own prose (`requests are never
/// truncated`) sits two characters from its window number and would claim it.
const PROMPT_HINTS: [&str; 2] = ["prompt", "input"];
const WINDOW_HINTS: [&str; 2] = ["context", "window"];

/// The window of prose searched on each side of a number for its hint.
const HINT_PROXIMITY: usize = 48;

/// The first index at which `needle` occurs in `hay` (`rfind`: the last).
/// Bodies arrive as bytes: an error page is not guaranteed UTF-8 at a chosen
/// offset, and a str slice at a byte index is the panic the repo denies.
fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|i| &hay[*i..*i + needle.len()] == needle)
}

fn rfind_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len())
        .rev()
        .find(|i| &hay[*i..*i + needle.len()] == needle)
}

/// The value of an all-ASCII-digit run, built without a str slice.
fn parse_digits(digits: &[u8]) -> u64 {
    digits.iter().fold(0u64, |acc, b| {
        acc.saturating_mul(10)
            .saturating_add(u64::from(*b).saturating_sub(u64::from(b'0')))
    })
}

/// The gap from a number to the nearest occurrence of `hints`, searching
/// backwards from `start` and forwards from `end`; `None` when no hint is
/// near. The gap is the byte distance between them, so the tightest binding
/// wins.
fn hint_gap(lower: &[u8], start: usize, end: usize, hints: &[&str]) -> Option<usize> {
    let from = start.saturating_sub(HINT_PROXIMITY);
    let pre = &lower[from..start];
    let post = &lower[end..(end + HINT_PROXIMITY).min(lower.len())];
    let backwards = hints
        .iter()
        .filter_map(|h| rfind_bytes(pre, h.as_bytes()).map(|p| start - (from + p + h.len())));
    let forwards = hints.iter().filter_map(|h| find_bytes(post, h.as_bytes()));
    backwards.chain(forwards).min()
}

/// Parse the provider's numbers out of an over-window body: each digit run is
/// bound to the nearest hint, and the pair is a report only when both sides
/// are named.
#[must_use]
pub(crate) fn parse_context_report(body: &str) -> Option<ContextReport> {
    let lower = body.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut prompt: Option<u64> = None;
    let mut window: Option<u64> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let end = bytes[i..]
            .iter()
            .position(|b| !b.is_ascii_digit())
            .map_or(bytes.len(), |n| i + n);
        // A number only speaks where its prose names it: an unbound count
        // (`3 requests per minute`) is a rate, not a token count.
        let prompt_gap = hint_gap(bytes, i, end, &PROMPT_HINTS);
        let window_gap = hint_gap(bytes, i, end, &WINDOW_HINTS);
        let value = parse_digits(&bytes[i..end]);
        let bound_to_prompt = match (prompt_gap, window_gap) {
            (Some(p), Some(w)) => p <= w,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => continue,
        };
        if bound_to_prompt {
            prompt.get_or_insert(value);
        } else {
            window.get_or_insert(value);
        }
        i = end;
    }
    match (prompt, window) {
        (Some(prompt_tokens), Some(window_tokens)) => Some(ContextReport {
            prompt_tokens,
            window_tokens,
        }),
        _ => None,
    }
}

impl ProviderError {
    /// The provider's numbers, when an over-window 400 carries them (#338).
    #[must_use]
    pub fn context_report(&self) -> Option<ContextReport> {
        let Self::Status { status, body } = self else {
            return None;
        };
        if !is_overflow_body(*status, body) {
            return None;
        }
        parse_context_report(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(body: &str) -> ProviderError {
        ProviderError::Status {
            status: 400,
            body: body.to_owned(),
        }
    }

    /// The exact body from the #338 incident's dead subagent.
    const GATEWAY: &str = "request exceeds the context window: prompt (205014 tokens) leaves no room to answer in the context (204800); requests are never truncated";

    /// The `OpenAI` shape, which states the window before the prompt.
    const OPENAI: &str = "This model's maximum context length is 32768 tokens. However, your prompt contains at least 30000 input tokens.";

    #[test]
    fn the_overflow_signatures_are_matched() {
        assert!(is_overflow_body(400, GATEWAY));
        assert!(is_overflow_body(400, OPENAI));
        assert!(is_overflow_body(
            400,
            r#"{"error":{"code":"context_length_exceeded"}}"#
        ));
        assert!(is_overflow_body(400, "prompt is too long: 210000 tokens"));
    }

    #[test]
    fn an_unrelated_status_is_not_an_overflow() {
        assert!(!is_overflow_body(400, "Invalid API key provided."));
        assert!(!is_overflow_body(401, GATEWAY));
        assert!(!is_overflow_body(500, GATEWAY));
    }

    #[test]
    fn the_gateway_report_is_its_own_arithmetic() {
        let report = parse_context_report(GATEWAY).expect("the gateway states both numbers");
        assert_eq!(report.prompt_tokens, 205_014);
        assert_eq!(report.window_tokens, 204_800);
        // The prompt is over the window: the provider's own arithmetic leaves
        // no room, so no cap saves this request — only compaction does.
        assert_eq!(report.room(), 0);
    }

    #[test]
    fn the_openai_shape_reports_both_sides_whatever_their_order() {
        let report = parse_context_report(OPENAI).expect("both numbers");
        assert_eq!(report.window_tokens, 32_768);
        assert_eq!(report.prompt_tokens, 30_000);
        // 32768 − 30000 leaves 2768, below the framing margin: the margin is
        // taken first, so the cap is 0 and no reprice can save this request.
        assert_eq!(report.room(), 0);
        // A shorter prompt leaves room the retry can spend.
        let roomy = parse_context_report(
            "This model's maximum context length is 32768 tokens. However, your prompt contains at least 20000 input tokens.",
        )
        .expect("both numbers");
        assert_eq!(roomy.room(), 32_768 - 20_000 - 4_096);
    }

    #[test]
    fn a_body_without_the_pair_reports_nothing() {
        assert!(parse_context_report("context length exceeded").is_none());
        assert!(parse_context_report("context window is 8192").is_none());
    }

    #[test]
    fn an_overflow_error_carries_its_report() {
        assert!(status(GATEWAY).context_report().is_some());
        assert!(status("Invalid API key.").context_report().is_none());
        assert!(ProviderError::IdleTimeout.context_report().is_none());
    }
}
