//! Recovery: the only part of Khodz Recovery that writes files.
//!
//! Safety rules enforced here:
//! * Output goes only into a new session folder inside a destination the user chose; every path
//!   component taken from the scanned medium is sanitised so nothing can escape that folder.
//! * Files are created with `create_new` — an existing file is never overwritten.
//! * A live device is never recovered onto the same physical disk unless the caller passes an
//!   explicit, recorded override ([`check_destination`]).
//! * Every file's SHA-256 is computed while writing and re-checked by reading the written file
//!   back; the outcome says exactly what is known (verified / mismatch / partial source / failed).

#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

mod destination;
mod names;
mod platform;
mod writer;

pub use destination::{DestinationCheck, SourceRef, check_destination};
pub use names::{output_relative_path, sanitize_component, session_dir_name};
pub use platform::{disks_for_path, free_space};
pub use writer::{
    ItemResult, Outcome, RecoverItem, RecoveryProgress, RecoverySummary, create_session_dir, recover,
};
