//! Tasks (processes) and scheduling.
//!
//! Every process has one thread, so "task" and "process" mean the same thing.
//! Lock order: WAIT_LOCK -> TABLE -> Task::inner -> SCHED.

pub mod context;
pub mod cpu;
mod exec;
pub mod kstack;
mod proc;
mod sched;

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

pub use exec::exec;
pub use proc::*;
pub use sched::*;

use crate::config::NOFILE;
use crate::fs::{File, InodeRef};
use crate::mm::AddressSpace;
use crate::sync::SpinLock;
use crate::trap::{TF_SIZE, TrapFrame};
use context::Context;
use kstack::KernelStack;

pub type Pid = i32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Runnable,
    Running,
    Sleeping,
    Zombie,
}

/// Scheduler state of a task, protected by the SCHED lock.
pub struct SchedInfo {
    pub state: State,
    pub chan: usize,
    pub hart: i32,
}

pub struct Task {
    pub pid: Pid,
    kstack: KernelStack,
    context: UnsafeCell<Context>,
    sched: UnsafeCell<SchedInfo>,
    /// satp of the address space, readable without the inner lock.
    pub satp: AtomicUsize,
    pub killed: AtomicBool,
    pub pgid: AtomicI32,
    pub inner: SpinLock<TaskInner>,
}

// `context` is only touched by the hart running or switching to the task and
// `sched` only under SCHED.
unsafe impl Sync for Task {}
unsafe impl Send for Task {}

pub struct TaskInner {
    pub name: [u8; 16],
    pub aspace: AddressSpace,
    pub files: [Option<Arc<File>>; NOFILE],
    pub cwd: Option<InodeRef>,
    pub parent: Weak<Task>,
    pub children: Vec<Arc<Task>>,
    pub exit_code: i32,
}

impl Task {
    fn new(pid: Pid, entry: extern "C" fn() -> !, inner: TaskInner) -> Option<Arc<Self>> {
        let kstack = KernelStack::alloc()?;
        let tf_addr = kstack.top() - TF_SIZE;
        let mut ctx = Context::zero();
        ctx.ra = entry as *const () as usize;
        ctx.sp = tf_addr;
        let satp = inner.aspace.satp();
        Some(Arc::new(Self {
            pid,
            kstack,
            context: UnsafeCell::new(ctx),
            sched: UnsafeCell::new(SchedInfo { state: State::Runnable, chan: 0, hart: -1 }),
            satp: AtomicUsize::new(satp),
            killed: AtomicBool::new(false),
            pgid: AtomicI32::new(pid),
            inner: SpinLock::new("task", inner),
        }))
    }

    /// The user trap frame at the top of the kernel stack.
    pub fn trapframe(&self) -> *mut TrapFrame {
        (self.kstack.top() - TF_SIZE) as *mut TrapFrame
    }

    /// # Safety
    /// SCHED must be held.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn sched_info(&self) -> &mut SchedInfo {
        unsafe { &mut *self.sched.get() }
    }

    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::Relaxed)
    }

    pub fn name(&self) -> alloc::string::String {
        let inner = self.inner.lock();
        let end = inner.name.iter().position(|&c| c == 0).unwrap_or(16);
        alloc::string::String::from_utf8_lossy(&inner.name[..end]).into_owned()
    }
}

pub fn set_name(inner: &mut TaskInner, name: &str) {
    inner.name = [0; 16];
    let n = name.len().min(15);
    inner.name[..n].copy_from_slice(&name.as_bytes()[..n]);
}
