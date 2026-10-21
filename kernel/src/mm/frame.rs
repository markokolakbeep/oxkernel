//! Physical frame allocator: a buddy allocator over the RAM after the kernel.

use oxcore::buddy::BuddyAllocator;
use oxcore::{PAGE_SIZE, page_round_down, page_round_up};

use super::{phys_to_virt, virt_to_phys};
use crate::sync::SpinLock;

static FRAMES: SpinLock<BuddyAllocator> = SpinLock::new("frames", BuddyAllocator::empty());

/// `free_start` is the first physical byte after the kernel image and
/// `reserved` a physical range to keep out (the devicetree blob).
pub fn init(ram_start: usize, ram_end: usize, free_start: usize, reserved: (usize, usize)) {
    let npages = (ram_end - ram_start) / PAGE_SIZE;
    // The buddy metadata (a byte per page) goes right after the kernel.
    let meta = phys_to_virt(free_start) as *mut u8;
    let first_free = page_round_up(free_start + npages);
    let (rs, re) = (page_round_down(reserved.0), page_round_up(reserved.1));
    let mut frames = FRAMES.lock();
    unsafe {
        frames.init(phys_to_virt(ram_start), npages, meta);
        if re <= first_free || rs >= ram_end {
            frames.add_region(phys_to_virt(first_free), phys_to_virt(ram_end));
        } else {
            if rs > first_free {
                frames.add_region(phys_to_virt(first_free), phys_to_virt(rs));
            }
            if re < ram_end {
                frames.add_region(phys_to_virt(re.max(first_free)), phys_to_virt(ram_end));
            }
        }
    }
    let free = frames.free_pages();
    drop(frames);
    println!(
        "mm: {} MiB RAM, {} MiB free, kernel+meta end at {:#x}",
        (ram_end - ram_start) >> 20,
        (free * PAGE_SIZE) >> 20,
        first_free
    );
}

/// Allocates `2^order` physically contiguous pages, not zeroed. Returns the
/// physical address.
pub fn alloc_pages(order: usize) -> Option<usize> {
    FRAMES.lock().alloc(order).map(virt_to_phys)
}

/// # Safety
/// `pa` must come from `alloc_pages(order)` and no longer be in use.
pub unsafe fn free_pages(pa: usize, order: usize) {
    unsafe { FRAMES.lock().free(phys_to_virt(pa), order) };
}

/// Allocates one zeroed frame.
pub fn alloc_frame() -> Option<usize> {
    let pa = alloc_pages(0)?;
    unsafe { core::ptr::write_bytes(phys_to_virt(pa) as *mut u8, 0, PAGE_SIZE) };
    Some(pa)
}

/// # Safety
/// As for [`free_pages`].
pub unsafe fn free_frame(pa: usize) {
    unsafe { free_pages(pa, 0) };
}

/// (free, total) page counts.
pub fn stats() -> (usize, usize) {
    let f = FRAMES.lock();
    (f.free_pages(), f.total_pages())
}
