use super::*;
use jiff::Timestamp;
use jiff::tz::TimeZone;

// (a) the jiff formatter is exact in a fixed zone — pins the spec + zone
// handling (the axis the handrolled civil-from-days was fragile on).
#[test]
fn format_utc_is_exact_for_known_epochs() {
    let at = |secs: i64| format_in(TimeZone::UTC, Timestamp::from_second(secs).unwrap());
    assert_eq!(at(0), "1970-01-01 00:00");
    assert_eq!(at(86_400), "1970-01-02 00:00");
    assert_eq!(at(1_700_000_000), "2023-11-14 22:13");
    // leap day (the fragile axis) and pre-1970 (negative):
    assert_eq!(at(1_709_164_800), "2024-02-29 00:00");
    assert_eq!(at(-1), "1969-12-31 23:59");
}

// (b) the dated user message carries the injected date line — a 16-char
// "YYYY-MM-DD HH:MM", independent of the machine's zone.
#[test]
fn dated_user_content_carries_the_date_line() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let out = dated_user_content("the raw transcript", now);
    assert!(out.starts_with("Current date and time (local): "));
    assert!(out.ends_with("the raw transcript"));
    let line = out.lines().next().expect("the preamble is the first line");
    let after = line
        .strip_prefix("Current date and time (local): ")
        .expect("the date prefix");
    assert_eq!(after.len(), 16, "expected YYYY-MM-DD HH:MM, got {line:?}");
}

// negative: an empty body still gets the date line (the preamble is always
// present, not conditional on there being content).
#[test]
fn dated_user_content_empty_body_still_has_the_date_line() {
    let now = Timestamp::from_second(1_700_000_000).unwrap();
    let out = dated_user_content("", now);
    assert!(out.starts_with("Current date and time (local): "));
}
