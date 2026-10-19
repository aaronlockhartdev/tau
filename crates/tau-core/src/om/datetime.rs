//! Local date/time for the OM prompts (ticket #183), via jiff. jiff is
//! already a transitive dependency (pulled in by gix), so this adds no new
//! crate and no hand-rolled civil-from-days / `localtime_r` FFI.

use jiff::Timestamp;
use jiff::fmt::strtime;
use jiff::tz::TimeZone;

/// The entry's local stamp in `tz`: `(date, time)` as `("YYYY-MM-DD",
/// "HH:MM")` (jiff). The zone parameter is a test seam — production calls
/// [`entry_stamp`].
pub(crate) fn entry_stamp_in(tz: TimeZone, ts: Timestamp) -> (String, String) {
    let zoned = ts.to_zoned(tz);
    // Fixed "%Y-%m-%d" / "%H:%M" specs cannot fail for a valid instant; the
    // fallback keeps this panic-free (AGENTS.md: no unwrap in production).
    let date = strtime::format("%Y-%m-%d", &zoned).unwrap_or_else(|_| "unknown".to_owned());
    let time = strtime::format("%H:%M", &zoned).unwrap_or_else(|_| "--:--".to_owned());
    (date, time)
}

/// The entry's local `(date, time)` stamp (system zone) — what
/// [`transcript`](super::super::om_integration) puts on each line and date
/// rollover (ticket #196).
pub(crate) fn entry_stamp(ts: Timestamp) -> (String, String) {
    entry_stamp_in(TimeZone::system(), ts)
}
