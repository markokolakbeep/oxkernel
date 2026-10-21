//! Powering off QEMU through its SiFive test device, which also sets QEMU's
//! exit status. That is how the test runner learns the result.

use crate::config::TEST_DEVICE;
use crate::mm::phys_to_virt;

const PASS: u32 = 0x5555;
const FAIL: u32 = 0x3333;

pub fn shutdown(code: u32) -> ! {
    let v = if code == 0 { PASS } else { (code << 16) | FAIL };
    unsafe { (phys_to_virt(TEST_DEVICE) as *mut u32).write_volatile(v) };
    crate::arch::sbi::shutdown()
}
