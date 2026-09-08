//! Wall-clock time for the control plane.
//!
//! Everything is UTC and RFC 3339 on the wire. Tests inject a fixed clock rather
//! than reading the system clock, so time-dependent behaviour is deterministic.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A UTC instant, serialised as RFC 3339.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(#[serde(with = "time::serde::rfc3339")] OffsetDateTime);

impl Timestamp {
    /// The current instant, truncated to microseconds.
    pub fn now() -> Self {
        Self::from_offset(OffsetDateTime::now_utc())
    }

    /// Wrap an existing instant, normalising to UTC and to microseconds.
    ///
    /// Truncation is not cosmetic. PostgreSQL `timestamptz` stores
    /// microseconds, so a nanosecond-precision value does not survive a
    /// round-trip. Without this, a progress checkpoint written and read back
    /// would compare as *newer* than what was stored, and the idempotent merge
    /// would report a change on every replay.
    pub fn from_offset(value: OffsetDateTime) -> Self {
        let utc = value.to_offset(time::UtcOffset::UTC);
        let micros = utc.nanosecond() / 1_000 * 1_000;
        Self(utc.replace_nanosecond(micros).unwrap_or(utc))
    }

    /// The underlying instant.
    pub const fn as_offset(&self) -> OffsetDateTime {
        self.0
    }

    /// Unix seconds.
    pub fn unix_seconds(&self) -> i64 {
        self.0.unix_timestamp()
    }

    /// This instant shifted by `seconds`.
    pub fn plus_seconds(&self, seconds: i64) -> Self {
        Self(self.0 + time::Duration::seconds(seconds))
    }

    /// Whether `self` is within `seconds` before `reference`.
    ///
    /// Used by the Recent feed, which shows a rolling 24-hour window.
    pub fn within_seconds_before(&self, reference: Timestamp, seconds: i64) -> bool {
        let delta = reference.0 - self.0;
        delta >= time::Duration::ZERO && delta <= time::Duration::seconds(seconds)
    }
}

impl std::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self
            .0
            .format(&time::format_description::well_known::Rfc3339)
        {
            Ok(s) => f.write_str(&s),
            Err(_) => f.write_str("<unformattable timestamp>"),
        }
    }
}

/// A monotonically increasing clock source.
///
/// Production uses [`SystemClock`]. Tests use [`FixedClock`] so that expiry and
/// windowing logic is exercised without sleeping.
pub trait Clock: Send + Sync + 'static {
    /// The current instant according to this clock.
    fn now(&self) -> Timestamp;
}

/// The real system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that returns a fixed instant, for tests.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub Timestamp);

impl Clock for FixedClock {
    fn now(&self) -> Timestamp {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_as_rfc3339() {
        let ts =
            Timestamp::from_offset(OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap());
        let json = serde_json::to_string(&ts).unwrap();
        assert_eq!(json, "\"2023-11-14T22:13:20Z\"");
        assert_eq!(serde_json::from_str::<Timestamp>(&json).unwrap(), ts);
    }

    #[test]
    fn recent_window_is_inclusive_and_rejects_the_future() {
        let now =
            Timestamp::from_offset(OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap());
        let day = 86_400;

        assert!(now.plus_seconds(-1).within_seconds_before(now, day));
        assert!(now.plus_seconds(-day).within_seconds_before(now, day));
        assert!(!now.plus_seconds(-day - 1).within_seconds_before(now, day));
        // A future-dated event is not "recent"; it is a clock-skew bug.
        assert!(!now.plus_seconds(1).within_seconds_before(now, day));
    }

    #[test]
    fn precision_is_microseconds_so_a_postgres_round_trip_is_lossless() {
        let nanos = OffsetDateTime::from_unix_timestamp_nanos(1_700_000_000_123_456_789).unwrap();
        let ts = Timestamp::from_offset(nanos);
        assert_eq!(ts.as_offset().nanosecond(), 123_456_000);
        // Re-wrapping is a no-op: truncation is idempotent.
        assert_eq!(Timestamp::from_offset(ts.as_offset()), ts);
        assert_eq!(Timestamp::now().as_offset().nanosecond() % 1_000, 0);
    }

    #[test]
    fn fixed_clock_does_not_advance() {
        let clock = FixedClock(Timestamp::now());
        assert_eq!(clock.now(), clock.now());
    }
}
