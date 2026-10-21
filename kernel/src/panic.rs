//! Panic handler: report, walk the frame-pointer chain, power off.

use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::{self, csr};
use crate::console::RawUart;

static PANICKING: AtomicBool = AtomicBool::new(false);

/// Prints return addresses by following saved frame pointers. With
/// `-C force-frame-pointers`, `fp - 8` holds `ra` and `fp - 16` the caller's
/// fp. Resolve the addresses with `cargo xtask addr2line`.
fn backtrace(out: &mut RawUart) {
    let mut fp: usize;
    unsafe { core::arch::asm!("mv {}, fp", out(reg) fp) };
    let _ = write!(out, "backtrace:");
    for _ in 0..32 {
        if fp < crate::config::KERNEL_OFFSET || !fp.is_multiple_of(8) {
            break;
        }
        let ra = unsafe { *((fp - 8) as *const usize) };
        let prev = unsafe { *((fp - 16) as *const usize) };
        if ra == 0 {
            break;
        }
        let _ = write!(out, " {ra:#x}");
        if prev <= fp {
            break;
        }
        fp = prev;
    }
    let _ = writeln!(out);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    csr::intr_off();
    if PANICKING.swap(true, Ordering::SeqCst) {
        // Another hart is already reporting.
        loop {
            arch::wfi();
        }
    }
    let mut out = RawUart;
    let _ = writeln!(out, "\n!!! kernel panic on hart {}: {}", arch::hartid(), info);
    backtrace(&mut out);
    crate::power::shutdown(1)
}
