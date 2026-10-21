//! The kernel heap: the slab allocator from oxcore on top of the frame
//! allocator.

use core::alloc::{GlobalAlloc, Layout};

use oxcore::slab::{PageSource, SlabAllocator};

use super::{frame, phys_to_virt, virt_to_phys};
use crate::sync::SpinLock;

struct Frames;

impl PageSource for Frames {
    fn alloc_pages(&mut self, order: usize) -> Option<usize> {
        frame::alloc_pages(order).map(phys_to_virt)
    }

    unsafe fn free_pages(&mut self, addr: usize, order: usize) {
        unsafe { frame::free_pages(virt_to_phys(addr), order) };
    }
}

struct KernelHeap(SpinLock<SlabAllocator>);

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.lock().alloc(&mut Frames, layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { self.0.lock().dealloc(&mut Frames, ptr, layout.size(), layout.align()) };
    }
}

#[global_allocator]
static HEAP: KernelHeap = KernelHeap(SpinLock::new("heap", SlabAllocator::new()));

/// Bytes currently allocated from the heap.
#[cfg_attr(not(feature = "ktest"), allow(dead_code))]
pub fn in_use() -> usize {
    HEAP.0.lock().in_use()
}
