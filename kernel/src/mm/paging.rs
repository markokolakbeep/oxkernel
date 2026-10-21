//! Sv39 page tables.

use core::sync::atomic::{AtomicUsize, Ordering};

use oxcore::PAGE_SIZE;

use super::{frame, kernel_layout, phys_to_virt, virt_to_phys};
use crate::arch::csr;
use crate::config::*;

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;
pub const PTE_G: usize = 1 << 5;
pub const PTE_A: usize = 1 << 6;
pub const PTE_D: usize = 1 << 7;
const PTE_LEAF: usize = PTE_R | PTE_W | PTE_X;

const SATP_SV39: usize = 8 << 60;
pub const ENTRIES: usize = 512;
/// Root-table slots from here up are the kernel half, shared by every
/// address space.
pub const KERNEL_ROOT_FIRST: usize = 256;

static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);
static KERNEL_ROOT: AtomicUsize = AtomicUsize::new(0);

/// The satp each hart has loaded, so an address space is never freed while a
/// hart can still walk it.
static LOADED: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];

/// Loads `satp` on this hart (flushing the TLB). Call with interrupts off.
pub fn load_satp(satp: usize) {
    LOADED[crate::arch::hartid()].store(satp, Ordering::SeqCst);
    csr::set_satp(satp);
}

pub fn loaded_satp() -> usize {
    LOADED[crate::arch::hartid()].load(Ordering::Relaxed)
}

/// Spins until no hart has `satp` loaded. Harts only keep a task's table
/// loaded briefly after it stops running (see the scheduler).
pub fn wait_unloaded(satp: usize) {
    let me = crate::arch::hartid();
    assert!(LOADED[me].load(Ordering::SeqCst) != satp, "freeing the loaded address space");
    while LOADED.iter().any(|l| l.load(Ordering::SeqCst) == satp) {
        core::hint::spin_loop();
    }
}

pub fn kernel_satp() -> usize {
    KERNEL_SATP.load(Ordering::Relaxed)
}

pub fn kernel_root() -> usize {
    KERNEL_ROOT.load(Ordering::Relaxed)
}

pub fn pte_pa(pte: usize) -> usize {
    (pte >> 10) << 12
}

fn make_pte(pa: usize, flags: usize) -> usize {
    // Pre-set A and D so hardware never has to update them.
    let ad = PTE_A | if flags & PTE_W != 0 { PTE_D } else { 0 };
    ((pa >> 12) << 10) | flags | ad | PTE_V
}

fn vpn(va: usize, level: usize) -> usize {
    (va >> (12 + 9 * level)) & 0x1ff
}

pub fn table(pa: usize) -> &'static mut [usize; ENTRIES] {
    unsafe { &mut *(phys_to_virt(pa) as *mut [usize; ENTRIES]) }
}

#[derive(Debug)]
pub enum MapError {
    NoMemory,
    AlreadyMapped,
}

/// A page table identified by its root frame. It does not free anything on
/// drop; [`super::AddressSpace`] owns the user half.
pub struct PageTable {
    root: usize,
}

impl PageTable {
    pub fn new() -> Option<Self> {
        Some(Self { root: frame::alloc_frame()? })
    }

    pub fn root(&self) -> usize {
        self.root
    }

    pub fn satp(&self) -> usize {
        SATP_SV39 | (self.root >> 12)
    }

