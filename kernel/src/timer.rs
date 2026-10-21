//! Timer: a tick every 1/TICK_HZ seconds on every hart, via SBI.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::{csr, sbi};
use crate::config::TICK_HZ;
use crate::sync::SpinLock;
use crate::task::{self, cpu::is_boot_hart};

static TIMEBASE_HZ: AtomicU64 = AtomicU64::new(10_000_000);
static TICKS: SpinLock<u64> = SpinLock::new("ticks", 0);

pub fn init(timebase_hz: u64) {
    if timebase_hz != 0 {
        TIMEBASE_HZ.store(timebase_hz, Ordering::Relaxed);
    }
}

pub fn timebase_hz() -> u64 {
    TIMEBASE_HZ.load(Ordering::Relaxed)
}

fn arm() {
    sbi::set_timer(csr::time() + timebase_hz() / TICK_HZ);
}

pub fn start_hart() {
    arm();
}

pub fn on_tick() {
    arm();
    if is_boot_hart() {
        let mut t = TICKS.lock();
        *t += 1;
        task::wakeup(&TICKS as *const _ as usize);
    }
}

pub fn now_us() -> u64 {
    (csr::time() as u128 * 1_000_000 / timebase_hz() as u128) as u64
}

/// Sleeps for at least `ms` milliseconds. Returns false if the task was
/// killed meanwhile.
pub fn sleep_ms(ms: u64) -> bool {
    let deadline = now_us() + ms * 1000;
    let me = task::current();
    let mut t = TICKS.lock();
    while now_us() < deadline {
        if me.is_killed() {
            return false;
        }
        t = task::sleep(&TICKS as *const _ as usize, t);
    }
    true
}
