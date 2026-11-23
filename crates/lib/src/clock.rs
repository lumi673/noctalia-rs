//! Clock accessors matching Luau's `os.clock`, `os.time` and `formatTime`.

use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static ORIGIN: OnceLock<Instant> = OnceLock::new();

#[inline]
fn origin() -> &'static Instant {
    ORIGIN.get_or_init(Instant::now)
}

/// Returns Luau's monotonic `os.clock()` value in seconds.
#[inline]
#[must_use]
pub fn os_clock() -> f64 {
    origin().elapsed().as_secs_f64()
}

/// Returns whole seconds since the Unix epoch, like Luau's `os.time()`.
#[inline]
#[must_use]
pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(i64::MAX, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

/// A deadline measured with the same monotonic clock as `os.clock`.
#[derive(Clone, Copy, Debug)]
pub struct ClockBudget {
    expires_at: f64,
}

impl ClockBudget {
    /// Starts a budget that expires after `seconds`.
    #[inline]
    #[must_use]
    pub fn new(seconds: f64) -> Self {
        Self {
            expires_at: os_clock() + seconds,
        }
    }

    /// Returns whether the budget has been consumed.
    #[inline]
    #[must_use]
    pub fn expired(&self) -> bool {
        os_clock() >= self.expires_at
    }

    /// Returns seconds remaining, never negative.
    #[inline]
    #[must_use]
    pub fn remaining(&self) -> f64 {
        (self.expires_at - os_clock()).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_monotonic() {
        let first = os_clock();
        let second = os_clock();
        assert!(second >= first);
    }

    #[test]
    fn budget_expires() {
        let budget = ClockBudget::new(0.0);
        assert!(budget.expired());
        assert_eq!(budget.remaining(), 0.0);
    }
}
