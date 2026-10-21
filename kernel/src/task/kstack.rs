//! Kernel stacks for tasks.
//!
//! All NPROC stacks are mapped once at boot in their own region, each with an
//! unmapped guard page below it. Mapping them up front means no page table
//! changes, and so no TLB shootdowns, ever happen in the kernel half.

use oxcore::PAGE_SIZE;

use crate::config::{KSTACK_PAGES, KSTACK_REGION, NPROC};
use crate::mm::frame;
use crate::mm::paging::{PTE_G, PTE_R, PTE_W, PageTable};
use crate::sync::SpinLock;

const STRIDE: usize = (KSTACK_PAGES + 1) * PAGE_SIZE;
const _: () = assert!(NPROC <= 64, "slot bitmap is a u64");

/// Bit i set when slot i is in use.
static SLOTS: SpinLock<u64> = SpinLock::new("kstack", 0);

pub struct KernelStack {
    slot: usize,
}

impl KernelStack {
    pub fn alloc() -> Option<Self> {
        let mut used = SLOTS.lock();
        let slot = (!*used).trailing_zeros() as usize;
        if slot >= NPROC {
            return None;
        }
        *used |= 1 << slot;
        Some(Self { slot })
    }

    pub fn top(&self) -> usize {
        KSTACK_REGION + self.slot * STRIDE + STRIDE
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        *SLOTS.lock() &= !(1 << self.slot);
    }
}

pub fn map_all(pt: &mut PageTable) {
    for slot in 0..NPROC {
        let base = KSTACK_REGION + slot * STRIDE + PAGE_SIZE; // skip the guard page
        for i in 0..KSTACK_PAGES {
            let pa = frame::alloc_frame().expect("no memory for kernel stacks");
            pt.map(base + i * PAGE_SIZE, pa, 0, PTE_R | PTE_W | PTE_G).unwrap();
        }
    }
}

/// Whether `addr` falls in a kernel stack guard page.
pub fn is_guard_page(addr: usize) -> bool {
    (KSTACK_REGION..KSTACK_REGION + NPROC * STRIDE).contains(&addr) && (addr - KSTACK_REGION) % STRIDE < PAGE_SIZE
}
