use super::*;
use jiff::Timestamp;
use jiff::tz::TimeZone;

// The per-entry stamps (ticket #196) are exact in a fixed zone: the date is
// "YYYY-MM-DD", the time "HH:MM", minute-granular, and a day rollover changes
// the date exactly once. The system-zone seam itself is exercised through the
// transcript tests in om_integration.
#[test]
fn entry_stamp_shapes_are_exact_in_a_fixed_zone() {
    let at = |secs: i64| Timestamp::from_second(secs).unwrap();
    // 2023-11-14 22:13:20 UTC
    let (d, t) = entry_stamp_in(TimeZone::UTC, at(1_700_000_000));
    assert_eq!((d.as_str(), t.as_str()), ("2023-11-14", "22:13"));
    // day rollover: 23:59:59 -> 00:00:00 the next day (2023-11-15 00:00 UTC)
    let (d, t) = entry_stamp_in(TimeZone::UTC, at(1_700_006_399));
    assert_eq!((d.as_str(), t.as_str()), ("2023-11-14", "23:59"));
    let (d, t) = entry_stamp_in(TimeZone::UTC, at(1_700_006_400));
    assert_eq!((d.as_str(), t.as_str()), ("2023-11-15", "00:00"));
    // leap day (the fragile civil axis) and pre-1970 (negative):
    let (d, t) = entry_stamp_in(TimeZone::UTC, at(1_709_164_800));
    assert_eq!((d.as_str(), t.as_str()), ("2024-02-29", "00:00"));
    let (d, t) = entry_stamp_in(TimeZone::UTC, at(-1));
    assert_eq!((d.as_str(), t.as_str()), ("1969-12-31", "23:59"));
}
