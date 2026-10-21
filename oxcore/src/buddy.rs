//! Binary buddy allocator over 4 KiB pages.
//!
//! Free blocks live on intrusive doubly linked lists, one per order; the list
//! node is stored in the free block itself. A side table holds one byte per
//! page: `order + 1` if the page heads a free block of that order, 0
//! otherwise. That makes "is my buddy free?" an O(1) lookup and lets `free`
//! catch double frees.
//!
//! Addresses handed in and out are whatever the caller can dereference (the
//! kernel uses its direct-map virtual addresses). Blocks are aligned relative
//! to `base`, so a base aligned to the largest block size gives naturally
//! aligned blocks.

use crate::PAGE_SIZE;

/// Orders `0..MAX_ORDER`; the largest block is `PAGE_SIZE << (MAX_ORDER - 1)`
/// (4 MiB).
pub const MAX_ORDER: usize = 11;

const NIL: usize = 0;

#[repr(C)]
struct FreeNode {
    next: usize,
    prev: usize,
}

pub struct BuddyAllocator {
    base: usize,
    npages: usize,
    meta: *mut u8,
    heads: [usize; MAX_ORDER],
    free_pages: usize,
}

// The allocator owns the memory it manages; the raw pointers are only touched
// through `&mut self`.
unsafe impl Send for BuddyAllocator {}

impl BuddyAllocator {
    pub const fn empty() -> Self {
        Self { base: 0, npages: 0, meta: core::ptr::null_mut(), heads: [NIL; MAX_ORDER], free_pages: 0 }
    }

    /// Sets up an allocator spanning `npages` pages from `base`. All pages
    /// start out allocated; hand them over with [`add_region`](Self::add_region).
    ///
    /// # Safety
    /// `base` must be page aligned, `meta` must point to `npages` writable
    /// bytes that outlive the allocator and lie outside every region later
    /// added.
    pub unsafe fn init(&mut self, base: usize, npages: usize, meta: *mut u8) {
        assert!(base.is_multiple_of(PAGE_SIZE) && base != NIL);
        self.base = base;
        self.npages = npages;
        self.meta = meta;
        self.heads = [NIL; MAX_ORDER];
        self.free_pages = 0;
        unsafe { core::ptr::write_bytes(meta, 0, npages) };
    }

    /// Frees every whole page in `[start, end)`.
    ///
    /// # Safety
    /// The range must lie inside the span given to `init` and must not be in
    /// use by anyone else.
    pub unsafe fn add_region(&mut self, start: usize, end: usize) {
        let first = (start.max(self.base) - self.base).div_ceil(PAGE_SIZE);
        let last = ((end - self.base) / PAGE_SIZE).min(self.npages);
        let mut idx = first;
        while idx < last {
            let mut order = MAX_ORDER - 1;
            while order > 0 && (idx & ((1 << order) - 1) != 0 || idx + (1 << order) > last) {
                order -= 1;
            }
            unsafe { self.free_idx(idx, order) };
            idx += 1 << order;
        }
    }

    pub fn free_pages(&self) -> usize {
        self.free_pages
    }

    pub fn total_pages(&self) -> usize {
        self.npages
    }

    /// Allocates `2^order` contiguous pages.
    pub fn alloc(&mut self, order: usize) -> Option<usize> {
        if order >= MAX_ORDER {
            return None;
        }
        let mut o = order;
        while o < MAX_ORDER && self.heads[o] == NIL {
            o += 1;
        }
        if o == MAX_ORDER {
            return None;
        }
        let addr = self.heads[o];
        let idx = self.idx_of(addr);
        unsafe { self.remove(idx, o) };
        // Split, returning the upper halves to the free lists.
        while o > order {
            o -= 1;
            unsafe { self.push(idx + (1 << o), o) };
        }
        self.free_pages -= 1 << order;
        Some(addr)
    }

    /// Returns a block from [`alloc`](Self::alloc), merging it with free buddies.
    ///
    /// # Safety
    /// `addr` must come from `alloc(order)` on this allocator and not be freed
    /// already.
    pub unsafe fn free(&mut self, addr: usize, order: usize) {
        let idx = self.idx_of(addr);
        unsafe { self.free_idx(idx, order) };
    }

    unsafe fn free_idx(&mut self, mut idx: usize, order: usize) {
        assert!(idx + (1 << order) <= self.npages, "buddy: free out of range");
        assert!(unsafe { *self.meta.add(idx) } == 0, "buddy: double free of page {idx}");
        self.free_pages += 1 << order;
        let mut o = order;
        while o < MAX_ORDER - 1 {
            let buddy = idx ^ (1 << o);
            if buddy + (1 << o) > self.npages || unsafe { *self.meta.add(buddy) } != o as u8 + 1 {
                break;
            }
            unsafe { self.remove(buddy, o) };
            idx = idx.min(buddy);
            o += 1;
        }
        unsafe { self.push(idx, o) };
    }

    fn idx_of(&self, addr: usize) -> usize {
        assert!(addr >= self.base && (addr - self.base).is_multiple_of(PAGE_SIZE), "buddy: bad address {addr:#x}");
        (addr - self.base) / PAGE_SIZE
    }

    fn addr_of(&self, idx: usize) -> usize {
        self.base + idx * PAGE_SIZE
    }

    unsafe fn node(&self, addr: usize) -> *mut FreeNode {
        addr as *mut FreeNode
    }

