//! The scheduler: one global run queue under the SCHED lock, a scheduler loop
//! per hart, and xv6-style sleep/wakeup on channels.
//!
//! A task switching away holds SCHED across `__switch`; the scheduler loop
//! that resumes releases it, and the reverse on the way back. A task only
//! goes back on the run queue (or the sleeper list) once its hart has fully
//! switched off its stack, so two harts can never run the same task.

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::context::__switch;
use super::cpu::{IDLE_MASK, mycpu};
use super::{State, Task};
use crate::arch::{self, csr, sbi};
use crate::mm::paging::{kernel_satp, load_satp, loaded_satp};
use crate::sync::{SpinLock, SpinLockGuard, pop_off, push_off};

pub struct Sched {
    runq: VecDeque<Arc<Task>>,
    sleepers: Vec<Arc<Task>>,
}

/// Length of the run queue, readable without the lock (for idle polling).
static RUNNABLE: AtomicUsize = AtomicUsize::new(0);

/// How long an idle hart polls for work before sleeping in `wfi`. Waking a
/// hart from `wfi` takes an IPI, which is slow; a short poll catches the
/// common case of a task that blocks and is woken again right away.
const IDLE_POLL_US: u64 = 50;

pub static SCHED: SpinLock<Sched> = SpinLock::new("sched", Sched { runq: VecDeque::new(), sleepers: Vec::new() });

/// The task running on this hart.
pub fn current() -> Arc<Task> {
    try_current().expect("no current task")
}

pub fn try_current() -> Option<Arc<Task>> {
    push_off();
    let t = mycpu().current().cloned();
    pop_off();
    t
}

/// Borrow of the current task without touching the refcount. The scheduler
/// keeps the task alive for as long as it runs on this hart.
fn current_ref() -> &'static Task {
    let t = mycpu().current().expect("no current task");
    unsafe { &*Arc::as_ptr(t) }
}

/// Kicks an idle hart so it notices new work before its next timer tick.
fn kick_idle_hart() {
    let idle = IDLE_MASK.load(Ordering::Relaxed) & !(1 << arch::hartid());
    if idle != 0 {
        sbi::send_ipi(1 << idle.trailing_zeros(), 0);
    }
}

/// Queues a newly created task.
pub fn make_runnable(t: Arc<Task>) {
    let mut s = SCHED.lock();
    unsafe { t.sched_info().state = State::Runnable };
    s.runq.push_back(t);
    RUNNABLE.fetch_add(1, Ordering::Relaxed);
    drop(s);
    kick_idle_hart();
}

