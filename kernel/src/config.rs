//! Compile-time configuration and the QEMU `virt` memory map.

pub const MAX_HARTS: usize = 8; // entry.S has a copy

/// Base of the direct map: physical address `pa` is mapped at
/// `KERNEL_OFFSET + pa`. The kernel image itself lives inside it.
pub const KERNEL_OFFSET: usize = 0xffff_ffc0_0000_0000;

/// Kernel stacks of tasks live in their own region, each with an unmapped
/// guard page below it, so an overflow faults instead of corrupting memory.
pub const KSTACK_REGION: usize = 0xffff_ffff_0000_0000;
pub const KSTACK_PAGES: usize = 4;

pub const NPROC: usize = 64;
pub const NOFILE: usize = 16;
pub const TICK_HZ: u64 = 100;

/// User space is the lower half of Sv39, below 2^38.
pub const USER_TOP: usize = 0x40_0000_0000;
pub const USER_STACK_TOP: usize = USER_TOP - 0x1000;
pub const USER_STACK_PAGES: usize = 16;
/// The heap may not grow past this, which keeps it clear of the stack.
pub const USER_HEAP_LIMIT: usize = 0x20_0000_0000;
pub const MAX_ARGS: usize = 32;
pub const MAX_ARG_BYTES: usize = 4096;
pub const MAX_PATH: usize = 256;

// QEMU virt devices (physical addresses).
pub const TEST_DEVICE: usize = 0x10_0000;
pub const PLIC_BASE: usize = 0x0c00_0000;
pub const PLIC_SIZE: usize = 0x40_0000;
pub const UART0: usize = 0x1000_0000;
pub const UART0_IRQ: u32 = 10;
pub const VIRTIO0: usize = 0x1000_1000;
pub const VIRTIO_STRIDE: usize = 0x1000;
pub const VIRTIO_SLOTS: usize = 8;
pub const VIRTIO0_IRQ: u32 = 1;
