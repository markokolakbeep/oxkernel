//! Target-independent building blocks shared by the kernel, user space and the
//! host tools.
//!
//! Everything here is plain `no_std` Rust with no hardware access, so it can be
//! unit-tested on the host with `cargo test -p oxcore`.

#![cfg_attr(not(test), no_std)]

pub mod abi;
pub mod buddy;
pub mod elf;
pub mod fdt;
pub mod fs;
pub mod path;
pub mod ringbuf;
pub mod slab;

pub const PAGE_SIZE: usize = 4096;
pub const PAGE_SHIFT: usize = 12;

pub const fn page_round_up(x: usize) -> usize {
    (x + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

pub const fn page_round_down(x: usize) -> usize {
    x & !(PAGE_SIZE - 1)
}
