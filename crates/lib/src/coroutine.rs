//! Coroutine scheduling with an `os.clock()` budget.
//!
//! A Rust handler can keep the straight-line control flow of a Luau coroutine
//! by calling [`yield_now`] inside a spawned [`Coroutine`]. The driver resumes
//! suspended coroutines until a [`ClockBudget`] expires. Dropping a coroutine
//! stops it at its next yield, the same way the shell discards a Luau
//! coroutine it never resumes again.

use std::any::Any;
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::clock::ClockBudget;

enum Signal {
    Yield(Box<dyn Any + Send>),
    Done,
}

struct Current {
    yielded: Sender<Signal>,
    resumed: Receiver<()>,
}

thread_local! {
    static CURRENT: RefCell<Option<Current>> = const { RefCell::new(None) };
}

/// Yields the running coroutine until its driver resumes it.
pub fn yield_now() {
    yield_value(());
}

/// Yields a value to the driver and waits to be resumed.
///
/// Calling this outside a coroutine yields the current thread instead.
pub fn yield_value<T: Send + 'static>(value: T) {
    CURRENT.with(|cell| {
        let binding = cell.borrow();
        let Some(current) = binding.as_ref() else {
            thread::yield_now();
            return;
        };
        let _ = current.yielded.send(Signal::Yield(Box::new(value)));
        assert!(
            current.resumed.recv().is_ok(),
            "coroutine stopped: driver dropped"
        );
    });
}

/// State reported by [`Coroutine::poll`].
pub enum Poll {
    /// The body is still running.
    Running,
    /// The body yielded a boxed value.
    Suspended(Box<dyn Any + Send>),
    /// The body finished.
    Ready,
}

/// A resumable Luau-style coroutine running on its own thread.
pub struct Coroutine<T> {
    signals: Receiver<Signal>,
    resume: Option<Sender<()>>,
    handle: Option<JoinHandle<Option<T>>>,
}

impl<T: Send + 'static> Coroutine<T> {
    /// Starts `body` as a suspended coroutine driver.
    #[must_use]
    #[expect(
        clippy::excessive_nesting,
        reason = "thread-local coroutine setup"
    )]
    pub fn spawn<F>(body: F) -> Self
    where
        F: FnOnce() -> T + Send + 'static,
    {
        let (yielded, signals) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let handle = thread::spawn(move || {
            CURRENT.with(|cell| {
                *cell.borrow_mut() = Some(Current { yielded, resumed });
            });
            let value = panic::catch_unwind(AssertUnwindSafe(body)).ok();
            CURRENT.with(|cell| {
                if let Some(current) = cell.borrow_mut().take() {
                    let _ = current.yielded.send(Signal::Done);
                }
            });
            value
        });
        Self {
            signals,
            resume: Some(resume),
            handle: Some(handle),
        }
    }

    /// Checks whether the coroutine yielded, finished, or is still running.
    #[must_use]
    pub fn poll(&self) -> Poll {
        match self.signals.try_recv() {
            Ok(Signal::Yield(value)) => Poll::Suspended(value),
            Ok(Signal::Done) | Err(mpsc::TryRecvError::Disconnected) => {
                Poll::Ready
            }
            Err(mpsc::TryRecvError::Empty) => Poll::Running,
        }
    }

    /// Resumes a suspended coroutine.
    pub fn resume(&self) {
        if let Some(resume) = self.resume.as_ref() {
            let _ = resume.send(());
        }
    }

    /// Waits for the finished body's return value.
    #[must_use]
    pub fn join(mut self) -> Option<T> {
        let handle = self.handle.take()?;
        handle.join().ok().flatten()
    }
}

impl<T> Drop for Coroutine<T> {
    fn drop(&mut self) {
        drop(self.resume.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Drives a coroutine until it finishes or its clock budget expires.
#[must_use]
pub fn run_budgeted<T: Send + 'static>(
    coroutine: Coroutine<T>,
    seconds: f64,
) -> Option<T> {
    let budget = ClockBudget::new(seconds);
    loop {
        match coroutine.poll() {
            Poll::Suspended(_) if !budget.expired() => coroutine.resume(),
            Poll::Suspended(_) => return None,
            Poll::Ready => return coroutine.join(),
            Poll::Running if budget.expired() => return None,
            Poll::Running => thread::yield_now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    #[expect(clippy::excessive_nesting, reason = "coroutine driver test")]
    fn coroutine_yields_and_resumes_in_order() {
        let steps = Arc::new(AtomicUsize::new(0));
        let worker = {
            let counter = Arc::clone(&steps);
            Coroutine::spawn(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                yield_now();
                counter.fetch_add(1, Ordering::SeqCst);
                yield_now();
                counter.fetch_add(1, Ordering::SeqCst);
                7
            })
        };
        assert_eq!(run_budgeted(worker, 1.0), Some(7));
        assert_eq!(steps.load(Ordering::SeqCst), 3);
    }

    #[test]
    #[expect(clippy::excessive_nesting, reason = "coroutine driver test")]
    fn yielded_values_reach_the_driver() {
        let coroutine = Coroutine::spawn(|| {
            yield_value(41_u32);
            0
        });
        let mut saw_value = false;
        let budget = ClockBudget::new(1.0);
        let mut value = None;
        while !budget.expired() {
            match coroutine.poll() {
                Poll::Suspended(payload) => {
                    value = payload.downcast::<u32>().ok().map(|value| *value);
                    saw_value = true;
                    coroutine.resume();
                    break;
                }
                Poll::Ready => break,
                Poll::Running => thread::yield_now(),
            }
        }
        assert!(saw_value);
        assert_eq!(value, Some(41));
        assert_eq!(coroutine.join(), Some(0));
    }

    #[test]
    fn expired_budget_stops_the_driver() {
        let coroutine = Coroutine::spawn(|| {
            yield_now();
            1
        });
        assert_eq!(run_budgeted(coroutine, 0.0), None);
    }
}
