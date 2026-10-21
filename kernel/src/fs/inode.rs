//! oxfs inodes, block allocation and directories.

use alloc::sync::Arc;
use alloc::vec::Vec;

use oxcore::abi::{Stat, T_DIR, errno::*};
use oxcore::fs::*;

use super::bcache;
use super::superblock;
use crate::KResult;
use crate::sync::{SleepLock, SleepLockGuard, SpinLock};

/// In-memory copy of an inode.
pub struct Inode {
    pub inum: u32,
    data: SleepLock<InodeData>,
}

pub struct InodeData {
    valid: bool,
    d: DiskInode,
}

/// Cache of in-memory inodes. An entry whose only reference is the cache's
/// own can be evicted.
static ICACHE: SpinLock<Vec<Arc<Inode>>> = SpinLock::new("icache", Vec::new());
const NINODE: usize = 128;

/// A counted reference to a cached inode; dropping the last one frees the
/// inode on disk if it has no links left.
pub struct InodeRef(Arc<Inode>);

impl Clone for InodeRef {
    fn clone(&self) -> Self {
        InodeRef(self.0.clone())
    }
}

pub fn iget(inum: u32) -> InodeRef {
    let mut cache = ICACHE.lock();
    if let Some(ip) = cache.iter().find(|ip| ip.inum == inum) {
        return InodeRef(ip.clone());
    }
    if cache.len() >= NINODE {
        cache.retain(|ip| Arc::strong_count(ip) > 1);
        assert!(cache.len() < NINODE, "icache: every inode in use");
    }
    let ip =
        Arc::new(Inode { inum, data: SleepLock::new("inode", InodeData { valid: false, d: DiskInode::default() }) });
    cache.push(ip.clone());
    InodeRef(ip)
}

impl Drop for InodeRef {
    fn drop(&mut self) {
        // Last reference besides the cache? Then nobody can find this inode
        // again if it has no links, so it is safe to free.
        let last = {
            let _cache = ICACHE.lock();
            Arc::strong_count(&self.0) == 2
        };
        if !last {
            return;
        }
        let Some(data) = self.0.data.try_lock() else { return };
        let mut ip = ILocked { inum: self.0.inum, g: data };
        if ip.g.valid && ip.g.d.nlink == 0 {
            ip.truncate();
            ip.g.d.kind = 0;
            ip.update();
            ip.g.valid = false;
            free_inum(ip.inum);
        }
    }
}

impl InodeRef {
    pub fn inum(&self) -> u32 {
        self.0.inum
    }

    /// Locks the inode, reading it from disk the first time.
    pub fn lock(&self) -> ILocked<'_> {
        let mut g = self.0.data.lock();
        if !g.valid {
            let sb = superblock();
            let (block, off) = sb.inode_pos(self.0.inum);
            let buf = bcache::read(block);
            g.d = DiskInode::decode(&buf[off..off + INODE_SIZE]);
            g.valid = true;
            assert!(g.d.kind != 0, "inode {} has no type", self.0.inum);
        }
        ILocked { inum: self.0.inum, g }
    }
}

/// A locked inode; all reads and writes go through here.
pub struct ILocked<'a> {
    pub inum: u32,
    g: SleepLockGuard<'a, InodeData>,
}

fn bitmap_alloc(start: u32, first: u32, limit: u32) -> Option<u32> {
    let mut n = first;
    while n < limit {
        let (block, _, _) = bitmap_pos(start, n);
        let mut buf = bcache::read(block);
        // Scan the rest of this bitmap block.
        while n < limit && bitmap_pos(start, n).0 == block {
            let (_, byte, bit) = bitmap_pos(start, n);
            if buf[byte] & bit == 0 {
                buf[byte] |= bit;
                buf.write();
                return Some(n);
            }
            n += 1;
        }
    }
    None
}

