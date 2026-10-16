//! Supervising the processes Termtrace starts and identifying the ones it observes.
//!
//! One place owns process-group signalling and terminate-with-grace, `/proc` process
//! identity that survives PID reuse, owner-private exclusive file locks, bounded
//! polling, and `PATH` executable lookup. It links no other Termtrace crate.

mod child;
mod group;
mod identity;
mod lock;
pub mod owned;
mod path;
mod poll;

pub use child::{SupervisedChild, Termination};
pub use group::ProcessGroup;
pub use identity::{ProcessIdentity, ProcessImage};
pub use lock::ExclusiveLock;
pub use path::{executables_on_path, find_executable, is_executable};
pub use poll::poll_until;
