//! Luau-compatible `math.random` state.
//!
//! Luau uses a 32-bit PCG generator with multiplier 6364136223846793005 and
//! increment 105. This port keeps the same state transitions and range
//! arithmetic, so seeded Rust handlers reproduce a Luau script's random
//! sequence exactly.
#![expect(
    clippy::cast_possible_truncation,
    reason = "Luau narrows math arguments to int32"
)]
#![expect(
    clippy::cast_sign_loss,
    reason = "Luau reinterprets int32 seeds as unsigned words"
)]
#![expect(
    clippy::cast_precision_loss,
    reason = "Luau converts a 64-bit PCG word to f64"
)]
#![expect(
    clippy::shadow_reuse,
    reason = "Luau argument conversion reuses the source names"
)]

use std::process;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::clock;

const MULTIPLIER: u64 = 6_364_136_223_846_793_005;
const INCREMENT: u64 = 0x69 | 0x1;

/// A deterministic Luau PCG32 stream.
#[derive(Clone, Debug)]
pub struct LuauRng {
    state: u64,
}

impl LuauRng {
    /// Seeds a stream with Luau's `math.randomseed` argument conversion.
    #[inline]
    #[must_use]
    pub const fn seed(seed: i64) -> Self {
        Self::from_u64(seed as i32 as u64)
    }

    /// Seeds a stream with the raw 64-bit value used by the C implementation.
    #[must_use]
    pub const fn from_u64(seed: u64) -> Self {
        let mut state = 0_u64;
        let _ = Self::next_u32_from(&mut state);
        state = state.wrapping_add(seed);
        let _ = Self::next_u32_from(&mut state);
        Self { state }
    }

    /// Creates a stream seeded from process identity, wall time and clock,
    /// mirroring the inputs Luau mixes when it opens the math library.
    #[must_use]
    pub fn from_process() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
        let seed = u64::from(process::id()).rotate_left(32)
            ^ clock::unix_seconds().unsigned_abs()
            ^ clock::os_clock().to_bits()
            ^ sequence.rotate_left(21);
        Self::from_u64(seed)
    }

    /// Returns the current internal state.
    #[inline]
    #[must_use]
    pub const fn state(&self) -> u64 {
        self.state
    }

    /// Returns Luau's next floating point value in `[0, 1)`.
    #[must_use]
    pub fn next_f64(&mut self) -> f64 {
        let low = u64::from(Self::next_u32_from(&mut self.state));
        let high = u64::from(Self::next_u32_from(&mut self.state));
        let bits = low | (high << 32);
        (bits as f64) * 2_f64.powi(-64)
    }

    /// Returns Luau's `math.random(n)` result.
    pub fn below(&mut self, upper: i64) -> Option<i64> {
        let upper = upper as i32;
        if upper < 1 {
            return None;
        }
        let product = u64::from(upper as u32)
            .wrapping_mul(u64::from(Self::next_u32_from(&mut self.state)));
        let value = 1_i64.wrapping_add(i64::from((product >> 32) as u32));
        (value <= i64::from(upper)).then_some(value)
    }

    /// Returns Luau's `math.random(low, high)` result.
    pub fn range(&mut self, low: i64, high: i64) -> Option<i64> {
        let low = low as i32;
        let high = high as i32;
        if low > high {
            return None;
        }
        let span = (high as u32).wrapping_sub(low as u32);
        if span == u32::MAX {
            return None;
        }
        let product = u64::from(span.wrapping_add(1))
            .wrapping_mul(u64::from(Self::next_u32_from(&mut self.state)));
        let value = low.wrapping_add((product >> 32) as i32);
        Some(i64::from(value))
    }

    const fn next_u32_from(state: &mut u64) -> u32 {
        let old = *state;
        *state = old.wrapping_mul(MULTIPLIER).wrapping_add(INCREMENT);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rotation = (old >> 59) as u32;
        xorshifted >> rotation | xorshifted << (rotation.wrapping_neg() & 31)
    }
}

impl Default for LuauRng {
    #[inline]
    fn default() -> Self {
        Self::from_process()
    }
}

static GLOBAL: OnceLock<Mutex<LuauRng>> = OnceLock::new();

#[inline]
fn global() -> &'static Mutex<LuauRng> {
    GLOBAL.get_or_init(|| Mutex::new(LuauRng::from_process()))
}

/// Applies Luau's `math.randomseed` to the process-wide stream.
pub fn seed_global(seed: i64) {
    let mut rng = global().lock().unwrap_or_else(PoisonError::into_inner);
    *rng = LuauRng::seed(seed);
}

/// Returns the next process-wide `math.random()` value.
#[must_use]
pub fn global_f64() -> f64 {
    let mut rng = global().lock().unwrap_or_else(PoisonError::into_inner);
    rng.next_f64()
}

/// Returns the next process-wide `math.random(n)` value.
#[must_use]
pub fn global_below(upper: i64) -> Option<i64> {
    let mut rng = global().lock().unwrap_or_else(PoisonError::into_inner);
    rng.below(upper)
}

/// Returns the next process-wide `math.random(low, high)` value.
#[must_use]
pub fn global_range(low: i64, high: i64) -> Option<i64> {
    let mut rng = global().lock().unwrap_or_else(PoisonError::into_inner);
    rng.range(low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_luau_seed_sequence() {
        let mut rng = LuauRng::seed(42);
        assert_eq!(rng.below(10), Some(6));
        assert_eq!(rng.below(10), Some(3));
        assert_eq!(rng.below(10), Some(1));
    }

    #[test]
    fn reseeding_replays_the_same_stream() {
        let first = {
            let mut rng = LuauRng::seed(7);
            (0..6).map(|_| rng.next_f64()).collect::<Vec<_>>()
        };
        let second = {
            let mut rng = LuauRng::seed(7);
            (0..6).map(|_| rng.next_f64()).collect::<Vec<_>>()
        };
        assert_eq!(first, second);
        assert!(first.iter().all(|value| (0.0..1.0).contains(value)));
    }

    #[test]
    fn range_rejects_empty_intervals() {
        let mut rng = LuauRng::seed(1);
        assert_eq!(rng.range(3, 2), None);
        assert_eq!(rng.below(0), None);
        assert_eq!(rng.range(-3, 3), Some(3));
    }

    #[test]
    fn i32_seed_wrap_matches_luau() {
        let wide = LuauRng::seed(i64::from(u32::MAX));
        let narrow = LuauRng::seed(-1);
        assert_eq!(wide.state(), narrow.state());
    }
}
