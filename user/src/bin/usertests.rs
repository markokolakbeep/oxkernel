//! Kernel test suite run from user space. Each test runs in its own child
//! process, so a crash fails only that test. `init test` runs this and powers
//! off with the result.

#![no_std]
#![no_main]

use ulib::*;

type TestResult = Result<(), String>;
type Test = (&'static str, fn() -> TestResult);

macro_rules! ensure {
    ($cond:expr, $($fmt:tt)*) => {
        if !$cond {
            return Err(format!($($fmt)*));
        }
    };
}

fn ok<T>(r: SysResult<T>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {}", errno::name(e)))
}

/// Runs `f` in a child; returns its exit code.
fn in_child(f: impl FnOnce() -> i32) -> Result<i32, String> {
    match ok(fork(), "fork")? {
        0 => exit(f()),
        pid => {
            let (p, code) = ok(wait(), "wait")?;
            ensure!(p == pid, "wait returned pid {p}, expected {pid}");
            Ok(code)
        }
    }
}

fn fork_wait() -> TestResult {
    for i in 0..20 {
        let code = in_child(move || i)?;
        ensure!(code == i, "child exit code {code}, expected {i}");
    }
    Ok(())
}

fn fork_many() -> TestResult {
    let mut pids = Vec::new();
    for i in 0..40 {
        match ok(fork(), "fork")? {
            0 => {
                yield_now();
                exit(i % 7);
            }
            pid => pids.push(pid),
        }
    }
    let mut sum = 0;
    for _ in 0..40 {
        let (pid, code) = ok(wait(), "wait")?;
        ensure!(pids.contains(&pid), "unknown child {pid}");
        sum += code;
    }
    ensure!(sum == (0..40).map(|i| i % 7).sum::<i32>(), "exit codes don't add up");
    ensure!(wait() == Err(errno::ECHILD), "expected ECHILD once all children are reaped");
    Ok(())
}

fn fork_exhaustion() -> TestResult {
    // Fork until the kernel says no, then make sure everything is reclaimed.
    let before = sysinfo();
    let code = in_child(|| {
        let mut n = 0;
        loop {
            match fork() {
                Ok(0) => {
                    let _ = sleep_ms(200);
                    exit(0);
                }
                Ok(_) => n += 1,
                Err(_) => break,
            }
        }
        while wait().is_ok() {}
        if n > 10 { 0 } else { 1 }
    })?;
    ensure!(code == 0, "could not fork more than 10 processes");
    let after = sysinfo();
    ensure!(after.nprocs == before.nprocs, "{} processes leaked", after.nprocs - before.nprocs);
    Ok(())
}

fn exec_args() -> TestResult {
    let code = in_child(|| {
        exec("/bin/usertests", &["usertests", "--exec-child", "hello", "world"]);
        99
    })?;
    ensure!(code == 42, "exec'd child returned {code}");
    let code = in_child(|| exec("/bin/does-not-exist", &["x"]) as i32)?;
    ensure!(code == errno::ENOENT as i32, "exec of missing file returned {code}");
    Ok(())
}

fn pipe_data() -> TestResult {
    let (r, w) = ok(pipe(), "pipe")?;
    const N: usize = 100_000;
    match ok(fork(), "fork")? {
        0 => {
            let _ = close(r);
            let data: Vec<u8> = (0..N).map(|i| (i % 251) as u8).collect();
            let _ = write_all(w, &data);
            exit(0);
        }
        _ => {
            ok(close(w), "close")?;
            let data = ok(read_to_end(r), "read")?;
            ok(close(r), "close")?;
            ok(wait(), "wait")?;
            ensure!(data.len() == N, "read {} bytes, expected {N}", data.len());
            ensure!(data.iter().enumerate().all(|(i, &b)| b == (i % 251) as u8), "data corrupted");
        }
    }
    Ok(())
}

fn pipe_broken() -> TestResult {
    let (r, w) = ok(pipe(), "pipe")?;
    ok(close(r), "close")?;
    ensure!(write(w, b"x") == Err(errno::EPIPE), "write to reader-less pipe should be EPIPE");
    ok(close(w), "close")?;
    Ok(())
}

fn sbrk_grow_shrink() -> TestResult {
    let start = ok(sbrk(0), "sbrk")?;
    let size = 4 << 20;
    ok(sbrk(size as isize), "sbrk grow")?;
    let mem = unsafe { core::slice::from_raw_parts_mut(start as *mut u8, size) };
    for (i, b) in mem.iter_mut().enumerate().step_by(4096) {
        ensure!(*b == 0, "fresh heap page not zeroed");
        *b = i as u8;
    }
    ok(sbrk(-(size as isize)), "sbrk shrink")?;
    ensure!(ok(sbrk(0), "sbrk")? == start, "break not restored");
    ensure!(sbrk(1 << 40).is_err(), "absurd sbrk should fail");
    Ok(())
}

fn bad_access_kills() -> TestResult {
    let code = in_child(|| {
        // Deliberately unmapped: the page at 0 is never mapped.
        #[allow(clippy::manual_dangling_ptr)]
        unsafe {
            core::ptr::write_volatile(0x8 as *mut u64, 1)
        };
        0
    })?;
    ensure!(code == -1, "null write: child exited {code}, expected to be killed");
    let code = in_child(|| {
        let p = 0xffff_ffc0_8020_0000usize as *const u64;
        unsafe { core::ptr::read_volatile(p) as i32 }
    })?;
    ensure!(code == -1, "kernel read: child exited {code}, expected to be killed");
    Ok(())
}

fn bad_pointers() -> TestResult {
    ensure!(write_raw(1, 0xdead_0000, 10) == Err(errno::EFAULT), "write from unmapped memory");
    ensure!(write_raw(1, 0xffff_ffc0_8020_0000, 10) == Err(errno::EFAULT), "write from kernel memory");
    ensure!(open_raw(0x1000, 0) == Err(errno::EFAULT), "open with bad path pointer");
    let mut buf = [0u8; 4];
    ensure!(read(99, &mut buf) == Err(errno::EBADF), "read from bad fd");
    Ok(())
}

fn kill_child() -> TestResult {
    let pid = match ok(fork(), "fork")? {
        0 => loop {
            core::hint::spin_loop();
        },
        pid => pid,
    };
    let _ = sleep_ms(20);
    ok(kill(pid), "kill")?;
    let (p, code) = ok(wait(), "wait")?;
    ensure!(p == pid && code == -1, "killed child returned ({p}, {code})");
    // A sleeping child must wake up and die too.
    let pid = match ok(fork(), "fork")? {
        0 => {
            let _ = sleep_ms(100_000);
            exit(0)
        }
        pid => pid,
    };
    let _ = sleep_ms(20);
    ok(kill(pid), "kill")?;
    let (_, code) = ok(wait(), "wait")?;
    ensure!(code == -1, "sleeping child returned {code}");
    ensure!(kill(1) == Err(errno::EPERM), "killing init must fail");
    Ok(())
}

fn sleep_time() -> TestResult {
    let t0 = time_us();
    ok(sleep_ms(50), "sleep")?;
    let dt = time_us() - t0;
    ensure!((50_000..1_000_000).contains(&dt), "slept {dt} us for 50 ms");
    Ok(())
}

fn file_roundtrip() -> TestResult {
    let path = "/tmp/rt";
    let fd = ok(open(path, abi::O_CREATE | abi::O_RDWR | abi::O_TRUNC), "create")?;
    // 300 KiB spans the direct blocks and goes into the indirect block.
    let data: Vec<u8> = (0..300 * 1024).map(|i: usize| (i * 7 % 256) as u8).collect();
    ok(write_all(fd, &data), "write")?;
    ok(close(fd), "close")?;
    ensure!(ok(stat(path), "stat")?.size == data.len() as u64, "size mismatch");
    let fd = ok(open(path, abi::O_RDONLY), "open")?;
    let back = ok(read_to_end(fd), "read")?;
    ok(close(fd), "close")?;
    ensure!(back == data, "file contents differ");
    let fd = ok(open(path, abi::O_WRONLY | abi::O_TRUNC), "truncate")?;
    ok(close(fd), "close")?;
    ensure!(ok(stat(path), "stat")?.size == 0, "truncate did not empty the file");
    ok(unlink(path), "unlink")?;
    ensure!(open(path, abi::O_RDONLY) == Err(errno::ENOENT), "unlinked file still there");
    Ok(())
}

fn disk_space_reclaimed() -> TestResult {
    // Writing and deleting a file repeatedly must not leak blocks: 40 rounds of
    // 1 MiB on a 32 MiB disk would run out otherwise.
    let chunk = vec![0x5au8; 64 * 1024];
    for round in 0..40 {
        let fd = ok(open("/tmp/space", abi::O_CREATE | abi::O_WRONLY), "create")?;
        for _ in 0..16 {
            ok(write_all(fd, &chunk), &format!("write (round {round})"))?;
        }
        ok(close(fd), "close")?;
        ok(unlink("/tmp/space"), "unlink")?;
    }
    Ok(())
}

fn directories() -> TestResult {
    ok(mkdir("/tmp/d"), "mkdir")?;
    ensure!(mkdir("/tmp/d") == Err(errno::EEXIST), "mkdir twice");
    ok(mkdir("/tmp/d/e"), "mkdir nested")?;
    ok(chdir("/tmp/d"), "chdir")?;
    let fd = ok(open("e/f", abi::O_CREATE | abi::O_WRONLY), "create relative")?;
    ok(write_all(fd, b"hi"), "write")?;
    ok(close(fd), "close")?;
    ensure!(ok(stat("../d/e/f"), "stat ..")?.size == 2, "relative lookup");
    ensure!(unlink("e") == Err(errno::ENOTEMPTY), "removed non-empty dir");
    ensure!(open("e/f/g", abi::O_RDONLY) == Err(errno::ENOTDIR), "file used as dir");
    ensure!(open("e", abi::O_WRONLY) == Err(errno::EISDIR), "dir opened for writing");
    ok(unlink("e/f"), "unlink file")?;
    ok(unlink("e"), "unlink dir")?;
    ok(chdir("/"), "chdir /")?;
    ok(unlink("/tmp/d"), "unlink dir")?;
    ensure!(chdir("/tmp/d") == Err(errno::ENOENT), "removed dir still reachable");
    Ok(())
}

fn concurrent_files() -> TestResult {
    const N: usize = 4;
    for i in 0..N {
        if ok(fork(), "fork")? == 0 {
            let path = format!("/tmp/c{i}");
            let data = vec![b'a' + i as u8; 50_000];
            for _ in 0..3 {
                let Ok(fd) = open(&path, abi::O_CREATE | abi::O_RDWR | abi::O_TRUNC) else { exit(1) };
                if write_all(fd, &data).is_err() {
                    exit(2);
                }
                let _ = close(fd);
                let Ok(fd) = open(&path, abi::O_RDONLY) else { exit(3) };
                if read_to_end(fd).ok().as_deref() != Some(&data[..]) {
                    exit(4);
                }
                let _ = close(fd);
            }
            let _ = unlink(&path);
            exit(0);
        }
    }
    for _ in 0..N {
        let (pid, code) = ok(wait(), "wait")?;
        ensure!(code == 0, "writer {pid} failed with {code}");
    }
    Ok(())
}

fn fd_table() -> TestResult {
    let mut fds = Vec::new();
    loop {
        match open("/README", abi::O_RDONLY) {
            Ok(fd) => fds.push(fd),
            Err(e) => {
                ensure!(e == errno::EMFILE, "expected EMFILE, got {}", errno::name(e));
                break;
            }
        }
    }
    ensure!(fds.len() >= 10, "only {} fds available", fds.len());
    for fd in fds {
        ok(close(fd), "close")?;
    }
    // dup shares the offset.
    let fd = ok(open("/README", abi::O_RDONLY), "open")?;
    let fd2 = ok(dup(fd), "dup")?;
    let mut a = [0u8; 5];
    let mut b = [0u8; 5];
    ok(read(fd, &mut a), "read")?;
    ok(read(fd2, &mut b), "read")?;
    ensure!(a != b, "dup'd fds should share the file offset");
    ok(close(fd), "close")?;
    ok(close(fd2), "close")?;
    Ok(())
}

fn parallel_cpu() -> TestResult {
    let ncpus = sysinfo().ncpus as usize;
    for _ in 0..ncpus * 2 {
        if ok(fork(), "fork")? == 0 {
            let mut x = 1u64;
            for i in 0..2_000_000u64 {
                x = x.wrapping_mul(i | 1).rotate_left(5);
            }
            core::hint::black_box(x);
            exit(0);
        }
    }
    for _ in 0..ncpus * 2 {
        ok(wait(), "wait")?;
    }
    Ok(())
}

fn no_memory_leak() -> TestResult {
    // Warm the kernel's caches once, then a second identical round must
    // give back every page it took.
    fn round() -> TestResult {
        for _ in 0..10 {
            in_child(|| {
                let (r, w) = pipe().unwrap();
                let _ = write(w, b"leak?");
                let _ = close(r);
                let _ = close(w);
                let _ = sbrk(256 * 1024);
                exec("/bin/true", &["true"]) as i32
            })?;
        }
        Ok(())
    }
    round()?;
    let before = sysinfo().free_pages;
    round()?;
    let after = sysinfo().free_pages;
    ensure!(after == before, "{} pages leaked", before as i64 - after as i64);
    Ok(())
}

const TESTS: &[Test] = &[
    ("fork_wait", fork_wait),
    ("fork_many", fork_many),
    ("fork_exhaustion", fork_exhaustion),
    ("exec_args", exec_args),
    ("pipe_data", pipe_data),
    ("pipe_broken", pipe_broken),
    ("sbrk_grow_shrink", sbrk_grow_shrink),
    ("bad_access_kills", bad_access_kills),
    ("bad_pointers", bad_pointers),
    ("kill_child", kill_child),
    ("sleep_time", sleep_time),
    ("file_roundtrip", file_roundtrip),
    ("disk_space_reclaimed", disk_space_reclaimed),
    ("directories", directories),
    ("concurrent_files", concurrent_files),
    ("fd_table", fd_table),
    ("parallel_cpu", parallel_cpu),
    ("no_memory_leak", no_memory_leak),
];

#[unsafe(no_mangle)]
fn main(args: &[&str]) -> i32 {
    if args.get(1) == Some(&"--exec-child") {
        return if args[2..] == ["hello", "world"] { 42 } else { 1 };
    }
    let only = args.get(1).copied();
    let _ = mkdir("/tmp");
    let mut failed = 0;
    let mut ran = 0;
    for &(name, test) in TESTS {
        if only.is_some_and(|o| o != name) {
            continue;
        }
        ran += 1;
        let t0 = time_us();
        // Each test in its own process; a crash shows up as a non-zero code.
        let result = match fork() {
            Ok(0) => exit(match test() {
                Ok(()) => 0,
                Err(msg) => {
                    eprintln!("  {name}: {msg}");
                    1
                }
            }),
            Ok(_) => match wait() {
                Ok((_, 0)) => Ok(()),
                Ok((_, code)) => Err(code),
                Err(e) => Err(-(e as i32)),
            },
            Err(e) => Err(-(e as i32)),
        };
        let ms = (time_us() - t0) / 1000;
        match result {
            Ok(()) => println!("test {name}: OK ({ms} ms)"),
            Err(code) => {
                println!("test {name}: FAILED (exit {code})");
                failed += 1;
            }
        }
    }
    if failed == 0 {
        println!("ALL TESTS PASSED ({ran} tests)");
        0
    } else {
        println!("{failed} of {ran} TESTS FAILED");
        1
    }
}
