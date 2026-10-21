#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    println!("{}", args[1..].join(" "));
    0
}
