use std::time::{Duration, Instant};

/// Snapshot of a long-running job's progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub items_found: u64,
}

impl Progress {
    /// Completion in `0.0..=1.0`. An unknown (zero) total reports 0.
    pub fn fraction(&self) -> f64 {
        if self.bytes_total == 0 {
            0.0
        } else {
            (self.bytes_done.min(self.bytes_total) as f64) / (self.bytes_total as f64)
        }
    }
}

/// Limits how often progress is reported so the UI is not flooded with events.
///
/// The first update and the final update (`done == total`) are always emitted.
#[derive(Debug)]
pub struct ProgressThrottle {
    interval: Duration,
    last: Option<Instant>,
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self { interval, last: None }
    }

    /// Returns true if an update at `now` should be emitted.
    pub fn should_emit(&mut self, now: Instant, progress: &Progress) -> bool {
        let finished = progress.bytes_total != 0 && progress.bytes_done >= progress.bytes_total;
        let due = match self.last {
            None => true,
            Some(last) => now.saturating_duration_since(last) >= self.interval,
        };
        if due || finished {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_is_clamped_and_handles_unknown_total() {
        let p = |done, total| Progress { bytes_done: done, bytes_total: total, items_found: 0 };
        assert_eq!(p(0, 0).fraction(), 0.0);
        assert_eq!(p(50, 100).fraction(), 0.5);
        assert_eq!(p(150, 100).fraction(), 1.0);
    }

    #[test]
    fn throttle_limits_rate_but_always_emits_completion() {
        let mut t = ProgressThrottle::new(Duration::from_millis(100));
        let start = Instant::now();
        let mid = Progress { bytes_done: 10, bytes_total: 100, items_found: 0 };
        let done = Progress { bytes_done: 100, bytes_total: 100, items_found: 0 };

        assert!(t.should_emit(start, &mid), "first update always emitted");
        assert!(!t.should_emit(start + Duration::from_millis(50), &mid));
        assert!(t.should_emit(start + Duration::from_millis(60), &done), "completion always emitted");
        assert!(t.should_emit(start + Duration::from_millis(200), &mid));
    }
}
