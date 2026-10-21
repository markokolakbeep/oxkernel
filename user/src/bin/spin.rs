//! Burns CPU, printing a dot now and then. `spin [seconds]`, default forever;
//! handy for watching the scheduler with `ps` and for trying ^C.

#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    let secs: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(u64::MAX / 2_000_000);
    let end = time_us() + secs * 1_000_000;
    let mut next_dot = time_us() + 1_000_000;
    let mut x = 0u64;
    while time_us() < end {
        for _ in 0..10_000 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        if time_us() >= next_dot {
            print!(".");
            next_dot += 1_000_000;
        }
    }
    println!();
    core::hint::black_box(x);
    0
}
