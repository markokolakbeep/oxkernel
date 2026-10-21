//! pid 1: opens the console, then keeps a shell running and reaps orphans.
//! `init test` runs /bin/usertests and powers off with its result;
//! `init bench` does the same with /bin/bench.

#![no_std]
#![no_main]

use ulib::*;

fn spawn(path: &str, args: &[&str]) -> i32 {
    match fork() {
        Ok(0) => {
            let e = exec(path, args);
            perror(path, e);
            exit(127);
        }
        Ok(pid) => pid,
        Err(e) => {
            perror("init: fork", e);
            -1
        }
    }
}

/// Waits for `pid`, reaping any orphans that exit meanwhile.
fn wait_for(pid: i32) -> i32 {
    loop {
        match wait() {
            Ok((p, code)) if p == pid => return code,
            Ok(_) => {}
            Err(_) => return -1,
        }
    }
}

fn run_and_power_off(path: &str) -> ! {
    let pid = spawn(path, &[path]);
    let code = wait_for(pid);
    let _ = shutdown(if code == 0 { 0 } else { 1 });
    exit(1)
}

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    if open("/dev/console", abi::O_RDWR).is_err() {
        exit(1);
    }
    let _ = dup(0);
    let _ = dup(0);
    match args.get(1).copied() {
        Some("test") => run_and_power_off("/bin/usertests"),
        Some("bench") => run_and_power_off("/bin/bench"),
        _ => {}
    }
    loop {
        println!("init: starting sh");
        let pid = spawn("/bin/sh", &["sh"]);
        if pid < 0 {
            let _ = sleep_ms(1000);
            continue;
        }
        let code = wait_for(pid);
        println!("init: sh exited with {code}");
    }
}
