//! RISC-V specifics: CSR access, SBI calls and the assembly entry points.

pub mod csr;
pub mod sbi;

core::arch::global_asm!(include_str!("entry.S"));
core::arch::global_asm!(include_str!("trap.S"));
core::arch::global_asm!(include_str!("switch.S"));

/// Id of the hart we are running on. `tp` holds it in S-mode.
#[inline(always)]
pub fn hartid() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id, options(nomem, nostack, preserves_flags)) };
    id
}

#[inline(always)]
pub fn wfi() {
    unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
}

/// Flushes this hart's whole TLB.
#[inline(always)]
pub fn sfence_vma_all() {
    unsafe { core::arch::asm!("sfence.vma", options(nostack)) };
}
