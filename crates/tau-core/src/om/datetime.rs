//! Local date/time for the OM prompts (ticket #183), via jiff. jiff is
//! already a transitive dependency (pulled in by gix), so this adds no new
//! crate and no hand-rolled civil-from-days / `localtime_r` FFI.

use jiff::Timestamp;
use jiff::fmt::strtime;
use jiff::tz::TimeZone;

/// Format `now` in `tz` as "YYYY-MM-DD HH:MM" (jiff). Shared by the local
/// production path and the fixed-zone (UTC) test path.
pub(crate) fn format_in(tz: TimeZone, now: Timestamp) -> String {
    let zoned = now.to_zoned(tz);
    // A fixed "%Y-%m-%d %H:%M" spec cannot fail for a valid instant; the
    // fallback keeps this panic-free (AGENTS.md: no unwrap in production).
    strtime::format("%Y-%m-%d %H:%M", &zoned).unwrap_or_else(|_| "unknown".to_owned())
}

/// The current local date/time as "YYYY-MM-DD HH:MM" — what the Observer and
/// Reflector are told (ticket #183), so observations are stamped with the real
/// date rather than a hallucinated one.
#[must_use]
pub fn format_local_datetime(now: Timestamp) -> String {
    format_in(TimeZone::system(), now)
}

/// The line prepended to an Observer/Reflector user message (ticket #183).
#[must_use]
pub fn datetime_preamble(now: Timestamp) -> String {
    format!(
        "Current date and time (local): {}\n\n",
        format_local_datetime(now)
    )
}

/// The dated user message: [`datetime_preamble`] over the raw content. Used at
/// every Observer/Reflector call site.
#[must_use]
pub fn dated_user_content(base: &str, now: Timestamp) -> String {
    let mut out = datetime_preamble(now);
    out.push_str(base);
    out
}
