#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    let mut status = 0;
    for a in &args[1..] {
        match a.parse::<i32>() {
            Ok(pid) => {
                if let Err(e) = kill(pid) {
                    perror(a, e);
                    status = 1;
                }
            }
            Err(_) => {
                eprintln!("kill: bad pid {a}");
                status = 1;
            }
        }
    }
    status
}
