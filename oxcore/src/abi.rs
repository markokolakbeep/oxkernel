//! The kernel/user ABI: syscall numbers, error codes, flags and the structs the
//! kernel copies out to user space.
//!
//! Calling convention: `a7` holds the syscall number, `a0`..`a5` the
//! arguments. The result comes back in `a0`; values in `-4095..0` are negated
//! error codes.

pub mod sys {
    pub const EXIT: usize = 1;
    pub const FORK: usize = 2;
    pub const WAIT: usize = 3;
    pub const EXEC: usize = 4;
    pub const GETPID: usize = 5;
    pub const SBRK: usize = 6;
    pub const SLEEP: usize = 7;
    pub const TIME_US: usize = 8;
    pub const YIELD: usize = 9;
    pub const KILL: usize = 10;
    pub const READ: usize = 11;
    pub const WRITE: usize = 12;
    pub const OPEN: usize = 13;
    pub const CLOSE: usize = 14;
    pub const DUP: usize = 15;
    pub const PIPE: usize = 16;
    pub const FSTAT: usize = 17;
    pub const MKDIR: usize = 18;
    pub const CHDIR: usize = 19;
    pub const UNLINK: usize = 20;
    pub const PROCINFO: usize = 21;
    pub const SETPGID: usize = 22;
    pub const SETFG: usize = 23;
    pub const SYSINFO: usize = 24;
    pub const SHUTDOWN: usize = 25;
}

pub mod errno {
    pub const EPERM: isize = 1;
    pub const ENOENT: isize = 2;
    pub const ESRCH: isize = 3;
    pub const EINTR: isize = 4;
    pub const EIO: isize = 5;
    pub const E2BIG: isize = 7;
    pub const ENOEXEC: isize = 8;
    pub const EBADF: isize = 9;
    pub const ECHILD: isize = 10;
    pub const EAGAIN: isize = 11;
    pub const ENOMEM: isize = 12;
    pub const EACCES: isize = 13;
    pub const EFAULT: isize = 14;
    pub const EEXIST: isize = 17;
    pub const ENOTDIR: isize = 20;
    pub const EISDIR: isize = 21;
    pub const EINVAL: isize = 22;
    pub const EMFILE: isize = 24;
    pub const EFBIG: isize = 27;
    pub const ENOSPC: isize = 28;
    pub const EPIPE: isize = 32;
    pub const ENAMETOOLONG: isize = 36;
    pub const ENOSYS: isize = 38;
    pub const ENOTEMPTY: isize = 39;

    pub fn name(e: isize) -> &'static str {
        match e {
            EPERM => "operation not permitted",
            ENOENT => "no such file or directory",
            ESRCH => "no such process",
            EINTR => "interrupted",
            EIO => "I/O error",
            E2BIG => "argument list too long",
            ENOEXEC => "exec format error",
            EBADF => "bad file descriptor",
            ECHILD => "no child processes",
            EAGAIN => "resource temporarily unavailable",
            ENOMEM => "out of memory",
            EACCES => "permission denied",
            EFAULT => "bad address",
            EEXIST => "file exists",
            ENOTDIR => "not a directory",
            EISDIR => "is a directory",
            EINVAL => "invalid argument",
            EMFILE => "too many open files",
            EFBIG => "file too large",
            ENOSPC => "no space left on device",
            EPIPE => "broken pipe",
            ENAMETOOLONG => "file name too long",
            ENOSYS => "function not implemented",
            ENOTEMPTY => "directory not empty",
            _ => "unknown error",
        }
    }
}

pub const O_RDONLY: usize = 0x000;
pub const O_WRONLY: usize = 0x001;
pub const O_RDWR: usize = 0x002;
pub const O_CREATE: usize = 0x200;
pub const O_TRUNC: usize = 0x400;

/// Inode kinds, shared with the on-disk format.
pub const T_FILE: u16 = 1;
pub const T_DIR: u16 = 2;
pub const T_DEV: u16 = 3;

/// Major device number of the console.
pub const CONSOLE_MAJOR: u16 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat {
    pub ino: u32,
    pub kind: u16,
    pub nlink: u16,
    pub size: u64,
}

pub const PROC_RUNNABLE: u32 = 0;
pub const PROC_RUNNING: u32 = 1;
pub const PROC_SLEEPING: u32 = 2;
pub const PROC_ZOMBIE: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    pub state: u32,
    /// Hart the process is running on, or last ran on; -1 if it never ran.
    pub hart: i32,
    pub _pad: u32,
    /// Resident user memory in bytes.
    pub mem: u64,
    pub name: [u8; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SysInfo {
    pub ncpus: u32,
    pub nprocs: u32,
    pub free_pages: u64,
    pub total_pages: u64,
    pub uptime_us: u64,
    pub timebase_hz: u64,
}

/// Decodes a raw syscall return value.
pub fn result(ret: isize) -> Result<usize, isize> {
    if (-4095..0).contains(&ret) { Err(-ret) } else { Ok(ret as usize) }
}
