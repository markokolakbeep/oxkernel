//! System call dispatch. Arguments arrive in a0..a5, the number in a7; the
//! result (or a negated errno) goes back in a0.

mod fs;
mod proc;

use alloc::sync::Arc;

use oxcore::abi::{errno::*, sys};

use crate::KResult;
use crate::fs::File;
use crate::mm::AddressSpace;
use crate::task;
use crate::trap::TrapFrame;

pub fn dispatch(tf: &mut TrapFrame) -> isize {
    let a = [tf.x[10], tf.x[11], tf.x[12], tf.x[13], tf.x[14], tf.x[15]];
    let r: KResult<usize> = match tf.x[17] {
        sys::EXIT => task::exit(a[0] as i32),
        sys::FORK => task::fork(tf).map(|p| p as usize),
        sys::WAIT => task::wait(a[0]).map(|p| p as usize),
        sys::EXEC => proc::sys_exec(tf, a[0], a[1]),
        sys::GETPID => Ok(task::current().pid as usize),
        sys::SBRK => proc::sys_sbrk(a[0] as isize),
        sys::SLEEP => proc::sys_sleep(a[0] as u64),
        sys::TIME_US => Ok(crate::timer::now_us() as usize),
        sys::YIELD => {
            task::yield_now();
            Ok(0)
        }
        sys::KILL => task::kill(a[0] as i32).map(|_| 0),
        sys::READ => fs::sys_read(a[0], a[1], a[2]),
        sys::WRITE => fs::sys_write(a[0], a[1], a[2]),
        sys::OPEN => fs::sys_open(a[0], a[1]),
        sys::CLOSE => fs::sys_close(a[0]),
        sys::DUP => fs::sys_dup(a[0]),
        sys::PIPE => fs::sys_pipe(a[0]),
        sys::FSTAT => fs::sys_fstat(a[0], a[1]),
        sys::MKDIR => fs::sys_mkdir(a[0]),
        sys::CHDIR => fs::sys_chdir(a[0]),
        sys::UNLINK => fs::sys_unlink(a[0]),
        sys::PROCINFO => proc::sys_procinfo(a[0], a[1]),
        sys::SETPGID => proc::sys_setpgid(),
        sys::SETFG => proc::sys_setfg(a[0] as i32),
        sys::SYSINFO => proc::sys_sysinfo(a[0]),
        sys::SHUTDOWN => proc::sys_shutdown(a[0] as u32),
        _ => Err(ENOSYS),
    };
    match r {
        Ok(v) => v as isize,
        Err(e) => -e,
    }
}

/// Runs `f` on the current task's address space.
fn with_aspace<R>(f: impl FnOnce(&mut AddressSpace) -> R) -> R {
    let t = task::current();
    let mut inner = t.inner.lock();
    f(&mut inner.aspace)
}

fn get_file(fd: usize) -> KResult<Arc<File>> {
    let t = task::current();
    let inner = t.inner.lock();
    inner.files.get(fd).and_then(|f| f.clone()).ok_or(EBADF)
}

/// Installs `f` in the lowest free descriptor.
fn alloc_fd(f: Arc<File>) -> KResult<usize> {
    let t = task::current();
    let mut inner = t.inner.lock();
    let fd = inner.files.iter().position(|f| f.is_none()).ok_or(EMFILE)?;
    inner.files[fd] = Some(f);
    Ok(fd)
}
