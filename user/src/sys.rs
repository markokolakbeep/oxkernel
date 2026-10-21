//! Thin wrappers around the system calls. Errors are positive errno values.

use core::arch::asm;

use alloc::ffi::CString;
use alloc::vec::Vec;

use oxcore::abi::{self, ProcInfo, Stat, SysInfo, sys};

pub type SysResult<T> = Result<T, isize>;

#[inline(always)]
fn syscall(n: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret: isize;
    unsafe {
        asm!("ecall", inlateout("a0") a0 as isize => ret, in("a1") a1, in("a2") a2, in("a7") n, options(nostack));
    }
    ret
}

fn check(ret: isize) -> SysResult<usize> {
    abi::result(ret)
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_default()
}

pub fn exit(code: i32) -> ! {
    syscall(sys::EXIT, code as usize, 0, 0);
    unreachable!()
}

pub fn fork() -> SysResult<i32> {
    check(syscall(sys::FORK, 0, 0, 0)).map(|p| p as i32)
}

/// Waits for any child; returns (pid, exit code).
pub fn wait() -> SysResult<(i32, i32)> {
    let mut status = 0i32;
    let pid = check(syscall(sys::WAIT, &mut status as *mut i32 as usize, 0, 0))?;
    Ok((pid as i32, status))
}

/// Replaces this process with `path`. Only returns on failure.
pub fn exec(path: &str, args: &[&str]) -> isize {
    let path = cstr(path);
    let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
    let mut argv: Vec<*const u8> = owned.iter().map(|a| a.as_ptr().cast()).collect();
    argv.push(core::ptr::null());
    match check(syscall(sys::EXEC, path.as_ptr() as usize, argv.as_ptr() as usize, 0)) {
        Ok(_) => unreachable!(),
        Err(e) => e,
    }
}

pub fn getpid() -> i32 {
    syscall(sys::GETPID, 0, 0, 0) as i32
}

/// Moves the program break by `incr` bytes; returns the old break.
pub fn sbrk(incr: isize) -> SysResult<usize> {
    check(syscall(sys::SBRK, incr as usize, 0, 0))
}

pub fn sleep_ms(ms: u64) -> SysResult<()> {
    check(syscall(sys::SLEEP, ms as usize, 0, 0)).map(|_| ())
}

pub fn time_us() -> u64 {
    syscall(sys::TIME_US, 0, 0, 0) as u64
}

pub fn yield_now() {
    syscall(sys::YIELD, 0, 0, 0);
}

pub fn kill(pid: i32) -> SysResult<()> {
    check(syscall(sys::KILL, pid as usize, 0, 0)).map(|_| ())
}

pub fn read(fd: usize, buf: &mut [u8]) -> SysResult<usize> {
    check(syscall(sys::READ, fd, buf.as_mut_ptr() as usize, buf.len()))
}

pub fn write(fd: usize, buf: &[u8]) -> SysResult<usize> {
    check(syscall(sys::WRITE, fd, buf.as_ptr() as usize, buf.len()))
}

/// Like `write`, but takes a raw pointer; for tests that pass bad addresses.
pub fn write_raw(fd: usize, ptr: usize, len: usize) -> SysResult<usize> {
    check(syscall(sys::WRITE, fd, ptr, len))
}

pub fn open(path: &str, flags: usize) -> SysResult<usize> {
    let path = cstr(path);
    check(syscall(sys::OPEN, path.as_ptr() as usize, flags, 0))
}

pub fn open_raw(path: usize, flags: usize) -> SysResult<usize> {
    check(syscall(sys::OPEN, path, flags, 0))
}

pub fn close(fd: usize) -> SysResult<()> {
    check(syscall(sys::CLOSE, fd, 0, 0)).map(|_| ())
}

pub fn dup(fd: usize) -> SysResult<usize> {
    check(syscall(sys::DUP, fd, 0, 0))
}

/// Returns (read end, write end).
pub fn pipe() -> SysResult<(usize, usize)> {
    let mut fds = [0i32; 2];
    check(syscall(sys::PIPE, fds.as_mut_ptr() as usize, 0, 0))?;
    Ok((fds[0] as usize, fds[1] as usize))
}

pub fn fstat(fd: usize) -> SysResult<Stat> {
    let mut st = Stat::default();
    check(syscall(sys::FSTAT, fd, &mut st as *mut Stat as usize, 0))?;
    Ok(st)
}

pub fn stat(path: &str) -> SysResult<Stat> {
    let fd = open(path, abi::O_RDONLY)?;
    let st = fstat(fd);
    let _ = close(fd);
    st
}

pub fn mkdir(path: &str) -> SysResult<()> {
    let path = cstr(path);
    check(syscall(sys::MKDIR, path.as_ptr() as usize, 0, 0)).map(|_| ())
}

pub fn chdir(path: &str) -> SysResult<()> {
    let path = cstr(path);
    check(syscall(sys::CHDIR, path.as_ptr() as usize, 0, 0)).map(|_| ())
}

pub fn unlink(path: &str) -> SysResult<()> {
    let path = cstr(path);
    check(syscall(sys::UNLINK, path.as_ptr() as usize, 0, 0)).map(|_| ())
}

pub fn procinfo() -> SysResult<Vec<ProcInfo>> {
    let mut buf = alloc::vec![ProcInfo::default(); 64];
    let n = check(syscall(sys::PROCINFO, buf.as_mut_ptr() as usize, buf.len(), 0))?;
    buf.truncate(n);
    Ok(buf)
}

/// Starts a new process group led by the caller.
pub fn setpgid() -> i32 {
    syscall(sys::SETPGID, 0, 0, 0) as i32
}

/// Sets the console's foreground process group (0 for none).
pub fn setfg(pgid: i32) {
    syscall(sys::SETFG, pgid as usize, 0, 0);
}

pub fn sysinfo() -> SysInfo {
    let mut info = SysInfo::default();
    syscall(sys::SYSINFO, &mut info as *mut SysInfo as usize, 0, 0);
    info
}

pub fn shutdown(code: u32) -> SysResult<()> {
    check(syscall(sys::SHUTDOWN, code as usize, 0, 0)).map(|_| ())
}

/// Reads the `time` CSR directly (the kernel enables it for U-mode).
#[inline(always)]
pub fn rdtime() -> u64 {
    let t: u64;
    unsafe { asm!("rdtime {}", out(reg) t, options(nomem, nostack)) };
    t
}
