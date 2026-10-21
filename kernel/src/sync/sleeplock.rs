use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

use super::SpinLock;
use crate::task::{sleep, wakeup};

/// A lock whose waiters sleep instead of spinning. Must only be taken from
/// task context.
pub struct SleepLock<T> {
    locked: SpinLock<bool>,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SleepLock<T> {}
unsafe impl<T: Send> Send for SleepLock<T> {}

pub struct SleepLockGuard<'a, T> {
    lock: &'a SleepLock<T>,
}

impl<T> SleepLock<T> {
    pub const fn new(name: &'static str, data: T) -> Self {
        Self { locked: SpinLock::new(name, false), data: UnsafeCell::new(data) }
    }

    fn chan(&self) -> usize {
        self as *const _ as usize
    }

    pub fn lock(&self) -> SleepLockGuard<'_, T> {
        let mut held = self.locked.lock();
        while *held {
            held = sleep(self.chan(), held);
        }
        *held = true;
        SleepLockGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<SleepLockGuard<'_, T>> {
        let mut held = self.locked.lock();
        if *held {
            return None;
        }
        *held = true;
        Some(SleepLockGuard { lock: self })
    }
}

impl<T> Deref for SleepLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SleepLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SleepLockGuard<'_, T> {
    fn drop(&mut self) {
        let mut held = self.lock.locked.lock();
        *held = false;
        wakeup(self.lock.chan());
    }
}
