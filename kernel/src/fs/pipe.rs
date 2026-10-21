//! Pipes: a 4 KiB ring buffer with sleeping readers and writers.

use oxcore::abi::errno::*;
use oxcore::ringbuf::RingBuf;

use crate::KResult;
use crate::sync::SpinLock;
use crate::task;

const PIPE_SIZE: usize = 4096;

struct Inner {
    buf: RingBuf<PIPE_SIZE>,
    read_open: bool,
    write_open: bool,
}

pub struct Pipe {
    inner: SpinLock<Inner>,
}

impl Pipe {
    pub fn new() -> Self {
        Self { inner: SpinLock::new("pipe", Inner { buf: RingBuf::new(), read_open: true, write_open: true }) }
    }

    fn read_chan(&self) -> usize {
        self as *const _ as usize
    }

    fn write_chan(&self) -> usize {
        self as *const _ as usize + 1
    }

    /// Blocks until data is available or every writer is gone.
    pub fn read(&self, dst: &mut [u8]) -> KResult<usize> {
        let me = task::current();
        let mut p = self.inner.lock();
        while p.buf.is_empty() && p.write_open {
            if me.is_killed() {
                return Err(EINTR);
            }
            p = task::sleep(self.read_chan(), p);
        }
        let n = p.buf.pop_slice(dst);
        drop(p);
        task::wakeup(self.write_chan());
        Ok(n)
    }

    /// Writes all of `src` unless the read end closes first.
    pub fn write(&self, src: &[u8]) -> KResult<usize> {
        let me = task::current();
        let mut done = 0;
        let mut p = self.inner.lock();
        while done < src.len() {
            if !p.read_open {
                return if done > 0 { Ok(done) } else { Err(EPIPE) };
            }
            if me.is_killed() {
                return Err(EINTR);
            }
            if p.buf.is_full() {
                task::wakeup(self.read_chan());
                p = task::sleep(self.write_chan(), p);
                continue;
            }
            done += p.buf.push_slice(&src[done..]);
        }
        drop(p);
        task::wakeup(self.read_chan());
        Ok(done)
    }

    pub fn close(&self, write_end: bool) {
        let mut p = self.inner.lock();
        if write_end {
            p.write_open = false;
        } else {
            p.read_open = false;
        }
        drop(p);
        task::wakeup(self.read_chan());
        task::wakeup(self.write_chan());
    }
}
