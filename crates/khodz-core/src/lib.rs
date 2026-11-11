//! Shared primitives for Khodz Recovery.
//!
//! This crate performs no I/O. It defines the error type, size/alignment helpers,
//! cooperative cancellation and progress throttling used by every engine crate.

pub mod bytes;
pub mod cancel;
pub mod error;
pub mod progress;
pub mod units;

pub use cancel::CancelToken;
pub use error::{Error, Result};
pub use progress::{Progress, ProgressThrottle};
pub use units::{ByteSize, align_down, align_up};
