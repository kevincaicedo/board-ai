//! Wall-clock time for the vault: session folder dates and `created` stamps. Session time
//! ([`crate::clock::Clock`]) is monotonic and starts at zero, so it can't date a file.
//!
//! Dates are UTC. Local time needs the time zone database or `localtime_r` (a dependency or
//! `unsafe`); a session started late in the evening can carry the next day's date. Civil-date
//! math is Howard Hinnant's `days_from_civil`, a dozen lines instead of a date crate.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_SECOND: i64 = 1_000;
const SECONDS_PER_DAY: i64 = 86_400;

/// Milliseconds since the Unix epoch, UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WallTime(i64);

/// Where wall-clock time comes from: the system, or a fake in tests.
pub trait WallClock: Send + Sync + fmt::Debug {
    fn now(&self) -> WallTime;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now(&self) -> WallTime {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        WallTime(i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX))
    }
}

/// A wall clock that moves only when told to. Clones share the same time.
#[derive(Clone, Debug, Default)]
pub struct FakeWallClock {
    unix_ms: Arc<AtomicI64>,
}

impl FakeWallClock {
    #[must_use]
    pub fn at(time: WallTime) -> Self {
        Self {
            unix_ms: Arc::new(AtomicI64::new(time.0)),
        }
    }

    pub fn advance_ms(&self, ms: i64) {
        self.unix_ms.fetch_add(ms, Ordering::AcqRel);
    }
}

impl WallClock for FakeWallClock {
    fn now(&self) -> WallTime {
        WallTime(self.unix_ms.load(Ordering::Acquire))
    }
}

/// A UTC calendar date and time of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Civil {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

impl WallTime {
    #[must_use]
    pub const fn from_unix_ms(unix_ms: i64) -> Self {
        Self(unix_ms)
    }

    #[must_use]
    pub const fn unix_ms(self) -> i64 {
        self.0
    }

    /// `2026-10-15`.
    #[must_use]
    pub fn date(self) -> String {
        let civil = self.civil();
        format!("{:04}-{:02}-{:02}", civil.year, civil.month, civil.day)
    }

    /// `143210`: the time of day, for session ids.
    #[must_use]
    pub fn time_of_day_compact(self) -> String {
        let civil = self.civil();
        format!("{:02}{:02}{:02}", civil.hour, civil.minute, civil.second)
    }

    /// `2026-10-15T14:32:10Z`.
    #[must_use]
    pub fn rfc3339(self) -> String {
        let civil = self.civil();
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            civil.year, civil.month, civil.day, civil.hour, civil.minute, civil.second
        )
    }

    /// Parses exactly the form [`WallTime::rfc3339`] writes. Anything else (a hand-edited
    /// stamp in another form) is `None`, and callers fall back.
    #[must_use]
    pub fn parse_rfc3339(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let shape_ok = bytes.len() == 20
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes[19] == b'Z';
        if !shape_ok {
            return None;
        }
        let number = |range: std::ops::Range<usize>| -> Option<u32> {
            let digits = text.get(range)?;
            digits
                .bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| digits.parse().ok())?
        };
        let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
        let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
        let valid = (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60;
        if !valid {
            return None;
        }
        let days = days_from_civil(i64::from(year), month, day);
        let seconds = days * SECONDS_PER_DAY
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second);
        Some(Self(seconds * MS_PER_SECOND))
    }

    fn civil(self) -> Civil {
        let seconds = self.0.div_euclid(MS_PER_SECOND);
        let days = seconds.div_euclid(SECONDS_PER_DAY);
        let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        // `second_of_day` is in [0, 86400), so these fit in u32.
        let second_of_day = u32::try_from(second_of_day).unwrap_or(0);
        Civil {
            year,
            month,
            day,
            hour: second_of_day / 3_600,
            minute: second_of_day % 3_600 / 60,
            second: second_of_day % 60,
        }
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::{WallTime, civil_from_days, days_from_civil};

    #[test]
    fn formats_known_instants() {
        // 2026-10-15T14:32:10Z
        let time = WallTime::from_unix_ms(1_792_074_730_000);
        assert_eq!(time.date(), "2026-10-15");
        assert_eq!(time.time_of_day_compact(), "143210");
        assert_eq!(time.rfc3339(), "2026-10-15T14:32:10Z");
        assert_eq!(WallTime::from_unix_ms(0).rfc3339(), "1970-01-01T00:00:00Z");
        assert_eq!(
            WallTime::from_unix_ms(951_782_400_000).date(),
            "2000-02-29",
            "leap day"
        );
    }

    #[test]
    fn rfc3339_round_trips_and_rejects_other_forms() {
        for unix_ms in [0, 951_782_400_000, 1_792_074_730_000, 4_102_444_799_000] {
            let time = WallTime::from_unix_ms(unix_ms);
            assert_eq!(WallTime::parse_rfc3339(&time.rfc3339()), Some(time));
        }
        for bad in [
            "",
            "2026-10-15",
            "2026-10-15T14:32:10+02:00",
            "2026-13-15T14:32:10Z",
            "2026-1a-15T14:32:10Z",
            "2026-10-15T24:00:00Z",
        ] {
            assert_eq!(WallTime::parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn civil_conversion_is_its_own_inverse() {
        for days in (-800_000..800_000).step_by(997) {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }
}
