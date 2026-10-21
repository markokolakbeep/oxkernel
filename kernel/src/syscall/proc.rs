//! Process and system calls.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use oxcore::abi::{ProcInfo, SysInfo, errno::*};
use oxcore::page_round_up;

use super::with_aspace;
use crate::KResult;
use crate::arch;
use crate::config::*;
use crate::console::FOREGROUND;
use crate::mm::paging::{PTE_R, PTE_W};
use crate::task::{self, cpu::ONLINE};
use crate::trap::TrapFrame;
use crate::{mm, timer};

pub fn sys_exec(tf: &mut TrapFrame, path: usize, argv: usize) -> KResult<usize> {
    let (path, args) = with_aspace(|a| -> KResult<(String, Vec<String>)> {
        let path = a.copy_in_str(path, MAX_PATH)?;
        let mut args = Vec::new();
        let mut bytes = 0;
        loop {
            let p: usize = a.copy_in_val(argv + args.len() * 8)?;
            if p == 0 {
                break;
            }
            if args.len() == MAX_ARGS {
                return Err(E2BIG);
            }
            let s = a.copy_in_str(p, MAX_ARG_BYTES)?;
            bytes += s.len() + 1;
            if bytes > MAX_ARG_BYTES {
                return Err(E2BIG);
            }
            args.push(s);
        }
        Ok((path, args))
    })?;
    task::exec(tf, &path, &args)
}

pub fn sys_sbrk(incr: isize) -> KResult<usize> {
    with_aspace(|a| {
        let old = a.brk;
        let new = old.checked_add_signed(incr).ok_or(ENOMEM)?;
        if new < a.heap_start || new > USER_HEAP_LIMIT {
            return Err(ENOMEM);
        }
        if new > old {
            let mut va = page_round_up(old);
            while va < new {
                if let Err(e) = a.map_zeroed(va, PTE_R | PTE_W) {
                    a.unmap_range(page_round_up(old), va);
                    return Err(e);
                }
                va += oxcore::PAGE_SIZE;
            }
        } else if new < old {
            a.unmap_range(page_round_up(new), page_round_up(old));
            arch::sfence_vma_all();
        }
        a.brk = new;
        Ok(old)
    })
}

pub fn sys_sleep(ms: u64) -> KResult<usize> {
    if timer::sleep_ms(ms) { Ok(0) } else { Err(EINTR) }
}

pub fn sys_procinfo(buf: usize, max: usize) -> KResult<usize> {
    let infos = task::procinfo();
    let n = infos.len().min(max);
    with_aspace(|a| {
        for (i, info) in infos[..n].iter().enumerate() {
            a.copy_out_val(buf + i * size_of::<ProcInfo>(), info)?;
        }
        Ok(n)
    })
}

/// Puts the caller in a new process group named after its pid.
pub fn sys_setpgid() -> KResult<usize> {
    let t = task::current();
    t.pgid.store(t.pid, Ordering::Relaxed);
    Ok(t.pid as usize)
}

/// Sets the process group the console sends ^C to.
pub fn sys_setfg(pgid: i32) -> KResult<usize> {
    FOREGROUND.store(pgid, Ordering::Relaxed);
    Ok(0)
}

pub fn sys_sysinfo(buf: usize) -> KResult<usize> {
    let (free, total) = mm::frame::stats();
    let info = SysInfo {
        ncpus: ONLINE.load(Ordering::Relaxed) as u32,
        nprocs: task::nprocs() as u32,
        free_pages: free as u64,
        total_pages: total as u64,
        uptime_us: timer::now_us(),
        timebase_hz: timer::timebase_hz(),
    };
    with_aspace(|a| a.copy_out_val(buf, &info))?;
    Ok(0)
}

/// Powers off; only init may do this.
pub fn sys_shutdown(code: u32) -> KResult<usize> {
    if task::current().pid != 1 {
        return Err(EPERM);
    }
    crate::drivers::virtio_blk::flush();
    println!("oxkernel: shutdown (code {code})");
    crate::power::shutdown(code)
}