fn bitmap_free(start: u32, n: u32) {
    let (block, byte, bit) = bitmap_pos(start, n);
    let mut buf = bcache::read(block);
    assert!(buf[byte] & bit != 0, "oxfs: freeing free item {n}");
    buf[byte] &= !bit;
    buf.write();
}

/// Allocates a zeroed data block.
fn balloc() -> KResult<u32> {
    let sb = superblock();
    let b = bitmap_alloc(sb.block_bitmap_start, sb.data_start, sb.nblocks).ok_or(ENOSPC)?;
    let mut buf = bcache::read(b);
    buf.fill(0);
    buf.write();
    Ok(b)
}

fn bfree(b: u32) {
    let sb = superblock();
    assert!(b >= sb.data_start && b < sb.nblocks, "oxfs: freeing metadata block {b}");
    bitmap_free(sb.block_bitmap_start, b);
}

fn free_inum(inum: u32) {
    bitmap_free(superblock().inode_bitmap_start, inum);
}

/// Allocates an inode of `kind` with no links.
pub fn ialloc(kind: u16, major: u16) -> KResult<InodeRef> {
    let sb = superblock();
    let inum = bitmap_alloc(sb.inode_bitmap_start, 1, sb.ninodes).ok_or(ENOSPC)?;
    let (block, off) = sb.inode_pos(inum);
    let mut buf = bcache::read(block);
    DiskInode { kind, major, ..Default::default() }.encode(&mut buf[off..off + INODE_SIZE]);
    buf.write();
    drop(buf);
    Ok(iget(inum))
}

