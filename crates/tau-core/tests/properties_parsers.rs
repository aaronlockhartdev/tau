#![allow(clippy::unwrap_used, clippy::panic)]
use proptest::prelude::*;
use tau_core::hashline::{canon, line_hashes, normalize};
use tau_core::provider::SseParser;
use tau_core::session::SessionStore;

proptest! {
    /// The SSE feed is a network parser: arbitrary bytes are always a
    /// Result, never a panic.
    #[test]
    fn sse_feed_never_panics(text in ".*") {
        let mut parser = SseParser::new();
        let _ = parser.feed(text.as_bytes());
    }

    /// A session .jsonl of arbitrary content always loads to a Result.
    #[test]
    fn session_open_never_panics(text in ".*") {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".tau").join("sessions")).unwrap();
        let store = SessionStore::for_workspace(dir.path(), "prop");
        std::fs::write(store.path(), text.as_bytes()).unwrap();
        let mut store = store;
        let _ = store.open();
    }

    /// The CRLF and LF forms of a document hash identically (the hashline
    /// line-ending contract: canon strips the CR, so the pair is invisible).
    #[test]
    fn line_hashes_ignore_line_endings(text in ".*") {
        let lf = normalize(&text);
        let crlf = lf.replace('\n', "\r\n");
        let a = line_hashes(&lf);
        let b = line_hashes(&crlf);
        prop_assert_eq!(a.is_ok(), b.is_ok());
        prop_assert_eq!(a.ok(), b.ok());
    }

    /// canon and normalize are projections: a second application changes
    /// nothing the first did not.
    #[test]
    fn canon_is_idempotent(line in ".*") {
        prop_assert_eq!(canon(&canon(&line)), canon(&line));
    }

    #[test]
    fn normalize_is_idempotent(text in ".*") {
        prop_assert_eq!(normalize(&normalize(&text)), normalize(&text));
    }
}
