//! Locks. Spinlocks keep interrupts off while held (xv6 style) so an
//! interrupt handler can never spin on a lock its own hart holds. Sleep locks
//! let the owner block, for example on disk I/O.

mod once;
mod sleeplock;
mod spin;

pub use once::Once;
pub use sleeplock::{SleepLock, SleepLockGuard};
pub use spin::{SpinLock, SpinLockGuard, pop_off, push_off};
