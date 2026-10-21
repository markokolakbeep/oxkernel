//! Calls into the Supervisor Binary Interface (OpenSBI).

const EXT_TIME: usize = 0x5449_4d45;
const EXT_HSM: usize = 0x0048_534d;
const EXT_SRST: usize = 0x5352_5354;

fn call(ext: usize, fid: usize, a0: usize, a1: usize, a2: usize) -> (isize, usize) {
    let (err, val): (isize, usize);
    unsafe {
        core::arch::asm!(
            "ecall",
            inlateout("a0") a0 => err,
            inlateout("a1") a1 => val,
            in("a2") a2,
            in("a6") fid,
            in("a7") ext,
            options(nostack),
        );
    }
    (err, val)
}

/// Programs this hart's next timer interrupt.
pub fn set_timer(deadline: u64) {
    call(EXT_TIME, 0, deadline as usize, 0, 0);
}

/// Starts a stopped hart at physical address `start` with `a1 = opaque`.
pub fn hart_start(hartid: usize, start: usize, opaque: usize) -> isize {
    call(EXT_HSM, 0, hartid, start, opaque).0
}

/// Powers the machine off. Only used when the test device is unavailable.
pub fn shutdown() -> ! {
    call(EXT_SRST, 0, 0, 0, 0);
    loop {
        super::wfi();
    }
}

const EXT_IPI: usize = 0x0073_5049;

/// Raises a supervisor software interrupt on the harts in `mask`
/// (relative to `base`).
pub fn send_ipi(mask: usize, base: usize) {
    call(EXT_IPI, 0, mask, base, 0);
}
