//! Micro-benchmarks. Times come from the `time` CSR, so under QEMU they
//! measure emulated time on the host machine, not real hardware.

#![no_std]
#![no_main]

use ulib::*;

struct Clock {
    hz: u64,
}

impl Clock {
    fn ns(&self, ticks: u64) -> u64 {
        (ticks as u128 * 1_000_000_000 / self.hz as u128) as u64
    }
}

fn report(name: &str, value: u64, unit: &str) {
    println!("{name:<34} {value:>10} {unit}");
}

fn null_syscall(c: &Clock) {
    const N: u64 = 20_000;
    let t0 = rdtime();
    for _ in 0..N {
        getpid();
    }
    report("null syscall (getpid)", c.ns(rdtime() - t0) / N, "ns");
}

/// yield with nothing else runnable: trap, switch to the scheduler and
/// straight back.
fn yield_alone(c: &Clock) {
    const N: u64 = 5_000;
    let t0 = rdtime();
    for _ in 0..N {
        yield_now();
    }
    report("yield (switch to scheduler and back)", c.ns(rdtime() - t0) / N, "ns");
}

/// Two processes yielding to each other: every yield switches address space,
/// so this includes the TLB flush and refill that `yield_alone` avoids.
fn yield_pair(c: &Clock) {
    const N: u64 = 2_000;
    match fork().unwrap() {
        0 => {
            for _ in 0..N {
                yield_now();
            }
            exit(0);
        }
        _ => {
            let t0 = rdtime();
            for _ in 0..N {
                yield_now();
            }
            // Each iteration is two switches: to the child and back.
            report("switch between two processes", c.ns(rdtime() - t0) / N / 2, "ns");
            let _ = wait();
        }
    }
}

/// Two processes bounce a byte over a pair of pipes. Each round trip is two
/// context switches plus four syscalls.
fn pipe_pingpong(c: &Clock) {
    const N: u64 = 2_000;
    let (r1, w1) = pipe().unwrap();
    let (r2, w2) = pipe().unwrap();
    let mut b = [0u8; 1];
    match fork().unwrap() {
        0 => {
            for _ in 0..N {
                if read(r1, &mut b) != Ok(1) {
                    exit(1);
                }
                let _ = write(w2, &b);
            }
            exit(0);
        }
        _ => {
            let t0 = rdtime();
            for _ in 0..N {
                let _ = write(w1, &b);
                let _ = read(r2, &mut b);
            }
            let dt = rdtime() - t0;
            let _ = wait();
            report("pipe ping-pong round trip", c.ns(dt) / N, "ns");
        }
    }
    for fd in [r1, w1, r2, w2] {
        let _ = close(fd);
    }
}

fn fork_exec_wait(c: &Clock) {
    const N: u64 = 100;
    let t0 = rdtime();
    for _ in 0..N {
        match fork().unwrap() {
            0 => {
                exec("/bin/true", &["true"]);
                exit(1);
            }
            _ => {
                let _ = wait();
            }
        }
    }
    report("fork + exec + wait", c.ns(rdtime() - t0) / N / 1000, "us");
}

fn fork_wait(c: &Clock) {
    const N: u64 = 200;
    let t0 = rdtime();
    for _ in 0..N {
        match fork().unwrap() {
            0 => exit(0),
            _ => {
                let _ = wait();
            }
        }
    }
    report("fork + exit + wait", c.ns(rdtime() - t0) / N / 1000, "us");
}

fn pipe_throughput(c: &Clock) {
    const TOTAL: usize = 8 << 20;
    let (r, w) = pipe().unwrap();
    match fork().unwrap() {
        0 => {
            let _ = close(r);
            let buf = vec![7u8; 4096];
            let mut sent = 0;
            while sent < TOTAL {
                let _ = write_all(w, &buf);
                sent += buf.len();
            }
            exit(0);
        }
        _ => {
            let _ = close(w);
            let mut buf = vec![0u8; 4096];
            let t0 = rdtime();
            let mut got = 0;
            while let Ok(n) = read(r, &mut buf) {
                if n == 0 {
                    break;
                }
                got += n;
            }
            let dt = c.ns(rdtime() - t0);
            let _ = wait();
            let _ = close(r);
            report("pipe throughput", (got as u64 * 1_000_000_000 / dt.max(1)) >> 20, "MiB/s");
        }
    }
}

fn file_io(c: &Clock) {
    const TOTAL: usize = 2 << 20;
    let buf = vec![3u8; 64 * 1024];
    let fd = open("/tmp/bench", abi::O_CREATE | abi::O_WRONLY | abi::O_TRUNC).unwrap();
    let t0 = rdtime();
    for _ in 0..TOTAL / buf.len() {
        write_all(fd, &buf).unwrap();
    }
    let wt = c.ns(rdtime() - t0);
    let _ = close(fd);
    let fd = open("/tmp/bench", abi::O_RDONLY).unwrap();
    let mut rbuf = vec![0u8; 64 * 1024];
    let t0 = rdtime();
    while read(fd, &mut rbuf).unwrap_or(0) > 0 {}
    let rt = c.ns(rdtime() - t0);
    let _ = close(fd);
    let _ = unlink("/tmp/bench");
    report("file write (write-through)", (TOTAL as u64 * 1_000_000_000 / wt.max(1)) >> 10, "KiB/s");
    report("file read (buffer cache + disk)", (TOTAL as u64 * 1_000_000_000 / rt.max(1)) >> 10, "KiB/s");
}

/// Wall time for `jobs` CPU-bound processes each doing the same work.
fn cpu_jobs(jobs: usize) -> u64 {
    let t0 = time_us();
    for _ in 0..jobs {
        if fork().unwrap() == 0 {
            let mut x = 1u64;
            for i in 0..20_000_000u64 {
                x = x.wrapping_mul(i | 1).rotate_left(7);
            }
            core::hint::black_box(x);
            exit(0);
        }
    }
    for _ in 0..jobs {
        let _ = wait();
    }
    time_us() - t0
}

fn smp_scaling() {
    let ncpus = sysinfo().ncpus as usize;
    let one = cpu_jobs(1);
    println!("scheduler scaling (same work per job, {ncpus} harts):");
    for jobs in 1..=ncpus.max(1) {
        let t = if jobs == 1 { one } else { cpu_jobs(jobs) };
        // Perfect scaling keeps the wall time flat while jobs <= harts.
        let speedup = jobs as u64 * one * 100 / t.max(1);
        println!("  {jobs} job(s): {:>7} us   throughput x{}.{:02}", t, speedup / 100, speedup % 100);
    }
}

#[unsafe(no_mangle)]
fn main(_args: &[&str]) -> i32 {
    let info = sysinfo();
    let c = Clock { hz: info.timebase_hz };
    let _ = mkdir("/tmp");
    println!("oxkernel bench ({} harts, timebase {} Hz; emulated time)", info.ncpus, info.timebase_hz);
    null_syscall(&c);
    yield_alone(&c);
    yield_pair(&c);
    pipe_pingpong(&c);
    fork_wait(&c);
    fork_exec_wait(&c);
    pipe_throughput(&c);
    file_io(&c);
    smp_scaling();
    0
}