impl ILocked<'_> {
    pub fn d(&self) -> &DiskInode {
        &self.g.d
    }

    pub fn d_mut(&mut self) -> &mut DiskInode {
        &mut self.g.d
    }

    pub fn is_dir(&self) -> bool {
        self.g.d.kind == T_DIR
    }

    /// Writes the in-memory inode back to disk.
    pub fn update(&mut self) {
        let (block, off) = superblock().inode_pos(self.inum);
        let mut buf = bcache::read(block);
        self.g.d.encode(&mut buf[off..off + INODE_SIZE]);
        buf.write();
    }

    pub fn stat(&self) -> Stat {
        Stat { ino: self.inum, kind: self.g.d.kind, nlink: self.g.d.nlink, size: self.g.d.size }
    }

    /// Disk block holding file block `bn`, allocating it if `alloc`.
    /// Returns 0 for a hole when not allocating.
    fn bmap(&mut self, bn: usize, alloc: bool) -> KResult<u32> {
        if bn < NDIRECT {
            if self.g.d.direct[bn] == 0 && alloc {
                self.g.d.direct[bn] = balloc()?;
                self.update();
            }
            return Ok(self.g.d.direct[bn]);
        }
        let bn = bn - NDIRECT;
        if bn >= NINDIRECT {
            return Err(EFBIG);
        }
        if self.g.d.indirect == 0 {
            if !alloc {
                return Ok(0);
            }
            self.g.d.indirect = balloc()?;
            self.update();
        }
        let mut ind = bcache::read(self.g.d.indirect);
        let slot = bn * 4..bn * 4 + 4;
        let mut b = u32::from_le_bytes(ind[slot.clone()].try_into().unwrap());
        if b == 0 && alloc {
            b = balloc()?;
            ind[slot].copy_from_slice(&b.to_le_bytes());
            ind.write();
        }
        Ok(b)
    }

    pub fn read(&mut self, off: u64, dst: &mut [u8]) -> KResult<usize> {
        let size = self.g.d.size;
        if off >= size {
            return Ok(0);
        }
        let n = dst.len().min((size - off) as usize);
        let mut done = 0;
        while done < n {
            let pos = off as usize + done;
            let in_block = pos % BLOCK_SIZE;
            let chunk = (BLOCK_SIZE - in_block).min(n - done);
            let b = self.bmap(pos / BLOCK_SIZE, false)?;
            if b == 0 {
                dst[done..done + chunk].fill(0);
            } else {
                let buf = bcache::read(b);
                dst[done..done + chunk].copy_from_slice(&buf[in_block..in_block + chunk]);
            }
            done += chunk;
        }
        Ok(n)
    }

    pub fn write(&mut self, off: u64, src: &[u8]) -> KResult<usize> {
        if off > self.g.d.size {
            return Err(EINVAL);
        }
        if off + src.len() as u64 > MAX_FILE_SIZE {
            return Err(EFBIG);
        }
        let mut done = 0;
        while done < src.len() {
            let pos = off as usize + done;
            let in_block = pos % BLOCK_SIZE;
            let chunk = (BLOCK_SIZE - in_block).min(src.len() - done);
            let b = match self.bmap(pos / BLOCK_SIZE, true) {
                Ok(b) => b,
                Err(e) if done == 0 => return Err(e),
                Err(_) => break,
            };
            let mut buf = bcache::read(b);
            buf[in_block..in_block + chunk].copy_from_slice(&src[done..done + chunk]);
            buf.write();
            done += chunk;
        }
        if off + done as u64 > self.g.d.size {
            self.g.d.size = off + done as u64;
            self.update();
        }
        Ok(done)
    }

    /// Frees all of the file's blocks.
    pub fn truncate(&mut self) {
        for i in 0..NDIRECT {
            if self.g.d.direct[i] != 0 {
                bfree(self.g.d.direct[i]);
                self.g.d.direct[i] = 0;
            }
        }
        if self.g.d.indirect != 0 {
            let ind = bcache::read(self.g.d.indirect);
            let blocks: Vec<u32> =
                ind.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).filter(|&b| b != 0).collect();
            drop(ind);
            for b in blocks {
                bfree(b);
            }
            bfree(self.g.d.indirect);
            self.g.d.indirect = 0;
        }
        self.g.d.size = 0;
        self.update();
    }

    fn dirent_at(&mut self, off: u64) -> KResult<Dirent> {
        let mut raw = [0u8; DIRENT_SIZE];
        if self.read(off, &mut raw)? != DIRENT_SIZE {
            return Err(EIO);
        }
        Ok(Dirent::decode(&raw))
    }

    /// Finds `name` in this directory: (inode number, byte offset of entry).
    pub fn dir_lookup(&mut self, name: &str) -> KResult<Option<(u32, u64)>> {
        assert!(self.is_dir());
        let mut off = 0;
        while off < self.g.d.size {
            let de = self.dirent_at(off)?;
            if de.inum != 0 && de.name_is(name) {
                return Ok(Some((de.inum, off)));
            }
            off += DIRENT_SIZE as u64;
        }
        Ok(None)
    }

    /// Adds the entry `name -> inum`, reusing a free slot if there is one.
    pub fn dir_link(&mut self, name: &str, inum: u32) -> KResult<()> {
        if name.len() > NAME_MAX {
            return Err(ENAMETOOLONG);
        }
        if self.dir_lookup(name)?.is_some() {
            return Err(EEXIST);
        }
        let mut off = 0;
        while off < self.g.d.size {
            if self.dirent_at(off)?.inum == 0 {
                break;
            }
            off += DIRENT_SIZE as u64;
        }
        let mut raw = [0u8; DIRENT_SIZE];
        Dirent::new(inum, name).encode(&mut raw);
        if self.write(off, &raw)? != DIRENT_SIZE {
            return Err(ENOSPC);
        }
        Ok(())
    }

    pub fn dir_clear(&mut self, off: u64) -> KResult<()> {
        self.write(off, &[0u8; DIRENT_SIZE])?;
        Ok(())
    }

    /// True if the directory holds nothing but "." and "..".
    pub fn dir_is_empty(&mut self) -> KResult<bool> {
        let mut off = 2 * DIRENT_SIZE as u64;
        while off < self.g.d.size {
            if self.dirent_at(off)?.inum != 0 {
                return Ok(false);
            }
            off += DIRENT_SIZE as u64;
        }
        Ok(true)
    }
}
