//! User address spaces.
//!
//! Each process has its own root table. The kernel half (root slots 256..511)
//! is copied from the kernel's root table, so the kernel stays mapped in
//! every address space and a trap does not need to switch `satp`. The user
//! half only ever holds 4 KiB pages, all owned by this address space.

use alloc::string::String;
use alloc::vec::Vec;

use oxcore::abi::errno::*;
use oxcore::{PAGE_SIZE, page_round_down};

use super::frame;
use super::paging::*;
use super::phys_to_virt;
use crate::KResult;
use crate::config::USER_TOP;

pub struct AddressSpace {
    pt: PageTable,
    pub heap_start: usize,
    pub brk: usize,
    pages: usize,
}

fn page_bytes(pa: usize) -> &'static mut [u8; PAGE_SIZE] {
    unsafe { &mut *(phys_to_virt(pa) as *mut [u8; PAGE_SIZE]) }
}

impl AddressSpace {
    pub fn new() -> KResult<Self> {
        let pt = PageTable::new().ok_or(ENOMEM)?;
        let kernel = table(kernel_root());
        let root = table(pt.root());
        root[KERNEL_ROOT_FIRST..].copy_from_slice(&kernel[KERNEL_ROOT_FIRST..]);
        Ok(Self { pt, heap_start: 0, brk: 0, pages: 0 })
    }

    pub fn satp(&self) -> usize {
        self.pt.satp()
    }

    /// Bytes of user memory mapped.
    pub fn resident(&self) -> usize {
        self.pages * PAGE_SIZE
    }

    /// Makes sure the user page at `va` is mapped with at least `flags`
    /// (plus U), allocating a zeroed frame if needed. Returns its frame.
    pub fn map_zeroed(&mut self, va: usize, flags: usize) -> KResult<usize> {
        let va = page_round_down(va);
        if va >= USER_TOP {
            return Err(EFAULT);
        }
        if let Some((pa, _)) = self.pt.translate(va) {
            self.pt.add_flags(va, flags | PTE_U);
            return Ok(pa);
        }
        let pa = frame::alloc_frame().ok_or(ENOMEM)?;
        if self.pt.map(va, pa, 0, flags | PTE_U).is_err() {
            unsafe { frame::free_frame(pa) };
            return Err(ENOMEM);
        }
        self.pages += 1;
        Ok(pa)
    }

    /// Unmaps and frees the user pages in `[start, end)`. The caller flushes
    /// the TLB.
    pub fn unmap_range(&mut self, start: usize, end: usize) {
        let mut va = page_round_down(start);
        while va < end {
            if let Some(pa) = self.pt.unmap(va) {
                unsafe { frame::free_frame(pa) };
                self.pages -= 1;
            }
            va += PAGE_SIZE;
        }
    }

    /// Eagerly copies every user page into a new address space.
    pub fn fork(&self) -> KResult<Self> {
        let mut child = Self::new()?;
        child.heap_start = self.heap_start;
        child.brk = self.brk;
        let mut result = Ok(());
        for_each_user_leaf(self.pt.root(), |va, pte| {
            if result.is_err() {
                return;
            }
            result = (|| {
                let pa = frame::alloc_frame().ok_or(ENOMEM)?;
                page_bytes(pa).copy_from_slice(page_bytes(pte_pa(pte)));
                let flags = pte & (PTE_R | PTE_W | PTE_X | PTE_U);
                if child.pt.map(va, pa, 0, flags).is_err() {
                    unsafe { frame::free_frame(pa) };
                    return Err(ENOMEM);
                }
                child.pages += 1;
                Ok(())
            })();
        });
        result.map(|_| child)
    }

    /// Physical address behind user address `va`, checking it is a user page
    /// with the access rights needed.
    fn user_pa(&self, va: usize, write: bool) -> KResult<usize> {
        if va >= USER_TOP {
            return Err(EFAULT);
        }
        let (pa, flags) = self.pt.translate(va).ok_or(EFAULT)?;
        let need = PTE_U | if write { PTE_W } else { PTE_R };
        if flags & need != need {
            return Err(EFAULT);
        }
        Ok(pa)
    }

