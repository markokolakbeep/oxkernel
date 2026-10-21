#![no_std]
#![no_main]

use ulib::*;

#[unsafe(no_mangle)]
fn main(_args: &[&str]) -> i32 {
    let i = sysinfo();
    let up = i.uptime_us / 1000;
    println!("cpus:     {}", i.ncpus);
    println!("procs:    {}", i.nprocs);
    println!("memory:   {} KiB free of {} KiB", i.free_pages * 4, i.total_pages * 4);
    println!("uptime:   {}.{:03} s", up / 1000, up % 1000);
    println!("timebase: {} Hz", i.timebase_hz);
    0
}
