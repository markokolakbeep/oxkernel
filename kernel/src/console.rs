//! Kernel printing and the console line discipline.

use alloc::vec::Vec;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicI32, Ordering};

use oxcore::abi::errno::EINTR;

use crate::KResult;
use crate::drivers::uart;
use crate::sync::SpinLock;
use crate::task;

static PRINT: SpinLock<()> = SpinLock::new("print", ());

/// Writes straight to the UART. The terminal is in raw mode, so \n needs a \r.
pub struct RawUart;

impl Write for RawUart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            put_byte(b);
        }
        Ok(())
    }
}

fn put_byte(b: u8) {
    if b == b'\n' {
        uart::putc(b'\r');
    }
    uart::putc(b);
}

pub fn _print(args: fmt::Arguments) {
    let _g = PRINT.lock();
    let _ = RawUart.write_fmt(args);
}

/// Output from user space (fd 1 and 2 on the console).
pub fn write_bytes(buf: &[u8]) {
    let _g = PRINT.lock();
    for &b in buf {
        put_byte(b);
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => { $crate::console::_print(format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! println {
    () => { $crate::print!("\n") };
    ($($arg:tt)*) => { $crate::console::_print(format_args!("{}\n", format_args!($($arg)*))) };
}

const INPUT_SIZE: usize = 256;

const CTRL_C: u8 = 0x03;
const CTRL_D: u8 = 0x04;
const CTRL_U: u8 = 0x15;
const BACKSPACE: u8 = 0x08;
const DELETE: u8 = 0x7f;

/// Line-buffered input, like xv6's console: `r` is the next byte to read,
/// `w` the end of committed lines, `e` the end of the line being edited.
struct Input {
    buf: [u8; INPUT_SIZE],
    r: usize,
    w: usize,
    e: usize,
}

static INPUT: SpinLock<Input> = SpinLock::new("console", Input { buf: [0; INPUT_SIZE], r: 0, w: 0, e: 0 });

/// Process group that ^C goes to; 0 for none.
pub static FOREGROUND: AtomicI32 = AtomicI32::new(0);

fn chan() -> usize {
    &INPUT as *const _ as usize
}

fn echo(bytes: &[u8]) {
    write_bytes(bytes);
}

/// Called from the UART interrupt for every received byte.
pub fn on_input(c: u8) {
    let mut kill = false;
    {
        let mut inp = INPUT.lock();
        match c {
            CTRL_C => {
                // Throw away the half-typed line and interrupt the foreground job.
                inp.e = inp.w;
                echo(b"^C\n");
                kill = true;
            }
            CTRL_U => {
                while inp.e != inp.w && inp.buf[(inp.e - 1) % INPUT_SIZE] != b'\n' {
                    inp.e -= 1;
                    echo(b"\x08 \x08");
                }
            }
            BACKSPACE | DELETE => {
                if inp.e != inp.w {
                    inp.e -= 1;
                    echo(b"\x08 \x08");
                }
            }
            0 => {}
            _ => {
                if inp.e - inp.r < INPUT_SIZE {
                    let c = if c == b'\r' { b'\n' } else { c };
                    if c != CTRL_D {
                        echo(&[c]);
                    }
                    let e = inp.e;
                    inp.buf[e % INPUT_SIZE] = c;
                    inp.e += 1;
                    if c == b'\n' || c == CTRL_D || inp.e - inp.r == INPUT_SIZE {
                        inp.w = inp.e;
                        task::wakeup(chan());
                    }
                }
            }
        }
    }
    if kill {
        task::kill_pgid(FOREGROUND.load(Ordering::Relaxed));
    }
}

/// Reads up to one line. ^D at the start of a line reads as end of file.
pub fn read(max: usize) -> KResult<Vec<u8>> {
    let me = task::current();
    let mut out = Vec::new();
    let mut inp = INPUT.lock();
    while out.len() < max {
        while inp.r == inp.w {
            if me.is_killed() {
                return Err(EINTR);
            }
            inp = task::sleep(chan(), inp);
        }
        let c = inp.buf[inp.r % INPUT_SIZE];
        inp.r += 1;
        if c == CTRL_D {
            if !out.is_empty() {
                // Leave ^D for the next read, which then returns 0.
                inp.r -= 1;
            }
            break;
        }
        out.push(c);
        if c == b'\n' {
            break;
        }
    }
    Ok(out)
}
