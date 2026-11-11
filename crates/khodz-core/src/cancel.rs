use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Error, Result};

/// Cooperative cancellation flag shared between the UI thread and worker threads.
///
/// Workers call [`CancelToken::check`] between chunks; cancellation is therefore bounded by the
/// time it takes to process one chunk.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Returns `Err(Error::Cancelled)` once cancellation has been requested.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_state_across_threads() {
        let token = CancelToken::new();
        assert!(token.check().is_ok());
        let worker = token.clone();
        std::thread::spawn(move || worker.cancel()).join().unwrap();
        assert!(token.is_cancelled());
        assert!(matches!(token.check(), Err(Error::Cancelled)));
    }
}
