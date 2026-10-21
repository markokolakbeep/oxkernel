#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    let mut status = 0;
    for p in &args[1..] {
        if let Err(e) = mkdir(p) {
            perror(p, e);
            status = 1;
        }
    }
    status
}