    /// Copies `src` to user memory at `dst`.
    pub fn copy_out(&self, mut dst: usize, mut src: &[u8]) -> KResult<()> {
        while !src.is_empty() {
            let pa = self.user_pa(dst, true)?;
            let n = (PAGE_SIZE - dst % PAGE_SIZE).min(src.len());
            let off = pa % PAGE_SIZE;
            page_bytes(page_round_down(pa))[off..off + n].copy_from_slice(&src[..n]);
            src = &src[n..];
            dst += n;
        }
        Ok(())
    }

    /// Copies user memory at `src` into `dst`.
    pub fn copy_in(&self, mut dst: &mut [u8], mut src: usize) -> KResult<()> {
        while !dst.is_empty() {
            let pa = self.user_pa(src, false)?;
            let n = (PAGE_SIZE - src % PAGE_SIZE).min(dst.len());
            let off = pa % PAGE_SIZE;
            dst[..n].copy_from_slice(&page_bytes(page_round_down(pa))[off..off + n]);
            dst = &mut dst[n..];
            src += n;
        }
        Ok(())
    }

    pub fn copy_out_val<T: Copy>(&self, dst: usize, v: &T) -> KResult<()> {
        let bytes = unsafe { core::slice::from_raw_parts(v as *const T as *const u8, size_of::<T>()) };
        self.copy_out(dst, bytes)
    }

    pub fn copy_in_val<T: Copy + Default>(&self, src: usize) -> KResult<T> {
        let mut v = T::default();
        let bytes = unsafe { core::slice::from_raw_parts_mut(&mut v as *mut T as *mut u8, size_of::<T>()) };
        self.copy_in(bytes, src)?;
        Ok(v)
    }

    /// Copies a NUL-terminated string of at most `max` bytes.
    pub fn copy_in_str(&self, mut src: usize, max: usize) -> KResult<String> {
        let mut out = Vec::new();
        loop {
            let pa = self.user_pa(src, false)?;
            let off = pa % PAGE_SIZE;
            let page = page_bytes(page_round_down(pa));
            for &c in &page[off..] {
                if c == 0 {
                    return String::from_utf8(out).map_err(|_| EINVAL);
                }
                if out.len() == max {
                    return Err(ENAMETOOLONG);
                }
                out.push(c);
            }
            src += PAGE_SIZE - off;
        }
    }
}

/// Calls `f(va, pte)` for every 4 KiB leaf in the user half.
fn for_each_user_leaf(root: usize, mut f: impl FnMut(usize, usize)) {
    let r = table(root);
    for (i2, &e2) in r[..KERNEL_ROOT_FIRST].iter().enumerate() {
        if e2 & PTE_V == 0 {
            continue;
        }
        for (i1, &e1) in table(pte_pa(e2)).iter().enumerate() {
            if e1 & PTE_V == 0 {
                continue;
            }
            for (i0, &e0) in table(pte_pa(e1)).iter().enumerate() {
                if e0 & PTE_V != 0 {
                    f((i2 << 30) | (i1 << 21) | (i0 << 12), e0);
                }
            }
        }
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        wait_unloaded(self.pt.satp());
        for_each_user_leaf(self.pt.root(), |_, pte| unsafe { frame::free_frame(pte_pa(pte)) });
        let r = table(self.pt.root());
        for &e2 in &r[..KERNEL_ROOT_FIRST] {
            if e2 & PTE_V == 0 {
                continue;
            }
            for &e1 in table(pte_pa(e2)).iter() {
                if e1 & PTE_V != 0 {
                    unsafe { frame::free_frame(pte_pa(e1)) };
                }
            }
            unsafe { frame::free_frame(pte_pa(e2)) };
        }
        unsafe { frame::free_frame(self.pt.root()) };
    }
}
