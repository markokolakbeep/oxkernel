#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    let Some(secs) = args.get(1).and_then(|s| s.parse::<u64>().ok()) else {
        eprintln!("usage: sleep SECONDS");
        return 1;
    };
    if sleep_ms(secs * 1000).is_err() { 1 } else { 0 }
}
