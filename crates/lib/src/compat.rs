//! Helper-era APIs kept for `auto` and `off` plugins.
//!
//! Strict mode emits a standalone Luau entry. These items need the helper
//! process or a native thread, so strict plugins cannot use them. Old plugins
//! can keep importing them from `noctalia_rs::compat`.

use std::mem::take;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::clock::os_clock;
use crate::context::{Context, lock};
use crate::ops::Op;

pub mod coroutine;

pub use coroutine::{
    Coroutine, Poll as CoroutinePoll, run_budgeted, yield_now, yield_value,
};

/// Raw host snapshot assembled by the bridge helper.
pub struct Parts {
    pub entry: String,
    pub event: String,
    pub payload: Value,
    pub config: Value,
    pub state: Value,
    pub host: Value,
    pub data_dir: PathBuf,
}

/// Writes one op to the host as soon as a native query is issued.
pub type OpSink = Arc<dyn Fn(&Op) + Send + Sync>;

impl Context {
    /// Installs the immediate op writer used for synchronous host queries.
    #[doc(hidden)]
    pub fn set_op_sink(&mut self, sink: OpSink) {
        let mut current = lock(&self.op_sink);
        *current = Some(sink);
    }

    /// Drains ops buffered while no op sink is installed.
    #[doc(hidden)]
    #[must_use]
    pub fn take_pending_ops(&self) -> Vec<Op> {
        let mut pending = lock(&self.pending_ops);
        take(&mut pending)
    }
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
    fn budget_expires() {
        let budget = ClockBudget::new(0.0);
        assert!(budget.expired());
        assert_eq!(budget.remaining(), 0.0);
    }
}