    unsafe fn push(&mut self, idx: usize, order: usize) {
        let addr = self.addr_of(idx);
        let head = self.heads[order];
        unsafe {
            *self.node(addr) = FreeNode { next: head, prev: NIL };
            if head != NIL {
                (*self.node(head)).prev = addr;
            }
            *self.meta.add(idx) = order as u8 + 1;
        }
        self.heads[order] = addr;
    }

    unsafe fn remove(&mut self, idx: usize, order: usize) {
        let addr = self.addr_of(idx);
        unsafe {
            let FreeNode { next, prev } = *self.node(addr);
            if prev != NIL {
                (*self.node(prev)).next = next;
            } else {
                self.heads[order] = next;
            }
            if next != NIL {
                (*self.node(next)).prev = prev;
            }
            *self.meta.add(idx) = 0;
        }
    }
}

/// Smallest order whose block holds `pages` pages.
pub fn order_for_pages(pages: usize) -> usize {
    pages.max(1).next_power_of_two().trailing_zeros() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{Layout, alloc, dealloc};

    struct Arena {
        ptr: *mut u8,
        layout: Layout,
        meta: Vec<u8>,
    }

    impl Arena {
        fn new(pages: usize) -> Self {
            let layout = Layout::from_size_align(pages * PAGE_SIZE, PAGE_SIZE << (MAX_ORDER - 1)).unwrap();
            let ptr = unsafe { alloc(layout) };
            assert!(!ptr.is_null());
            Self { ptr, layout, meta: vec![0; pages] }
        }

        fn allocator(&mut self) -> BuddyAllocator {
            let pages = self.layout.size() / PAGE_SIZE;
            let mut b = BuddyAllocator::empty();
            unsafe {
                b.init(self.ptr as usize, pages, self.meta.as_mut_ptr());
                b.add_region(self.ptr as usize, self.ptr as usize + pages * PAGE_SIZE);
            }
            b
        }
    }

    impl Drop for Arena {
        fn drop(&mut self) {
            unsafe { dealloc(self.ptr, self.layout) };
        }
    }

    #[test]
    fn alloc_every_page_then_free() {
        let mut arena = Arena::new(1024 + 37);
        let mut b = arena.allocator();
        assert_eq!(b.free_pages(), 1024 + 37);
        let mut pages = Vec::new();
        while let Some(p) = b.alloc(0) {
            pages.push(p);
        }
        assert_eq!(pages.len(), 1024 + 37);
        pages.sort();
        pages.dedup();
        assert_eq!(pages.len(), 1024 + 37, "no page handed out twice");
        for p in pages {
            unsafe { b.free(p, 0) };
        }
        assert_eq!(b.free_pages(), 1024 + 37);
        // Everything coalesced back: a max-order block is available again.
        assert!(b.alloc(MAX_ORDER - 1).is_some());
    }

    #[test]
    fn split_and_merge() {
        let mut arena = Arena::new(1 << (MAX_ORDER - 1));
        let mut b = arena.allocator();
        let a = b.alloc(0).unwrap();
        let c = b.alloc(3).unwrap();
        assert_eq!(a % PAGE_SIZE, 0);
        assert_eq!((c - arena.ptr as usize) % (8 * PAGE_SIZE), 0, "order-3 block is 8-page aligned");
        assert!(b.alloc(MAX_ORDER - 1).is_none(), "the only big block was split");
        unsafe {
            b.free(a, 0);
            b.free(c, 3);
        }
        assert_eq!(b.alloc(MAX_ORDER - 1), Some(arena.ptr as usize));
    }

    #[test]
    #[should_panic(expected = "double free")]
    fn double_free_panics() {
        let mut arena = Arena::new(16);
        let mut b = arena.allocator();
        let p = b.alloc(0).unwrap();
        unsafe {
            b.free(p, 0);
            b.free(p, 0);
        }
    }

    #[test]
    fn random_stress_has_no_overlap() {
        let mut arena = Arena::new(2048);
        let mut b = arena.allocator();
        let mut live: Vec<(usize, usize)> = Vec::new();
        let mut seed = 0x1234_5678u64;
        let mut rand = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            if live.is_empty() || rand() % 3 != 0 {
                let order = (rand() % 6) as usize;
                if let Some(p) = b.alloc(order) {
                    let len = PAGE_SIZE << order;
                    for &(q, qo) in &live {
                        let qlen = PAGE_SIZE << qo;
                        assert!(p + len <= q || q + qlen <= p, "overlap");
                    }
                    live.push((p, order));
                }
            } else {
                let i = (rand() as usize) % live.len();
                let (p, o) = live.swap_remove(i);
                unsafe { b.free(p, o) };
            }
        }
        for (p, o) in live {
            unsafe { b.free(p, o) };
        }
        assert_eq!(b.free_pages(), 2048);
    }

    #[test]
    fn partial_region() {
        let mut arena = Arena::new(64);
        let base = arena.ptr as usize;
        let mut b = BuddyAllocator::empty();
        unsafe {
            b.init(base, 64, arena.meta.as_mut_ptr());
            // Unaligned start: the partial first page is skipped.
            b.add_region(base + 3 * PAGE_SIZE + 100, base + 40 * PAGE_SIZE);
        }
        assert_eq!(b.free_pages(), 36);
        while let Some(p) = b.alloc(0) {
            assert!(p >= base + 4 * PAGE_SIZE && p < base + 40 * PAGE_SIZE);
        }
    }

    #[test]
    fn order_for_pages_rounds_up() {
        assert_eq!(order_for_pages(0), 0);
        assert_eq!(order_for_pages(1), 0);
        assert_eq!(order_for_pages(2), 1);
        assert_eq!(order_for_pages(3), 2);
        assert_eq!(order_for_pages(1024), 10);
    }
}
