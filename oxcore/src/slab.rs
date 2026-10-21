//! Size-class slab allocator for small objects, layered on a page source.
//!
//! Requests up to 2 KiB are served from power-of-two size classes (16 B to
//! 2 KiB). Each class carves whole pages into equal objects threaded on an
//! intrusive free list. Because objects are power-of-two sized and pages are
//! page aligned, every object is aligned to its own size. Bigger requests go
//! straight to the page source.
//!
//! Slab pages are never handed back to the page source. That keeps the code
//! simple and is fine for a kernel heap whose working set is mostly
//! long-lived caches.

use crate::PAGE_SIZE;
use crate::buddy::order_for_pages;

pub trait PageSource {
    /// Returns the address of `2^order` contiguous, page-aligned pages.
    fn alloc_pages(&mut self, order: usize) -> Option<usize>;
    /// # Safety
    /// `addr` must come from `alloc_pages(order)`.
    unsafe fn free_pages(&mut self, addr: usize, order: usize);
}

const MIN_SHIFT: usize = 4;
const MAX_SMALL_SHIFT: usize = 11;
const NCLASSES: usize = MAX_SMALL_SHIFT - MIN_SHIFT + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SizeClass {
    Small(usize),
    Pages(usize),
}

fn classify(size: usize, align: usize) -> SizeClass {
    let need = size.max(align).max(1);
    if need <= 1 << MAX_SMALL_SHIFT {
        let shift = (need.next_power_of_two().trailing_zeros() as usize).max(MIN_SHIFT);
        SizeClass::Small(shift - MIN_SHIFT)
    } else {
        SizeClass::Pages(order_for_pages(size.div_ceil(PAGE_SIZE)))
    }
}

pub struct SlabAllocator {
    free: [usize; NCLASSES],
    /// Bytes currently handed out, rounded up to the size class.
    in_use: usize,
}

impl Default for SlabAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl SlabAllocator {
    pub const fn new() -> Self {
        Self { free: [0; NCLASSES], in_use: 0 }
    }

    pub fn in_use(&self) -> usize {
        self.in_use
    }

    pub fn alloc(&mut self, src: &mut impl PageSource, size: usize, align: usize) -> *mut u8 {
        match classify(size, align) {
            SizeClass::Small(class) => {
                if self.free[class] == 0 && !self.refill(src, class) {
                    return core::ptr::null_mut();
                }
                let obj = self.free[class];
                self.free[class] = unsafe { *(obj as *const usize) };
                self.in_use += 1 << (class + MIN_SHIFT);
                obj as *mut u8
            }
            SizeClass::Pages(order) => {
                if align > PAGE_SIZE << order {
                    return core::ptr::null_mut();
                }
                match src.alloc_pages(order) {
                    Some(addr) => {
                        self.in_use += PAGE_SIZE << order;
                        addr as *mut u8
                    }
                    None => core::ptr::null_mut(),
                }
            }
        }
    }

    /// # Safety
    /// `ptr` must come from `alloc` with the same `size` and `align`.
    pub unsafe fn dealloc(&mut self, src: &mut impl PageSource, ptr: *mut u8, size: usize, align: usize) {
        match classify(size, align) {
            SizeClass::Small(class) => {
                unsafe { *(ptr as *mut usize) = self.free[class] };
                self.free[class] = ptr as usize;
                self.in_use -= 1 << (class + MIN_SHIFT);
            }
            SizeClass::Pages(order) => {
                unsafe { src.free_pages(ptr as usize, order) };
                self.in_use -= PAGE_SIZE << order;
            }
        }
    }

    fn refill(&mut self, src: &mut impl PageSource, class: usize) -> bool {
        let Some(page) = src.alloc_pages(0) else { return false };
        let size = 1 << (class + MIN_SHIFT);
        // Thread the objects in address order so allocation walks the page
        // forwards.
        let count = PAGE_SIZE / size;
        for i in (0..count).rev() {
            let obj = page + i * size;
            unsafe { *(obj as *mut usize) = self.free[class] };
            self.free[class] = obj;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{Layout, alloc, dealloc};
    use std::collections::HashMap;

    #[derive(Default)]
    struct HostPages {
        live: HashMap<usize, Layout>,
    }

    impl PageSource for HostPages {
        fn alloc_pages(&mut self, order: usize) -> Option<usize> {
            let layout = Layout::from_size_align(PAGE_SIZE << order, PAGE_SIZE << order).unwrap();
            let p = unsafe { alloc(layout) } as usize;
            self.live.insert(p, layout);
            Some(p)
        }

        unsafe fn free_pages(&mut self, addr: usize, order: usize) {
            let layout = self.live.remove(&addr).expect("freeing unknown pages");
            assert_eq!(layout.size(), PAGE_SIZE << order);
            unsafe { dealloc(addr as *mut u8, layout) };
        }
    }

    #[test]
    fn classes() {
        assert_eq!(classify(1, 1), SizeClass::Small(0));
        assert_eq!(classify(16, 8), SizeClass::Small(0));
        assert_eq!(classify(17, 8), SizeClass::Small(1));
        assert_eq!(classify(8, 64), SizeClass::Small(2));
        assert_eq!(classify(2048, 8), SizeClass::Small(NCLASSES - 1));
        assert_eq!(classify(2049, 8), SizeClass::Pages(0));
        assert_eq!(classify(4096, 4096), SizeClass::Pages(0));
        assert_eq!(classify(3 * 4096, 8), SizeClass::Pages(2));
    }

    #[test]
    fn objects_are_aligned_distinct_and_reused() {
        let mut src = HostPages::default();
        let mut slab = SlabAllocator::new();
        let mut seen = std::collections::HashSet::new();
        let mut ptrs = Vec::new();
        for size in [1usize, 8, 24, 100, 500, 2048, 5000, 9000] {
            for _ in 0..300 {
                let p = slab.alloc(&mut src, size, 8);
                assert!(!p.is_null());
                let class = size.max(16).next_power_of_two().min(4096);
                assert_eq!(p as usize % class, 0, "size {size} misaligned");
                assert!(seen.insert(p as usize), "duplicate pointer");
                unsafe { core::ptr::write_bytes(p, 0xab, size) };
                ptrs.push((p, size));
            }
        }
        let pages_before = src.live.len();
        for &(p, size) in &ptrs {
            unsafe { slab.dealloc(&mut src, p, size, 8) };
        }
        assert_eq!(slab.in_use(), 0);
        // Small objects come back from the free lists without new pages.
        for &(_, size) in ptrs.iter().filter(|(_, s)| *s <= 2048) {
            assert!(!slab.alloc(&mut src, size, 8).is_null());
        }
        assert!(src.live.len() <= pages_before);
    }
}
