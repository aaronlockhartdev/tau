use super::*;
use serde_json::json;

fn mk(id: &str, parent: Option<&str>, ts: u64, kind: &str, text: &str) -> Entry {
    Entry {
        id: id.to_owned(),
        parent: parent.map(str::to_owned),
        timestamp: ts,
        kind: kind.to_owned(),
        payload: json!({ "text": text }),
        blob: None,
        first_kept_entry_id: None,
        crc: None,
    }
}

fn is_stamp(line: &str) -> bool {
    // A transcript entry line: "(HH:MM) [kind] text".
    line.strip_prefix('(')
        .and_then(|r| r.get(0..5))
        .is_some_and(|hhmm| hhmm.as_bytes().get(2) == Some(&b':'))
}

// A single-entry window: one "Date:" header (the entry's own local date) and
// one stamped entry line — the stamp comes from the entry's epoch-ms, never
// from call time (ticket #196).
#[test]
fn transcript_stamps_a_single_entry() {
    // 2023-11-14 22:13:20 UTC — one entry is a single local day in any zone.
    let entries = vec![mk(
        "00000001",
        None,
        1_700_000_000_000,
        "user",
        "hello world",
    )];
    let out = transcript(&entries);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "one Date: header + one entry line, got {out:?}"
    );
    assert!(
        lines[0].starts_with("Date: "),
        "first line is the date header: {out:?}"
    );
    assert_eq!(lines[0].len(), 16, "date is YYYY-MM-DD: {out:?}");
    assert!(
        lines[1].ends_with("[user] hello world"),
        "line keeps kind + text: {out:?}"
    );
    assert!(
        is_stamp(lines[1]),
        "line starts with the (HH:MM) stamp: {out:?}"
    );
}

// A 72-hour span crosses at least one local day in every zone (no local day
// is longer than 48 h), so exactly two "Date:" headers: one per local day,
// the second after the rollover.
#[test]
fn transcript_rolls_over_the_date_header_between_days() {
    let entries = vec![
        mk("00000001", None, 1_700_000_000_000, "user", "first"),
        mk(
            "00000002",
            Some("00000001"),
            1_700_000_000_000 + 72 * 3600 * 1000,
            "assistant",
            "later",
        ),
    ];
    let out = transcript(&entries);
    let headers: Vec<&str> = out.lines().filter(|l| l.starts_with("Date: ")).collect();
    assert_eq!(headers.len(), 2, "one header per local day: {out:?}");
    assert_ne!(
        headers[0], headers[1],
        "the rollover changes the date: {out:?}"
    );
    // Both entry lines are still stamped.
    let stamped = out.lines().filter(|l| is_stamp(l)).count();
    assert_eq!(
        stamped, 2,
        "each entry line keeps its (HH:MM) stamp: {out:?}"
    );
}

// Entries with no text contribute neither a line nor a header bump.
#[test]
fn transcript_skips_entries_without_text() {
    let entries = vec![
        mk("00000001", None, 1_700_000_000_000, "tool", ""),
        mk(
            "00000002",
            Some("00000001"),
            1_700_000_060_000,
            "user",
            "only one",
        ),
    ];
    let out = transcript(&entries);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 2, "empty-text entry drops out: {out:?}");
    assert!(lines[1].ends_with("[user] only one"), "got {out:?}");
}

// The Observer prompt points at the transcript's stamps as the time source,
// and the #191 preamble is gone from the prompt.
#[test]
fn observer_prompt_points_at_the_transcript_timestamps() {
    let prompt = crate::om::observer_system_prompt();
    assert!(
        prompt.contains("THOSE timestamps"),
        "prompt directs the observer to copy the transcript stamps"
    );
    assert!(
        !prompt.contains("Current date and time (local):"),
        "the #191 preamble is removed"
    );
    // The format example no longer carries a concrete copyable date.
    assert!(
        !prompt.contains("Dec 4, 2025"),
        "example dates are YYYY-MM-DD"
    );
    assert!(prompt.contains("Date: 2025-12-04"));
}
