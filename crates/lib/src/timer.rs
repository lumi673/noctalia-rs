//! Opaque ids for timers scheduled by a Rust handler.
//!
//! The Luau entry stores the same values as numbers. Keeping separate Rust
//! types prevents a timeout handle from being passed where an interval handle
//! is expected.

use std::sync::atomic::{AtomicU64, Ordering};

/// A one-shot timer scheduled with [`crate::Context::set_timeout`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TimerHandle(u64);

/// A repeating timer scheduled with [`crate::Context::set_interval`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IntervalHandle(u64);

impl From<u64> for TimerHandle {
    #[inline]
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<u64> for IntervalHandle {
    #[inline]
    fn from(value: u64) -> Self {
        Self(value)
    }
}
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

impl TimerHandle {
    #[inline]
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl IntervalHandle {
    #[inline]
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[inline]
pub(crate) fn next_id() -> u64 {
    NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}
