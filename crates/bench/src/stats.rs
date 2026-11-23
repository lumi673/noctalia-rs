use serde::Serialize;

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Phase {
    pub surface: &'static str,
    pub samples: usize,
    pub warmup: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Summary {
    pub surface: &'static str,
    pub samples: usize,
    pub warmup: usize,
    pub median_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub min_us: f64,
    pub max_us: f64,
}

impl Summary {
    pub fn new(phase: Phase, mut times: Vec<Duration>) -> Self {
        if times.is_empty() {
            return Self {
                surface: phase.surface,
                samples: phase.samples,
                warmup: phase.warmup,
                median_us: 0.0,
                p95_us: 0.0,
                p99_us: 0.0,
                min_us: 0.0,
                max_us: 0.0,
            };
        }
        times.sort();
        let micros = |value: Duration| value.as_secs_f64() * 1_000_000.0;
        let len = times.len();
        let median_us = if len.is_multiple_of(2) {
            (micros(times[len / 2 - 1]) + micros(times[len / 2])) / 2.0
        } else {
            micros(times[len / 2])
        };
        Self {
            surface: phase.surface,
            samples: phase.samples,
            warmup: phase.warmup,
            median_us,
            p95_us: percentile(&times, 0.95),
            p99_us: percentile(&times, 0.99),
            min_us: micros(times[0]),
            max_us: micros(times[len - 1]),
        }
    }
}

fn percentile(times: &[Duration], fraction: f64) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    let rank = (fraction * times.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(times.len() - 1);
    times[index].as_secs_f64() * 1_000_000.0
}
