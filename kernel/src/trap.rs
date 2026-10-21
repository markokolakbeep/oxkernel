//! Trap handling for both U-mode and S-mode.

use core::sync::atomic::Ordering;

use crate::arch::csr::{self, SCAUSE_INTERRUPT};
use crate::drivers::plic;
use crate::task::{self, cpu::mycpu};
use crate::{syscall, timer};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TrapFrame {
    /// x0..x31; x0 is unused.
    pub x: [usize; 32],
    pub sstatus: usize,
    pub sepc: usize,
    /// The kernel's tp (hart id), stashed on the way out to user space.
    pub kernel_tp: usize,
    _pad: usize,
}

pub const TF_SIZE: usize = size_of::<TrapFrame>();
const _: () = assert!(TF_SIZE == 288 && TF_SIZE.is_multiple_of(16)); // trap.S has a copy

impl TrapFrame {
    pub const fn zeroed() -> Self {
        Self { x: [0; 32], sstatus: 0, sepc: 0, kernel_tp: 0, _pad: 0 }
    }
}

unsafe extern "C" {
    fn __trap_entry();
    fn __return_to_user(tf: *mut TrapFrame) -> !;
}

const IRQ_SOFT: usize = 1;
const IRQ_TIMER: usize = 5;
const IRQ_EXT: usize = 9;

const EXC_ECALL_U: usize = 8;

pub fn init_hart() {
    csr::set_stvec(__trap_entry as *const () as usize);
    csr::set_sscratch(0);
    csr::set_sie(csr::SIE_SEIE | csr::SIE_STIE | csr::SIE_SSIE);
    csr::enable_user_counters();
}

fn exception_name(code: usize) -> &'static str {
    match code {
        0 => "instruction address misaligned",
        1 => "instruction access fault",
        2 => "illegal instruction",
        3 => "breakpoint",
        4 => "load address misaligned",
        5 => "load access fault",
        6 => "store address misaligned",
        7 => "store access fault",
        8 => "ecall from U-mode",
        9 => "ecall from S-mode",
        12 => "instruction page fault",
        13 => "load page fault",
        15 => "store page fault",
        _ => "unknown exception",
    }
}

fn clear_soft_irq() {
    unsafe { core::arch::asm!("csrc sip, {}", in(reg) 1usize << 1) };
}

/// Interrupts common to both modes. Returns whether the current task should
/// give up the CPU.
fn handle_interrupt(code: usize) -> bool {
    match code {
        IRQ_SOFT => {
            clear_soft_irq();
            false
        }
        IRQ_TIMER => {
            timer::on_tick();
            true
        }
        IRQ_EXT => {
            plic::handle();
            false
        }
        _ => panic!("unexpected interrupt {code}"),
    }
}

#[unsafe(no_mangle)]
extern "C" fn kernel_trap(tf: &mut TrapFrame) {
    let scause = csr::scause();
    assert!(tf.sstatus & csr::SSTATUS_SPP != 0, "kernel_trap: not from S-mode");
    let code = scause & !SCAUSE_INTERRUPT;
    if scause & SCAUSE_INTERRUPT != 0 {
        // Preempt kernel code too, but only a task (not the scheduler loop)
        // and only when it held no spinlocks.
        if handle_interrupt(code) && mycpu().current().is_some() && mycpu().noff.get() == 0 {
            task::yield_now();
        }
        return;
    }
    let stval = csr::stval();
    if task::kstack::is_guard_page(stval) {
        panic!("kernel stack overflow (stval {stval:#x}, sepc {:#x})", tf.sepc);
    }
    panic!("kernel exception: {} (scause {scause:#x}) at sepc {:#x}, stval {stval:#x}", exception_name(code), tf.sepc);
}

#[unsafe(no_mangle)]
extern "C" fn user_trap(tf: *mut TrapFrame) -> ! {
    let tf = unsafe { &mut *tf };
    let scause = csr::scause();
    let code = scause & !SCAUSE_INTERRUPT;
    if scause & SCAUSE_INTERRUPT != 0 {
        if handle_interrupt(code) {
            task::yield_now();
        }
    } else if code == EXC_ECALL_U {
        tf.sepc += 4;
        csr::intr_on();
        let ret = syscall::dispatch(tf);
        tf.x[10] = ret as usize;
    } else {
        let t = task::current();
        println!(
            "pid {} ({}): {} at sepc {:#x}, stval {:#x}; killed",
            t.pid,
            t.name(),
            exception_name(code),
            tf.sepc,
            csr::stval()
        );
        t.killed.store(true, Ordering::Relaxed);
    }
    if task::current().is_killed() {
        task::exit(-1);
    }
    return_to_user()
}

/// Leaves the kernel for the current task's user code.
pub fn return_to_user() -> ! {
    let tf = task::current().trapframe();
    unsafe { __return_to_user(tf) }
}
