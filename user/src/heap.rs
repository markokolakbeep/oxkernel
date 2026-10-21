//! User heap: the same slab allocator as the kernel, fed with pages from
//! sbrk. Freed multi-page blocks go on per-order free lists for reuse.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;

use oxcore::PAGE_SIZE;
use oxcore::buddy::MAX_ORDER;
use oxcore::slab::{PageSource, SlabAllocator};

struct SbrkPages {
    free: [usize; MAX_ORDER],
}

impl PageSource for SbrkPages {
    fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        if order >= MAX_ORDER {
            let bytes = PAGE_SIZE << order;
            return crate::sys::sbrk(bytes as isize).ok();
        }
        if self.free[order] != 0 {
            let p = self.free[order];
            self.free[order] = unsafe { *(p as *const usize) };
            return Some(p);
        }
        let brk = crate::sys::sbrk(0).ok()?;
        // Keep pages page aligned even if someone moved the break by hand.
        let pad = brk.next_multiple_of(PAGE_SIZE) - brk;
        let start = crate::sys::sbrk((pad + (PAGE_SIZE << order)) as isize).ok()? + pad;
        Some(start)
    }

    unsafe fn free_pages(&mut self, addr: usize, order: usize) {
        if order < MAX_ORDER {
            unsafe { *(addr as *mut usize) = self.free[order] };
            self.free[order] = addr;
        }
    }
}

struct UserHeap(UnsafeCell<(SlabAllocator, SbrkPages)>);

// User programs are single threaded.
unsafe impl Sync for UserHeap {}

unsafe impl GlobalAlloc for UserHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let (slab, pages) = unsafe { &mut *self.0.get() };
        slab.alloc(pages, layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let (slab, pages) = unsafe { &mut *self.0.get() };
        unsafe { slab.dealloc(pages, ptr, layout.size(), layout.align()) };
    }
}

#[global_allocator]
static HEAP: UserHeap = UserHeap(UnsafeCell::new((SlabAllocator::new(), SbrkPages { free: [0; MAX_ORDER] })));
