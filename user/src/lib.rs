//! ulib: the user-space runtime for oxkernel programs.
//!
//! A program is a `#![no_std] #![no_main]` binary that defines
//!
//! ```ignore
//! #[unsafe(no_mangle)]
//! fn main(args: &[&str]) -> i32 { ... }
//! ```
//!
//! and gets `_start`, a heap, `print!`/`println!` and a panic handler from
//! here.

#![no_std]

extern crate alloc;

mod heap;
pub mod io;
pub mod sys;

pub use alloc::{boxed::Box, format, string::String, string::ToString, vec, vec::Vec};
pub use oxcore::abi;
pub use oxcore::abi::errno;

pub use io::{read_line, read_to_end, write_all};
pub use sys::*;

unsafe extern "Rust" {
    fn main(args: &[&str]) -> i32;
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start(argc: usize, argv: *const *const u8) -> ! {
    let mut args: Vec<&'static str> = Vec::with_capacity(argc);
    for i in 0..argc {
        let p = unsafe { *argv.add(i) };
        let mut len = 0;
        while unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        let bytes = unsafe { core::slice::from_raw_parts(p, len) };
        args.push(core::str::from_utf8(bytes).unwrap_or("?"));
    }
    let code = unsafe { main(&args) };
    exit(code)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    eprintln!("panic: {info}");
    exit(101)
}

/// Prints `msg: <error>` to stderr.
pub fn perror(msg: &str, e: isize) {
    eprintln!("{msg}: {}", errno::name(e));
}