    /// Finds the PTE that maps `va` at `level` (0: 4 KiB, 1: 2 MiB, 2: 1 GiB),
    /// creating intermediate tables when `alloc` is set.
    fn walk(&mut self, va: usize, level: usize, alloc: bool) -> Result<Option<&'static mut usize>, MapError> {
        let mut t = table(self.root);
        for l in (level + 1..=2).rev() {
            let pte = &mut t[vpn(va, l)];
            if *pte & PTE_V == 0 {
                if !alloc {
                    return Ok(None);
                }
                let pa = frame::alloc_frame().ok_or(MapError::NoMemory)?;
                *pte = ((pa >> 12) << 10) | PTE_V;
            } else if *pte & PTE_LEAF != 0 {
                // A superpage covers va already.
                return if alloc { Err(MapError::AlreadyMapped) } else { Ok(None) };
            }
            t = table(pte_pa(*pte));
        }
        Ok(Some(&mut t[vpn(va, level)]))
    }

    pub fn map(&mut self, va: usize, pa: usize, level: usize, flags: usize) -> Result<(), MapError> {
        let pte = self.walk(va, level, true)?.unwrap();
        if *pte & PTE_V != 0 {
            return Err(MapError::AlreadyMapped);
        }
        *pte = make_pte(pa, flags);
        Ok(())
    }

    /// Maps `[va, va+len)` to `[pa, pa+len)` with the biggest pages alignment
    /// allows.
    pub fn map_range(&mut self, mut va: usize, mut pa: usize, len: usize, flags: usize) -> Result<(), MapError> {
        let end = va + len;
        while va < end {
            let level = (0..=2)
                .rev()
                .find(|&l| {
                    let size = PAGE_SIZE << (9 * l);
                    va.is_multiple_of(size) && pa.is_multiple_of(size) && end - va >= size
                })
                .unwrap();
            self.map(va, pa, level, flags)?;
            let size = PAGE_SIZE << (9 * level);
            va += size;
            pa += size;
        }
        Ok(())
    }

    /// Removes the 4 KiB mapping of `va`, returning the frame it pointed to.
    pub fn unmap(&mut self, va: usize) -> Option<usize> {
        let pte = self.walk(va, 0, false).ok()??;
        if *pte & PTE_V == 0 {
            return None;
        }
        let pa = pte_pa(*pte);
        *pte = 0;
        Some(pa)
    }

    /// Physical address and PTE flags for `va`, following superpages.
    pub fn translate(&self, va: usize) -> Option<(usize, usize)> {
        let mut t = table(self.root);
        for l in (0..=2).rev() {
            let pte = t[vpn(va, l)];
            if pte & PTE_V == 0 {
                return None;
            }
            if pte & PTE_LEAF != 0 {
                let offset = va & ((PAGE_SIZE << (9 * l)) - 1);
                return Some((pte_pa(pte) + offset, pte & 0x3ff));
            }
            t = table(pte_pa(pte));
        }
        None
    }

    /// Adds `flags` to an existing 4 KiB mapping.
    pub fn add_flags(&mut self, va: usize, flags: usize) {
        if let Ok(Some(pte)) = self.walk(va, 0, false) {
            *pte |= flags | if flags & PTE_W != 0 { PTE_D } else { 0 };
        }
    }
}

/// Builds the kernel's page table with per-section permissions (W^X), the
/// direct map of the remaining RAM, device MMIO and the kernel stack region,
/// then switches to it.
pub fn init_kernel(ram_end: usize) {
    let mut pt = PageTable::new().expect("no memory for kernel page table");
    let k = kernel_layout();
    let g = PTE_G;
    let sections = [(k.text, PTE_R | PTE_X), (k.rodata, PTE_R), (k.data, PTE_R | PTE_W)];
    for ((start, end), flags) in sections {
        pt.map_range(start, virt_to_phys(start), end - start, flags | g).unwrap();
    }
    let ram_from = virt_to_phys(k.data.1);
    pt.map_range(phys_to_virt(ram_from), ram_from, ram_end - ram_from, PTE_R | PTE_W | g).unwrap();

    let devices =
        [(TEST_DEVICE, PAGE_SIZE), (PLIC_BASE, PLIC_SIZE), (UART0, PAGE_SIZE), (VIRTIO0, VIRTIO_SLOTS * VIRTIO_STRIDE)];
    for (pa, len) in devices {
        pt.map_range(phys_to_virt(pa), pa, len, PTE_R | PTE_W | g).unwrap();
    }

    crate::task::kstack::map_all(&mut pt);

    KERNEL_ROOT.store(pt.root(), Ordering::Relaxed);
    KERNEL_SATP.store(pt.satp(), Ordering::Relaxed);
    load_satp(pt.satp());
    println!("mm: kernel page table active (satp {:#x})", pt.satp());
}
