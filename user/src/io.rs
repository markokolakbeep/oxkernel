//! Formatted output and small I/O helpers.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write};

use crate::sys::{SysResult, read, write};

pub const STDIN: usize = 0;
pub const STDOUT: usize = 1;
pub const STDERR: usize = 2;

/// Writes all of `buf`, retrying short writes.
pub fn write_all(fd: usize, mut buf: &[u8]) -> SysResult<()> {
    while !buf.is_empty() {
        let n = write(fd, buf)?;
        if n == 0 {
            return Err(crate::errno::EIO);
        }
        buf = &buf[n..];
    }
    Ok(())
}

/// Buffers formatted output so each `print!` is one write syscall.
pub struct Fd(pub usize);

impl Fd {
    pub fn print(&self, args: fmt::Arguments) {
        let mut s = String::new();
        let _ = s.write_fmt(args);
        let _ = write_all(self.0, s.as_bytes());
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::io::Fd($crate::io::STDOUT).print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::io::Fd($crate::io::STDOUT).print(format_args!("{}\n", format_args!($($arg)*))) };
}

#[macro_export]
macro_rules! eprintln {
    ($($arg:tt)*) => { $crate::io::Fd($crate::io::STDERR).print(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Reads one line (without the newline). `None` at end of input.
pub fn read_line(fd: usize) -> Option<String> {
    let mut line = Vec::new();
    let mut c = [0u8; 1];
    loop {
        match read(fd, &mut c) {
            Ok(1) if c[0] == b'\n' => break,
            Ok(1) => line.push(c[0]),
            _ if line.is_empty() => return None,
            _ => break,
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

pub fn read_to_end(fd: usize) -> SysResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = read(fd, &mut buf)?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}
