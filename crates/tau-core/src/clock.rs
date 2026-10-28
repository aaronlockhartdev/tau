//! The wall clock in the two units this repo stamps with (S5). `jiff` is
//! already a dependency; before this module each site repeated the same
//! `duration_since(UNIX_EPOCH)` dance, in two incompatible shapes.

use jiff::Timestamp;

/// A clock reading outside jiff's range (a set-back or wild RTC) yields no
/// stamp, which the readers below render as 0 rather than a panic: every
/// consumer here is a record stamp or a temp-file name, never a decision whose
/// correctness depends on the reading.
fn stamp_at(when: std::time::SystemTime) -> Option<Timestamp> {
    Timestamp::try_from(when).ok()
}

#[must_use]
pub(crate) fn now_ms() -> u64 {
    ms_at(std::time::SystemTime::now())
}

#[must_use]
pub(crate) fn now_ns() -> u128 {
    ns_at(std::time::SystemTime::now())
}

fn ms_at(when: std::time::SystemTime) -> u64 {
    stamp_at(when)
        .and_then(|t| u64::try_from(t.as_millisecond()).ok())
        .unwrap_or(0)
}

fn ns_at(when: std::time::SystemTime) -> u128 {
    stamp_at(when)
        .and_then(|t| u128::try_from(t.as_nanosecond()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clock_in_range_stamps_both_units_exactly() {
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        assert_eq!(ms_at(when), 1_700_000_000_000);
        assert_eq!(ns_at(when), 1_700_000_000_000_000_000);
    }

    #[test]
    fn a_clock_out_of_range_stamps_zero_not_a_panic() {
        // jiff spans the years ±9999; this is a billion years out.
        let wild = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1 << 40);
        assert_eq!(ms_at(wild), 0, "the stamp degrades");
        assert_eq!(ns_at(wild), 0, "both units degrade alike");
        let before_epoch = std::time::UNIX_EPOCH - std::time::Duration::from_secs(3_600);
        assert_eq!(ms_at(before_epoch), 0, "a pre-epoch clock is no stamp");
    }

    #[test]
    fn the_live_clock_is_current() {
        let floor = ms_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000));
        assert!(now_ms() > floor, "the wall clock reads after 2023");
        assert!(now_ns() >= u128::from(now_ms()) * 1_000_000);
    }
}
