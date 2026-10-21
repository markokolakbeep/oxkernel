use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::arch::csr;
use crate::task::cpu::mycpu;

/// Disables interrupts, counting nesting depth. The interrupt state of the
/// outermost call is restored by the matching last `pop_off`.
pub fn push_off() {
    let was_on = csr::intr_get();
    csr::intr_off();
    let cpu = mycpu();
    if cpu.noff.get() == 0 {
        cpu.intena.set(was_on);
    }
    cpu.noff.set(cpu.noff.get() + 1);
}

pub fn pop_off() {
    assert!(!csr::intr_get(), "pop_off: interrupts enabled");
    let cpu = mycpu();
    let n = cpu.noff.get();
    assert!(n >= 1, "pop_off: not pushed");
    cpu.noff.set(n - 1);
    if n == 1 && cpu.intena.get() {
        csr::intr_on();
    }
}

pub struct SpinLock<T: ?Sized> {
    locked: AtomicBool,
    /// Hart id + 1 of the holder, 0 when free. Used to catch self-deadlock.
    owner: AtomicUsize,
    name: &'static str,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for SpinLock<T> {}
unsafe impl<T: ?Sized + Send> Send for SpinLock<T> {}

pub struct SpinLockGuard<'a, T: ?Sized> {
    lock: &'a SpinLock<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(name: &'static str, data: T) -> Self {
        Self { locked: AtomicBool::new(false), owner: AtomicUsize::new(0), name, data: UnsafeCell::new(data) }
    }
}

impl<T: ?Sized> SpinLock<T> {
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        push_off();
        if self.holding() {
            panic!("spinlock {}: recursive acquire", self.name);
        }
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        self.owner.store(crate::arch::hartid() + 1, Ordering::Relaxed);
        SpinLockGuard { lock: self }
    }

    /// Whether this hart holds the lock. Only meaningful with interrupts off.
    pub fn holding(&self) -> bool {
        self.locked.load(Ordering::Relaxed) && self.owner.load(Ordering::Relaxed) == crate::arch::hartid() + 1
    }

    /// Releases a lock whose guard was forgotten, for handing a lock across a
    /// context switch.
    ///
    /// # Safety
    /// This hart must hold the lock and no live guard may exist.
    pub unsafe fn force_unlock(&self) {
        assert!(self.holding(), "spinlock {}: force_unlock without holding", self.name);
        self.owner.store(0, Ordering::Relaxed);
        self.locked.store(false, Ordering::Release);
        pop_off();
    }

    /// Rebuilds a guard for a lock this hart holds without one.
    ///
    /// # Safety
    /// As for [`force_unlock`](Self::force_unlock).
    pub unsafe fn assume_locked(&self) -> SpinLockGuard<'_, T> {
        assert!(self.holding(), "spinlock {}: assume_locked without holding", self.name);
        SpinLockGuard { lock: self }
    }
}

impl<'a, T: ?Sized> SpinLockGuard<'a, T> {
    pub fn lock_ref(&self) -> &'a SpinLock<T> {
        self.lock
    }
}

impl<T: ?Sized> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.owner.store(0, Ordering::Relaxed);
        self.lock.locked.store(false, Ordering::Release);
        pop_off();
    }
}
