//! Per-hart state, found through `tp` (the hart id).

use alloc::sync::Arc;
use core::cell::{Cell, UnsafeCell};
use core::sync::atomic::{AtomicUsize, Ordering};

use super::Task;
use super::context::Context;
use crate::config::MAX_HARTS;

pub struct Cpu {
    /// Depth of `push_off` nesting, and whether interrupts were on before the
    /// outermost one.
    pub noff: Cell<i32>,
    pub intena: Cell<bool>,
    /// The task running on this hart, if any.
    current: UnsafeCell<Option<Arc<Task>>>,
    /// Saved context of this hart's scheduler loop.
    pub context: UnsafeCell<Context>,
}

struct Cpus([Cpu; MAX_HARTS]);

// Each Cpu is only touched by its own hart, with interrupts off.
unsafe impl Sync for Cpus {}

static CPUS: Cpus = Cpus(
    [const {
        Cpu {
            noff: Cell::new(0),
            intena: Cell::new(false),
            current: UnsafeCell::new(None),
            context: UnsafeCell::new(Context::zero()),
        }
    }; MAX_HARTS],
);

/// Harts listed in the devicetree.
pub static HARTS: AtomicUsize = AtomicUsize::new(1);
/// Harts that have finished booting.
pub static ONLINE: AtomicUsize = AtomicUsize::new(0);
/// Bit i set while hart i sits idle in `wfi`.
pub static IDLE_MASK: AtomicUsize = AtomicUsize::new(0);
pub static BOOT_HART: AtomicUsize = AtomicUsize::new(0);

/// This hart's state. Interrupts must be off, or the caller must not care
/// about being moved to another hart.
pub fn mycpu() -> &'static Cpu {
    &CPUS.0[crate::arch::hartid()]
}

impl Cpu {
    pub fn current(&self) -> Option<&Arc<Task>> {
        unsafe { (*self.current.get()).as_ref() }
    }

    pub fn set_current(&self, t: Option<Arc<Task>>) {
        unsafe { *self.current.get() = t };
    }

    pub fn take_current(&self) -> Option<Arc<Task>> {
        unsafe { (*self.current.get()).take() }
    }
}

pub fn is_boot_hart() -> bool {
    crate::arch::hartid() == BOOT_HART.load(Ordering::Relaxed)
}
