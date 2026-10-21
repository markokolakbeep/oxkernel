use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

/// A value written exactly once during boot and read-only afterwards.
pub struct Once<T> {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<T>>,
}

unsafe impl<T: Send + Sync> Sync for Once<T> {}

const EMPTY: u8 = 0;
const BUSY: u8 = 1;
const READY: u8 = 2;

impl<T> Once<T> {
    pub const fn new() -> Self {
        Self { state: AtomicU8::new(EMPTY), value: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    pub fn set(&self, v: T) {
        if self.state.compare_exchange(EMPTY, BUSY, Ordering::Acquire, Ordering::Relaxed).is_err() {
            panic!("Once::set called twice");
        }
        unsafe { (*self.value.get()).write(v) };
        self.state.store(READY, Ordering::Release);
    }

    pub fn get(&self) -> Option<&T> {
        (self.state.load(Ordering::Acquire) == READY).then(|| unsafe { (*self.value.get()).assume_init_ref() })
    }

    pub fn wait(&self) -> &T {
        self.get().expect("Once used before initialisation")
    }
}
