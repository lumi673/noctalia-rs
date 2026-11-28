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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_monotonic() {
        let first = os_clock();
        let second = os_clock();
        assert!(second >= first);
    }
}
