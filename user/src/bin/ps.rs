#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(_args: &[&str]) -> i32 {
    let procs = match procinfo() {
        Ok(p) => p,
        Err(e) => {
            perror("ps", e);
            return 1;
        }
    };
    println!("  PID  PPID  PGID STATE     HART    MEM NAME");
    for p in procs {
        let state = match p.state {
            abi::PROC_RUNNABLE => "runnable",
            abi::PROC_RUNNING => "running",
            abi::PROC_SLEEPING => "sleeping",
            _ => "zombie",
        };
        let end = p.name.iter().position(|&c| c == 0).unwrap_or(16);
        let name = core::str::from_utf8(&p.name[..end]).unwrap_or("?");
        let hart = if p.hart < 0 { String::from("-") } else { p.hart.to_string() };
        println!("{:>5} {:>5} {:>5} {:<9} {:>4} {:>5}K {}", p.pid, p.ppid, p.pgid, state, hart, p.mem / 1024, name);
    }
    0
}
