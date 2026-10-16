//! Session time: the only clock the core knows. The runtime stamps every input with it.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Milliseconds since the session started, on the runtime's monotonic clock (LLD §2.1).
///
/// Arithmetic saturates instead of wrapping: time never runs backwards in the core, and a deadline
/// past the end of the range means "never".
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS,
)]
#[serde(transparent)]
#[ts(export)]
pub struct SessionTime(#[ts(type = "number")] u64);

impl SessionTime {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn from_ms(ms: u64) -> Self {
        Self(ms)
    }

    #[must_use]
    pub const fn as_ms(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn plus_ms(self, ms: u64) -> Self {
        Self(self.0.saturating_add(ms))
    }

    /// `ms` before `self`, or zero.
    #[must_use]
    pub const fn minus_ms(self, ms: u64) -> Self {
        Self(self.0.saturating_sub(ms))
    }

    /// Milliseconds from `earlier` to `self`, or zero when `earlier` is not earlier.
    #[must_use]
    pub const fn ms_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

#[cfg(test)]
mod tests {
    use super::SessionTime;

    #[test]
    fn arithmetic_saturates_instead_of_wrapping() {
        assert_eq!(SessionTime::from_ms(u64::MAX).plus_ms(1).as_ms(), u64::MAX);
        assert_eq!(SessionTime::from_ms(5).ms_since(SessionTime::from_ms(9)), 0);
        assert_eq!(SessionTime::from_ms(5).minus_ms(9), SessionTime::ZERO);
        assert_eq!(SessionTime::from_ms(9).ms_since(SessionTime::from_ms(5)), 4);
    }
}