/// Spins briefly waiting for runnable work. Returns true if some appeared.
fn idle_poll() -> bool {
    let deadline = csr::time() + crate::timer::timebase_hz() * IDLE_POLL_US / 1_000_000;
    while csr::time() < deadline {
        if RUNNABLE.load(Ordering::Relaxed) > 0 {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

/// Per-hart scheduler loop.
pub fn scheduler() -> ! {
    loop {
        // Devices must be able to interrupt an idle hart.
        csr::intr_on();
        let mut s = SCHED.lock();
        let next = s.runq.pop_front();
        if next.is_some() {
            RUNNABLE.fetch_sub(1, Ordering::Relaxed);
        }
        let Some(task) = next else {
            IDLE_MASK.fetch_or(1 << arch::hartid(), Ordering::Relaxed);
            drop(s);
            if idle_poll() {
                IDLE_MASK.fetch_and(!(1 << arch::hartid()), Ordering::Relaxed);
                continue;
            }
            // Don't sit on a task's page table while idle.
            if loaded_satp() != kernel_satp() {
                csr::intr_off();
                load_satp(kernel_satp());
                csr::intr_on();
            }
            arch::wfi();
            IDLE_MASK.fetch_and(!(1 << arch::hartid()), Ordering::Relaxed);
            continue;
        };
        let satp = task.satp.load(Ordering::Relaxed);
        let ran_here_last = unsafe {
            let si = task.sched_info();
            let same = si.hart == arch::hartid() as i32;
            si.state = State::Running;
            si.hart = arch::hartid() as i32;
            same
        };
        // Lazy switch: if this hart still has the task's table loaded and
        // the task has not run anywhere else since, the TLB is still valid
        // for it. A task only changes its own mappings, and always flushes
        // the hart it runs on.
        if !(ran_here_last && loaded_satp() == satp) {
            load_satp(satp);
        }
        let cpu = mycpu();
        let ctx = task.context.get();
        cpu.set_current(Some(task));
        // The task inherits SCHED and releases it.
        core::mem::forget(s);
        unsafe { __switch(cpu.context.get(), ctx) };

        // Back here with SCHED held by the task that switched out. Its page
        // table stays loaded unless it exited (its parent will free it).
        let task = mycpu().take_current().unwrap();
        let mut s = unsafe { SCHED.assume_locked() };
        let state = unsafe { task.sched_info().state };
        if state == State::Zombie {
            load_satp(kernel_satp());
        }
        let leftover = match state {
            State::Runnable => {
                s.runq.push_back(task);
                RUNNABLE.fetch_add(1, Ordering::Relaxed);
                None
            }
            State::Sleeping => {
                s.sleepers.push(task);
                None
            }
            // The parent reaps it.
            State::Zombie => Some(task),
            State::Running => panic!("scheduler: task {} still running", task.pid),
        };
        drop(s);
        drop(leftover);
    }
}

/// Switches from the current task to this hart's scheduler. The caller must
/// hold SCHED (and nothing else) and have updated the task's state.
fn sched(guard: SpinLockGuard<'_, Sched>) {
    let cpu = mycpu();
    assert!(cpu.noff.get() == 1, "sched: holding other locks");
    assert!(!csr::intr_get(), "sched: interrupts on");
    let task = current_ref();
    let intena = cpu.intena.get();
    core::mem::forget(guard);
    unsafe { __switch(task.context.get(), cpu.context.get()) };
    // Possibly on a different hart now.
    mycpu().intena.set(intena);
}

/// Gives up the CPU for one round.
pub fn yield_now() {
    let s = SCHED.lock();
    unsafe { current_ref().sched_info().state = State::Runnable };
    sched(s);
    unsafe { SCHED.force_unlock() };
}

/// Atomically releases `guard` and sleeps on `chan`; reacquires the lock
/// before returning. Wakeups cannot be lost: the waker needs SCHED, which is
/// taken before `guard` is released.
pub fn sleep<'a, T>(chan: usize, guard: SpinLockGuard<'a, T>) -> SpinLockGuard<'a, T> {
    let lock = guard.lock_ref();
    let s = SCHED.lock();
    drop(guard);
    unsafe {
        let si = current_ref().sched_info();
        si.state = State::Sleeping;
        si.chan = chan;
    }
    sched(s);
    unsafe { SCHED.force_unlock() };
    lock.lock()
}

/// Wakes every task sleeping on `chan`.
pub fn wakeup(chan: usize) {
    let mut s = SCHED.lock();
    let mut woke = false;
    let mut i = 0;
    while i < s.sleepers.len() {
        let si = unsafe { s.sleepers[i].sched_info() };
        if si.chan == chan {
            si.state = State::Runnable;
            si.chan = 0;
            let t = s.sleepers.swap_remove(i);
            s.runq.push_back(t);
            RUNNABLE.fetch_add(1, Ordering::Relaxed);
            woke = true;
        } else {
            i += 1;
        }
    }
    drop(s);
    if woke {
        kick_idle_hart();
    }
}

/// Wakes `pid` if it is sleeping, so it can notice it was killed.
pub fn wake_pid(pid: super::Pid) {
    let mut s = SCHED.lock();
    if let Some(i) = s.sleepers.iter().position(|t| t.pid == pid) {
        let t = s.sleepers.swap_remove(i);
        unsafe {
            let si = t.sched_info();
            si.state = State::Runnable;
            si.chan = 0;
        }
        s.runq.push_back(t);
        RUNNABLE.fetch_add(1, Ordering::Relaxed);
    }
    drop(s);
    kick_idle_hart();
}

/// Where a task stands, for `ps`.
pub fn state_of(t: &Task) -> (State, i32) {
    let _s = SCHED.lock();
    let si = unsafe { t.sched_info() };
    (si.state, si.hart)
}

/// First code a new task runs: drop the SCHED lock it inherited from the
/// scheduler and enter user space.
pub extern "C" fn fork_return() -> ! {
    unsafe { SCHED.force_unlock() };
    crate::trap::return_to_user()
}

/// Releases the inherited SCHED lock in a task's first kernel entry point.
pub fn finish_first_switch() {
    unsafe { SCHED.force_unlock() };
}

/// Marks the current task a zombie and leaves it for good. The caller holds
/// SCHED.
pub fn exit_switch(s: SpinLockGuard<'_, Sched>) -> ! {
    unsafe { current_ref().sched_info().state = State::Zombie };
    sched(s);
    unreachable!("zombie rescheduled");
}
