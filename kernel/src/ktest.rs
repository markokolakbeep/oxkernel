//! In-kernel tests, built with `--features ktest`. They run in pid 1 after
//! the file system is mounted (instead of exec'ing /bin/init), then power off
//! with the result. `cargo xtask test` runs them.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use oxcore::PAGE_SIZE;
use oxcore::abi::T_FILE;
use oxcore::fs::BLOCK_SIZE;

use crate::arch::csr;
use crate::drivers::virtio_blk;
use crate::fs::{self, File};
use crate::mm::paging::{PTE_R, PTE_W};
use crate::mm::{AddressSpace, frame, heap};
use crate::sync::{SpinLock, pop_off, push_off};
use crate::task::cpu::{HARTS, ONLINE, mycpu};
use crate::{power, timer};

type TestResult = Result<(), String>;
type Test = (&'static str, fn() -> TestResult);

macro_rules! check {
    ($cond:expr) => {
        if !$cond {
            return Err(format!("check failed: {} ({}:{})", stringify!($cond), file!(), line!()));
        }
    };
}

fn frames_roundtrip() -> TestResult {
    // Allocate the Vec first: growing it would take heap pages mid-count.
    let mut v = Vec::with_capacity(8);
    let (free0, _) = frame::stats();
    for order in [0, 0, 1, 3, 5, 0, 2] {
        v.push((frame::alloc_pages(order).ok_or("out of frames")?, order));
    }
    for &(pa, order) in &v {
        check!(pa % (PAGE_SIZE << order) == 0);
    }
    for (pa, order) in v {
        unsafe { frame::free_pages(pa, order) };
    }
    check!(frame::stats().0 == free0);
    Ok(())
}

fn heap_alloc() -> TestResult {
    let before = heap::in_use();
    {
        let small: Vec<Box<u64>> = (0..1000).map(Box::new).collect();
        let big = alloc::vec![0xa5u8; 64 * 1024];
        check!(small.iter().enumerate().all(|(i, b)| **b == i as u64));
        check!(big.iter().all(|&b| b == 0xa5));
        check!(heap::in_use() > before);
    }
    check!(heap::in_use() == before);
    Ok(())
}

fn address_space_copy() -> TestResult {
    let mut a = AddressSpace::new().map_err(|_| "no memory")?;
    let va = 0x40_0000;
    a.map_zeroed(va, PTE_R | PTE_W).map_err(|_| "map failed")?;
    a.map_zeroed(va + PAGE_SIZE, PTE_R | PTE_W).map_err(|_| "map failed")?;
    // Straddles a page boundary.
    let msg = b"hello across a page boundary";
    a.copy_out(va + PAGE_SIZE - 5, msg).map_err(|_| "copy_out failed")?;
    let mut back = [0u8; 28];
    a.copy_in(&mut back, va + PAGE_SIZE - 5).map_err(|_| "copy_in failed")?;
    check!(&back == msg);
    check!(a.copy_out(va + 2 * PAGE_SIZE, b"x").is_err());
    check!(a.copy_in(&mut back, 0xffff_ffc0_8020_0000).is_err());
    let b = a.fork().map_err(|_| "fork failed")?;
    let mut fb = [0u8; 28];
    b.copy_in(&mut fb, va + PAGE_SIZE - 5).map_err(|_| "copy_in failed")?;
    check!(&fb == msg);
    check!(b.resident() == 2 * PAGE_SIZE);
    let (free0, _) = frame::stats();
    drop(b);
    check!(frame::stats().0 > free0);
    Ok(())
}

fn interrupt_nesting() -> TestResult {
    let was = csr::intr_get();
    push_off();
    push_off();
    check!(!csr::intr_get());
    pop_off();
    check!(!csr::intr_get());
    pop_off();
    check!(csr::intr_get() == was);
    let l = SpinLock::new("test", 5);
    {
        let g = l.lock();
        check!(*g == 5 && l.holding() && mycpu().noff.get() >= 1);
    }
    check!(!l.holding());
    Ok(())
}

fn disk_raw() -> TestResult {
    let last = virtio_blk::capacity_blocks() as u32 - 1;
    let mut buf: Box<[u8; BLOCK_SIZE]> = Box::new([0; BLOCK_SIZE]);
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i * 13 % 256) as u8;
    }
    virtio_blk::rw(last, &mut buf, true);
    let mut back: Box<[u8; BLOCK_SIZE]> = Box::new([0; BLOCK_SIZE]);
    virtio_blk::rw(last, &mut back, false);
    check!(buf[..] == back[..]);
    Ok(())
}

fn fs_file() -> TestResult {
    let ip = fs::create("/ktest-file", T_FILE, 0).map_err(|e| format!("create: {e}"))?;
    let f = File::inode(ip, true, true);
    let data: Vec<u8> = (0..20_000).map(|i| (i % 199) as u8).collect();
    check!(f.write(&data) == Ok(data.len()));
    drop(f);
    let f = File::inode(fs::namei("/ktest-file").map_err(|e| format!("namei: {e}"))?, true, false);
    let mut back = alloc::vec![0u8; 25_000];
    check!(f.read(&mut back) == Ok(data.len()));
    check!(back[..data.len()] == data[..]);
    drop(f);
    fs::unlink("/ktest-file").map_err(|e| format!("unlink: {e}"))?;
    check!(fs::namei("/ktest-file").is_err());
    check!(fs::namei("/bin/sh").is_ok());
    Ok(())
}

fn pipe_basic() -> TestResult {
    let (r, w) = File::pipe();
    check!(w.write(b"ping") == Ok(4));
    let mut buf = [0u8; 8];
    check!(r.read(&mut buf) == Ok(4));
    check!(&buf[..4] == b"ping");
    drop(w);
    check!(r.read(&mut buf) == Ok(0));
    Ok(())
}

fn timer_sleep() -> TestResult {
    let t0 = timer::now_us();
    check!(timer::sleep_ms(30));
    let dt = timer::now_us() - t0;
    check!((30_000..2_000_000).contains(&dt));
    Ok(())
}

fn smp_online() -> TestResult {
    // Every hart in the devicetree should come up within a second.
    let want = HARTS.load(Ordering::SeqCst);
    for _ in 0..100 {
        if ONLINE.load(Ordering::SeqCst) >= want {
            return Ok(());
        }
        timer::sleep_ms(10);
    }
    Err(format!("only {} of {want} harts online", ONLINE.load(Ordering::SeqCst)))
}

const TESTS: &[Test] = &[
    ("frames_roundtrip", frames_roundtrip),
    ("heap_alloc", heap_alloc),
    ("address_space_copy", address_space_copy),
    ("interrupt_nesting", interrupt_nesting),
    ("disk_raw", disk_raw),
    ("fs_file", fs_file),
    ("pipe_basic", pipe_basic),
    ("timer_sleep", timer_sleep),
    ("smp_online", smp_online),
];

pub fn run() -> ! {
    let mut failed = 0;
    for (name, test) in TESTS {
        match test() {
            Ok(()) => println!("ktest {name}: OK"),
            Err(msg) => {
                println!("ktest {name}: FAILED: {msg}");
                failed += 1;
            }
        }
    }
    if failed == 0 {
        println!("ktest: all passed ({} tests)", TESTS.len());
        power::shutdown(0)
    }
    println!("ktest: {failed} failed");
    power::shutdown(1)
}
