//! oxkernel: a small Unix-like kernel for RISC-V (QEMU `virt`).

#![no_std]
#![no_main]

extern crate alloc;

#[macro_use]
mod console;
mod arch;
mod config;
mod drivers;
mod fs;
#[cfg(feature = "ktest")]
mod ktest;
mod mm;
mod panic;
mod power;
mod sync;
mod syscall;
mod task;
mod timer;
mod trap;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use oxcore::fdt::Fdt;

use arch::sbi;
use config::KERNEL_OFFSET;
use sync::Once;
use task::cpu::{BOOT_HART, HARTS, ONLINE};

/// Kernel result: the error is a positive errno.
pub type KResult<T> = Result<T, isize>;

/// The kernel command line (QEMU's -append), copied out of the devicetree.
static BOOTARGS: Once<String> = Once::new();

unsafe extern "C" {
    fn _start_secondary();
}

#[unsafe(no_mangle)]
extern "C" fn kmain(hartid: usize, dtb_pa: usize) -> ! {
    BOOT_HART.store(hartid, Ordering::Relaxed);
    drivers::uart::init();
    println!();
    println!("oxkernel {} booting on hart {hartid}", env!("CARGO_PKG_VERSION"));

    // The boot page table direct-maps the first 4 GiB, so the DTB is
    // reachable already.
    let fdt = unsafe { Fdt::from_ptr(mm::phys_to_virt(dtb_pa) as *const u8) }.expect("bad devicetree");
    let info = fdt.machine_info().expect("unreadable devicetree");
    let dtb_range = (dtb_pa, dtb_pa + fdt.total_size());
    let (harts, nharts) = (info.harts, info.nharts);
    HARTS.store(nharts.min(config::MAX_HARTS), Ordering::Relaxed);
    let bootargs_raw = {
        let mut buf = [0u8; 256];
        let n = info.bootargs.len().min(buf.len());
        buf[..n].copy_from_slice(&info.bootargs.as_bytes()[..n]);
        (buf, n)
    };
    println!(
        "fdt: {} MiB RAM at {:#x}, {} harts, timebase {} Hz",
        info.mem_size >> 20,
        info.mem_start,
        nharts,
        info.timebase_hz
    );

    mm::init(info.mem_start, info.mem_size, dtb_range);
    BOOTARGS.set(String::from_utf8_lossy(&bootargs_raw.0[..bootargs_raw.1]).into_owned());

    trap::init_hart();
    drivers::plic::init();
    drivers::plic::init_hart();
    timer::init(info.timebase_hz);
    drivers::virtio_blk::init();
    fs::init_cache();
    task::spawn_init(init_task_entry);

    ONLINE.fetch_add(1, Ordering::SeqCst);
    let entry = _start_secondary as *const () as usize - KERNEL_OFFSET;
    for &h in &harts[..nharts] {
        if h != hartid && h < config::MAX_HARTS {
            let r = sbi::hart_start(h, entry, 0);
            if r != 0 {
                println!("smp: hart {h} failed to start ({r})");
            }
        }
    }
    timer::start_hart();
    task::scheduler()
}

#[unsafe(no_mangle)]
extern "C" fn kmain_secondary(hartid: usize) -> ! {
    mm::paging::load_satp(mm::paging::kernel_satp());
    trap::init_hart();
    drivers::plic::init_hart();
    timer::start_hart();
    let n = ONLINE.fetch_add(1, Ordering::SeqCst) + 1;
    println!("smp: hart {hartid} online ({n} up)");
    task::scheduler()
}

/// First code of pid 1: mount the file system, then become /bin/init.
#[cfg_attr(feature = "ktest", allow(unreachable_code))]
extern "C" fn init_task_entry() -> ! {
    task::finish_first_switch();
    fs::mount();
    task::current().inner.lock().cwd = Some(fs::iget(oxcore::fs::ROOT_INO));

    #[cfg(feature = "ktest")]
    ktest::run();

    // bootargs become init's arguments: `-append "test"` gives `init test`.
    let mut args: Vec<String> = Vec::from(["init".to_string()]);
    args.extend(BOOTARGS.wait().split_whitespace().map(String::from));
    let tf = unsafe { &mut *task::current().trapframe() };
    match task::exec(tf, "/bin/init", &args) {
        Ok(argc) => tf.x[10] = argc,
        Err(e) => panic!("exec /bin/init failed: {}", oxcore::abi::errno::name(e)),
    }
    trap::return_to_user()
}
