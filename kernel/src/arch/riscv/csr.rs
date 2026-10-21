//! Control and status register access.

macro_rules! csr_read {
    ($name:literal) => {{
        let v: usize;
        unsafe { core::arch::asm!(concat!("csrr {}, ", $name), out(reg) v, options(nomem, nostack)) };
        v
    }};
}

macro_rules! csr_write {
    ($name:literal, $v:expr) => {
        unsafe { core::arch::asm!(concat!("csrw ", $name, ", {}"), in(reg) $v, options(nostack)) }
    };
}

macro_rules! csr_set {
    ($name:literal, $v:expr) => {
        unsafe { core::arch::asm!(concat!("csrs ", $name, ", {}"), in(reg) $v, options(nostack)) }
    };
}

macro_rules! csr_clear {
    ($name:literal, $v:expr) => {
        unsafe { core::arch::asm!(concat!("csrc ", $name, ", {}"), in(reg) $v, options(nostack)) }
    };
}

pub const SSTATUS_SIE: usize = 1 << 1;
pub const SSTATUS_SPIE: usize = 1 << 5;
pub const SSTATUS_SPP: usize = 1 << 8;
pub const SSTATUS_FS: usize = 3 << 13;

pub const SIE_SSIE: usize = 1 << 1;
pub const SIE_STIE: usize = 1 << 5;
pub const SIE_SEIE: usize = 1 << 9;

pub const SCAUSE_INTERRUPT: usize = 1 << 63;

pub fn sstatus() -> usize {
    csr_read!("sstatus")
}

pub fn intr_on() {
    csr_set!("sstatus", SSTATUS_SIE)
}

pub fn intr_off() {
    csr_clear!("sstatus", SSTATUS_SIE)
}

pub fn intr_get() -> bool {
    sstatus() & SSTATUS_SIE != 0
}

pub fn set_sie(bits: usize) {
    csr_set!("sie", bits)
}

pub fn set_stvec(addr: usize) {
    csr_write!("stvec", addr)
}

pub fn set_sscratch(v: usize) {
    csr_write!("sscratch", v)
}

pub fn scause() -> usize {
    csr_read!("scause")
}

pub fn stval() -> usize {
    csr_read!("stval")
}

/// Switches address space and flushes the TLB.
pub fn set_satp(v: usize) {
    csr_write!("satp", v);
    super::sfence_vma_all();
}

/// Lets U-mode read `cycle`, `time` and `instret`.
pub fn enable_user_counters() {
    csr_write!("scounteren", 0x7usize)
}

pub fn time() -> u64 {
    csr_read!("time") as u64
}
