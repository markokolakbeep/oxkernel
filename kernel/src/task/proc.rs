//! Process lifecycle: creating init, fork, exit, wait and kill.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI32, Ordering};

use oxcore::abi::{self, ProcInfo, errno::*};

use super::sched::{SCHED, exit_switch, make_runnable, state_of, wake_pid, wakeup};
use super::{Pid, State, Task, TaskInner, current, fork_return};
use crate::KResult;
use crate::mm::AddressSpace;
use crate::sync::{Once, SpinLock};
use crate::trap::TrapFrame;

/// Every live task, including zombies not yet reaped.
static TABLE: SpinLock<Vec<Arc<Task>>> = SpinLock::new("proctable", Vec::new());
/// Serializes exit against wait so a parent cannot miss a child's exit.
static WAIT_LOCK: SpinLock<()> = SpinLock::new("wait", ());
static NEXT_PID: AtomicI32 = AtomicI32::new(1);
static INIT: Once<Arc<Task>> = Once::new();

fn empty_files() -> [Option<Arc<crate::fs::File>>; crate::config::NOFILE] {
    Default::default()
}

/// Creates pid 1. Its first kernel code is `entry`, which sets up the file
/// system and execs /bin/init.
pub fn spawn_init(entry: extern "C" fn() -> !) {
    let inner = TaskInner {
        name: *b"init\0\0\0\0\0\0\0\0\0\0\0\0",
        aspace: AddressSpace::new().expect("init address space"),
        files: empty_files(),
        cwd: None,
        parent: Weak::new(),
        children: Vec::new(),
        exit_code: 0,
    };
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    let task = Task::new(pid, entry, inner).expect("init task");
    INIT.set(task.clone());
    TABLE.lock().push(task.clone());
    make_runnable(task);
}

pub fn fork(tf: &TrapFrame) -> KResult<Pid> {
    let parent = current();
    let child_inner = {
        let inner = parent.inner.lock();
        TaskInner {
            name: inner.name,
            aspace: inner.aspace.fork()?,
            files: inner.files.clone(),
            cwd: inner.cwd.clone(),
            parent: Arc::downgrade(&parent),
            children: Vec::new(),
            exit_code: 0,
        }
    };
    let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
    // Building the task can fail if every kernel stack is in use; the child's
    // resources are dropped here, outside any lock.
    let child = Task::new(pid, fork_return, child_inner).ok_or(EAGAIN)?;
    child.pgid.store(parent.pgid.load(Ordering::Relaxed), Ordering::Relaxed);
    unsafe {
        let ctf = &mut *child.trapframe();
        *ctf = *tf;
        ctf.x[10] = 0; // fork returns 0 in the child
    }
    TABLE.lock().push(child.clone());
    parent.inner.lock().children.push(child.clone());
    make_runnable(child);
    Ok(pid)
}

pub fn exit(code: i32) -> ! {
    let task = current();
    if task.pid == 1 {
        panic!("init exited with code {code}");
    }
    // Drop files and cwd first: closing them may sleep on disk I/O.
    let (files, cwd) = {
        let mut inner = task.inner.lock();
        (core::mem::replace(&mut inner.files, empty_files()), inner.cwd.take())
    };
    drop(files);
    drop(cwd);

    let init = INIT.wait();
    let wait_guard = WAIT_LOCK.lock();
    let (children, parent) = {
        let mut inner = task.inner.lock();
        inner.exit_code = code;
        (core::mem::take(&mut inner.children), inner.parent.upgrade())
    };
    if !children.is_empty() {
        for c in &children {
            c.inner.lock().parent = Arc::downgrade(init);
        }
        init.inner.lock().children.extend(children);
        wakeup(Arc::as_ptr(init) as usize);
    }
    if let Some(p) = &parent {
        wakeup(Arc::as_ptr(p) as usize);
    }
    drop(parent);
    let s = SCHED.lock();
    drop(wait_guard);
    // `task` is the last Arc on our own stack; the table still holds one.
    drop(task);
    exit_switch(s)
}

/// Waits for a child to exit; returns its pid and stores its exit code at
/// `status` (a user address) if that is not 0.
pub fn wait(status: usize) -> KResult<Pid> {
    let task = current();
    let mut guard = WAIT_LOCK.lock();
    loop {
        let zombie = {
            let mut inner = task.inner.lock();
            if inner.children.is_empty() {
                return Err(ECHILD);
            }
            let found = inner.children.iter().position(|c| state_of(c).0 == State::Zombie);
            found.map(|i| inner.children.swap_remove(i))
        };
        if let Some(child) = zombie {
            drop(guard);
            let code = child.inner.lock().exit_code;
            TABLE.lock().retain(|t| t.pid != child.pid);
            let pid = child.pid;
            // Usually the last reference: frees the address space and stack.
            drop(child);
            if status != 0 {
                task.inner.lock().aspace.copy_out_val(status, &code)?;
            }
            return Ok(pid);
        }
        if task.is_killed() {
            return Err(EINTR);
        }
        guard = super::sleep(Arc::as_ptr(&task) as usize, guard);
    }
}

pub fn kill(pid: Pid) -> KResult<()> {
    let target = TABLE.lock().iter().find(|t| t.pid == pid).cloned().ok_or(ESRCH)?;
    if pid == 1 {
        return Err(EPERM);
    }
    target.killed.store(true, Ordering::Relaxed);
    wake_pid(pid);
    Ok(())
}

/// Kills every process in group `pgid` (the console's ^C).
pub fn kill_pgid(pgid: Pid) {
    if pgid <= 1 {
        return;
    }
    let targets: Vec<Arc<Task>> =
        TABLE.lock().iter().filter(|t| t.pgid.load(Ordering::Relaxed) == pgid).cloned().collect();
    for t in targets {
        if t.pid != 1 {
            t.killed.store(true, Ordering::Relaxed);
            wake_pid(t.pid);
        }
    }
}

pub fn procinfo() -> Vec<ProcInfo> {
    let tasks: Vec<Arc<Task>> = TABLE.lock().clone();
    tasks
        .iter()
        .map(|t| {
            let (ppid, mem, name) = {
                let inner = t.inner.lock();
                (inner.parent.upgrade().map_or(0, |p| p.pid), inner.aspace.resident() as u64, inner.name)
            };
            let (state, hart) = state_of(t);
            ProcInfo {
                pid: t.pid,
                ppid,
                pgid: t.pgid.load(Ordering::Relaxed),
                state: match state {
                    State::Runnable => abi::PROC_RUNNABLE,
                    State::Running => abi::PROC_RUNNING,
                    State::Sleeping => abi::PROC_SLEEPING,
                    State::Zombie => abi::PROC_ZOMBIE,
                },
                hart,
                _pad: 0,
                mem,
                name,
            }
        })
        .collect()
}

pub fn nprocs() -> usize {
    TABLE.lock().len()
}
