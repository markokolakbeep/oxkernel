//! Buffer cache: NBUF block buffers with LRU replacement.
//!
//! The metadata (which block a buffer holds, its reference count, whether its
//! data is valid) is under one spinlock. Each buffer's contents sit behind a
//! sleep lock, so only one task touches a block at a time. Writes go straight
//! to disk (`write` below); there is no write-back and no journal.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::{Deref, DerefMut};

use oxcore::fs::BLOCK_SIZE;

use crate::drivers::virtio_blk;
use crate::sync::{Once, SleepLock, SleepLockGuard, SpinLock};

const NBUF: usize = 64;
const NO_BLOCK: u32 = u32::MAX;

struct Meta {
    blockno: u32,
    refcnt: u32,
    valid: bool,
    last_use: u64,
}

struct Cache {
    meta: SpinLock<(Vec<Meta>, u64)>,
    bufs: Vec<SleepLock<Box<[u8; BLOCK_SIZE]>>>,
}

static CACHE: Once<Cache> = Once::new();

pub fn init() {
    let meta = (0..NBUF).map(|_| Meta { blockno: NO_BLOCK, refcnt: 0, valid: false, last_use: 0 }).collect();
    let bufs = (0..NBUF).map(|_| SleepLock::new("buf", Box::new([0u8; BLOCK_SIZE]))).collect();
    CACHE.set(Cache { meta: SpinLock::new("bcache", (meta, 0)), bufs });
}

/// A locked buffer. Dereferences to the block's bytes.
pub struct Buf {
    idx: usize,
    blockno: u32,
    data: Option<SleepLockGuard<'static, Box<[u8; BLOCK_SIZE]>>>,
}

fn get(blockno: u32) -> usize {
    let cache = CACHE.wait();
    let mut m = cache.meta.lock();
    if let Some(i) = m.0.iter().position(|b| b.blockno == blockno) {
        m.0[i].refcnt += 1;
        return i;
    }
    let victim =
        m.0.iter()
            .enumerate()
            .filter(|(_, b)| b.refcnt == 0)
            .min_by_key(|(_, b)| b.last_use)
            .map(|(i, _)| i)
            .expect("bcache: all buffers in use");
    let b = &mut m.0[victim];
    b.blockno = blockno;
    b.refcnt = 1;
    b.valid = false;
    victim
}

/// Returns the locked buffer for `blockno`, reading it from disk if needed.
pub fn read(blockno: u32) -> Buf {
    let cache = CACHE.wait();
    let idx = get(blockno);
    let mut data = cache.bufs[idx].lock();
    let valid = cache.meta.lock().0[idx].valid;
    if !valid {
        virtio_blk::rw(blockno, &mut data, false);
        cache.meta.lock().0[idx].valid = true;
    }
    Buf { idx, blockno, data: Some(data) }
}

impl Buf {
    /// Writes the buffer's contents to disk.
    pub fn write(&mut self) {
        virtio_blk::rw(self.blockno, self.data.as_mut().unwrap(), true);
    }
}

impl Deref for Buf {
    type Target = [u8; BLOCK_SIZE];
    fn deref(&self) -> &Self::Target {
        self.data.as_ref().unwrap()
    }
}

impl DerefMut for Buf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data.as_mut().unwrap()
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        // Unlock the contents before dropping the reference.
        self.data = None;
        let cache = CACHE.wait();
        let mut m = cache.meta.lock();
        m.1 += 1;
        let stamp = m.1;
        let b = &mut m.0[self.idx];
        b.refcnt -= 1;
        b.last_use = stamp;
    }
}
