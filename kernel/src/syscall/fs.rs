//! File system calls.

use alloc::sync::Arc;
use alloc::vec;

use oxcore::PAGE_SIZE;
use oxcore::abi::errno::*;
use oxcore::abi::*;

use super::{alloc_fd, get_file, with_aspace};
use crate::KResult;
use crate::config::MAX_PATH;
use crate::fs::{self, File};
use crate::task;

fn user_path(va: usize) -> KResult<alloc::string::String> {
    with_aspace(|a| a.copy_in_str(va, MAX_PATH))
}

pub fn sys_read(fd: usize, buf: usize, len: usize) -> KResult<usize> {
    let f = get_file(fd)?;
    let mut kbuf = vec![0u8; len.min(PAGE_SIZE)];
    let mut total = 0;
    while total < len {
        let want = (len - total).min(kbuf.len());
        let n = match f.read(&mut kbuf[..want]) {
            Ok(n) => n,
            Err(e) if total == 0 => return Err(e),
            Err(_) => break,
        };
        with_aspace(|a| a.copy_out(buf + total, &kbuf[..n]))?;
        total += n;
        // Pipes and the console return what they have; files keep going.
        if n < want || !f.is_inode() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_write(fd: usize, buf: usize, len: usize) -> KResult<usize> {
    let f = get_file(fd)?;
    let mut kbuf = vec![0u8; len.min(PAGE_SIZE)];
    let mut total = 0;
    while total < len {
        let n = (len - total).min(kbuf.len());
        with_aspace(|a| a.copy_in(&mut kbuf[..n], buf + total))?;
        match f.write(&kbuf[..n]) {
            Ok(w) => {
                total += w;
                if w < n {
                    break;
                }
            }
            Err(e) if total == 0 => return Err(e),
            Err(_) => break,
        }
    }
    Ok(total)
}

pub fn sys_open(path: usize, flags: usize) -> KResult<usize> {
    let path = user_path(path)?;
    let ip = if flags & O_CREATE != 0 { fs::create(&path, T_FILE, 0)? } else { fs::namei(&path)? };
    let readable = flags & O_WRONLY == 0;
    let writable = flags & (O_WRONLY | O_RDWR) != 0;
    let file = {
        let mut g = ip.lock();
        let d = *g.d();
        if d.kind == T_DIR && writable {
            return Err(EISDIR);
        }
        if d.kind == T_DEV {
            if d.major != CONSOLE_MAJOR {
                return Err(ENOENT);
            }
            None
        } else {
            if flags & O_TRUNC != 0 && d.kind == T_FILE {
                g.truncate();
            }
            Some(())
        }
    };
    let file = match file {
        Some(()) => File::inode(ip, readable, writable),
        None => File::console(readable, writable),
    };
    alloc_fd(Arc::new(file))
}

pub fn sys_close(fd: usize) -> KResult<usize> {
    let t = task::current();
    let f = {
        let mut inner = t.inner.lock();
        inner.files.get_mut(fd).and_then(|f| f.take()).ok_or(EBADF)?
    };
    // Dropping the last reference may close a pipe end or free an inode;
    // do it outside the task lock.
    drop(f);
    Ok(0)
}

pub fn sys_dup(fd: usize) -> KResult<usize> {
    alloc_fd(get_file(fd)?)
}

pub fn sys_pipe(fds: usize) -> KResult<usize> {
    let (r, w) = File::pipe();
    let rfd = alloc_fd(Arc::new(r))?;
    let wfd = match alloc_fd(Arc::new(w)) {
        Ok(fd) => fd,
        Err(e) => {
            sys_close(rfd)?;
            return Err(e);
        }
    };
    let pair = [rfd as i32, wfd as i32];
    if let Err(e) = with_aspace(|a| a.copy_out_val(fds, &pair)) {
        sys_close(rfd)?;
        sys_close(wfd)?;
        return Err(e);
    }
    Ok(0)
}

pub fn sys_fstat(fd: usize, st: usize) -> KResult<usize> {
    let stat = get_file(fd)?.stat();
    with_aspace(|a| a.copy_out_val(st, &stat))?;
    Ok(0)
}

pub fn sys_mkdir(path: usize) -> KResult<usize> {
    let path = user_path(path)?;
    fs::create(&path, T_DIR, 0)?;
    Ok(0)
}

pub fn sys_chdir(path: usize) -> KResult<usize> {
    let path = user_path(path)?;
    let ip = fs::namei(&path)?;
    if !ip.lock().is_dir() {
        return Err(ENOTDIR);
    }
    let t = task::current();
    let old = t.inner.lock().cwd.replace(ip);
    drop(old);
    Ok(0)
}

pub fn sys_unlink(path: usize) -> KResult<usize> {
    let path = user_path(path)?;
    fs::unlink(&path)?;
    Ok(0)
}
