//! Memory management.
//!
//! Physical memory is direct-mapped at [`KERNEL_OFFSET`], so the kernel can
//! reach any frame through `phys_to_virt`. The buddy allocator hands out
//! frames, the slab heap backs `alloc`, and every process gets its own Sv39
//! page table that shares the kernel half of the root table.

pub mod addrspace;
pub mod frame;
pub mod heap;
pub mod paging;

use crate::config::{KERNEL_OFFSET, KSTACK_REGION};

pub use addrspace::AddressSpace;

#[inline]
pub const fn phys_to_virt(pa: usize) -> usize {
    pa + KERNEL_OFFSET
}

/// Translates a direct-map address back to physical.
#[inline]
pub fn virt_to_phys(va: usize) -> usize {
    debug_assert!((KERNEL_OFFSET..KSTACK_REGION).contains(&va), "virt_to_phys({va:#x}) outside the direct map");
    va - KERNEL_OFFSET
}

unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

pub struct KernelLayout {
    pub text: (usize, usize),
    pub rodata: (usize, usize),
    pub data: (usize, usize),
}

pub fn kernel_layout() -> KernelLayout {
    KernelLayout {
        text: (&raw const __text_start as usize, &raw const __text_end as usize),
        rodata: (&raw const __rodata_start as usize, &raw const __rodata_end as usize),
        data: (&raw const __data_start as usize, &raw const __kernel_end as usize),
    }
}

/// Brings up the frame allocator and the kernel page table, then switches
/// this hart onto it.
pub fn init(ram_start: usize, ram_size: usize, reserved: (usize, usize)) {
    let kernel_end_pa = virt_to_phys(kernel_layout().data.1);
    frame::init(ram_start, ram_start + ram_size, kernel_end_pa, reserved);
    paging::init_kernel(ram_start + ram_size);
}
