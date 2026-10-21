//! Open files, shared between file descriptors by `dup` and `fork`.

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use oxcore::abi::{Stat, T_DEV, errno::*};

use super::{InodeRef, Pipe};
use crate::KResult;
use crate::console;

pub enum FileKind {
    Inode { ip: InodeRef, off: AtomicU64 },
    PipeRead(Arc<Pipe>),
    PipeWrite(Arc<Pipe>),
    Console,
}

pub struct File {
    pub kind: FileKind,
    pub readable: bool,
    pub writable: bool,
}

impl File {
    pub fn inode(ip: InodeRef, readable: bool, writable: bool) -> Self {
        Self { kind: FileKind::Inode { ip, off: AtomicU64::new(0) }, readable, writable }
    }

    pub fn console(readable: bool, writable: bool) -> Self {
        Self { kind: FileKind::Console, readable, writable }
    }

    pub fn pipe() -> (Self, Self) {
        let p = Arc::new(Pipe::new());
        (
            Self { kind: FileKind::PipeRead(p.clone()), readable: true, writable: false },
            Self { kind: FileKind::PipeWrite(p), readable: false, writable: true },
        )
    }

    /// Whether short reads mean end of data (true for regular files).
    pub fn is_inode(&self) -> bool {
        matches!(self.kind, FileKind::Inode { .. })
    }

    pub fn read(&self, dst: &mut [u8]) -> KResult<usize> {
        if !self.readable {
            return Err(EBADF);
        }
        match &self.kind {
            FileKind::Inode { ip, off } => {
                let mut g = ip.lock();
                let n = g.read(off.load(Ordering::Relaxed), dst)?;
                off.fetch_add(n as u64, Ordering::Relaxed);
                Ok(n)
            }
            FileKind::PipeRead(p) => p.read(dst),
            FileKind::Console => {
                let line = console::read(dst.len())?;
                dst[..line.len()].copy_from_slice(&line);
                Ok(line.len())
            }
            FileKind::PipeWrite(_) => Err(EBADF),
        }
    }

    pub fn write(&self, src: &[u8]) -> KResult<usize> {
        if !self.writable {
            return Err(EBADF);
        }
        match &self.kind {
            FileKind::Inode { ip, off } => {
                let mut g = ip.lock();
                let n = g.write(off.load(Ordering::Relaxed), src)?;
                off.fetch_add(n as u64, Ordering::Relaxed);
                Ok(n)
            }
            FileKind::PipeWrite(p) => p.write(src),
            FileKind::Console => {
                console::write_bytes(src);
                Ok(src.len())
            }
            FileKind::PipeRead(_) => Err(EBADF),
        }
    }

    pub fn stat(&self) -> Stat {
        match &self.kind {
            FileKind::Inode { ip, .. } => ip.lock().stat(),
            FileKind::Console => Stat { kind: T_DEV, nlink: 1, ..Default::default() },
            FileKind::PipeRead(_) | FileKind::PipeWrite(_) => Stat::default(),
        }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        match &self.kind {
            FileKind::PipeRead(p) => p.close(false),
            FileKind::PipeWrite(p) => p.close(true),
            _ => {}
        }
    }
}
