//! The file system: buffer cache, oxfs, path lookup, open files and pipes.

mod bcache;
mod file;
mod inode;
mod pipe;

use oxcore::abi::{T_DIR, T_FILE, errno::*};
use oxcore::fs::{MAGIC, NAME_MAX, ROOT_INO, SuperBlock};
use oxcore::path::split_first;

pub use file::File;
pub use inode::{ILocked, InodeRef, iget};
pub use pipe::Pipe;

use crate::KResult;
use crate::sync::Once;
use crate::task;

static SUPERBLOCK: Once<SuperBlock> = Once::new();

pub fn superblock() -> &'static SuperBlock {
    SUPERBLOCK.wait()
}

/// Sets up the buffer cache. Runs at boot.
pub fn init_cache() {
    bcache::init();
}

/// Reads the superblock. Needs task context, since it sleeps on the disk.
pub fn mount() {
    let buf = bcache::read(0);
    let sb = SuperBlock::decode(&buf[..]);
    drop(buf);
    assert!(sb.magic == MAGIC, "oxfs: bad superblock magic {:#x}", sb.magic);
    println!(
        "oxfs: {} blocks ({} MiB), {} inodes, data from block {}",
        sb.nblocks,
        (sb.nblocks as usize * oxcore::fs::BLOCK_SIZE) >> 20,
        sb.ninodes,
        sb.data_start
    );
    SUPERBLOCK.set(sb);
}

fn start_dir(path: &str) -> InodeRef {
    if path.starts_with('/') {
        return iget(ROOT_INO);
    }
    let me = task::current();
    let cwd = me.inner.lock().cwd.clone();
    cwd.unwrap_or_else(|| iget(ROOT_INO))
}

/// Walks `path`. With `parent`, stops at the last component's directory and
/// returns it along with that component.
fn namex(path: &str, parent: bool) -> KResult<(InodeRef, &str)> {
    let mut ip = start_dir(path);
    let mut rest = path;
    while let Some((name, next)) = split_first(rest) {
        if name.len() > NAME_MAX {
            return Err(ENAMETOOLONG);
        }
        let found = {
            let mut g = ip.lock();
            if !g.is_dir() {
                return Err(ENOTDIR);
            }
            if parent && next.is_empty() {
                drop(g);
                return Ok((ip, name));
            }
            g.dir_lookup(name)?
        };
        let (inum, _) = found.ok_or(ENOENT)?;
        ip = iget(inum);
        rest = next;
    }
    if parent {
        return Err(EINVAL);
    }
    Ok((ip, ""))
}

pub fn namei(path: &str) -> KResult<InodeRef> {
    namex(path, false).map(|(ip, _)| ip)
}

pub fn nameiparent(path: &str) -> KResult<(InodeRef, &str)> {
    namex(path, true)
}

/// Creates `path` as a new inode of `kind`. Opening an existing file with
/// O_CREATE is fine; anything else that exists is EEXIST.
pub fn create(path: &str, kind: u16, major: u16) -> KResult<InodeRef> {
    let (dp, name) = nameiparent(path)?;
    let mut dg = dp.lock();
    if let Some((inum, _)) = dg.dir_lookup(name)? {
        drop(dg);
        let ip = iget(inum);
        let existing = ip.lock().d().kind;
        if kind == T_FILE && existing != T_DIR {
            return Ok(ip);
        }
        return Err(EEXIST);
    }
    let ip = inode::ialloc(kind, major)?;
    {
        let mut ig = ip.lock();
        ig.d_mut().nlink = 1;
        ig.update();
        if kind == T_DIR {
            ig.dir_link(".", ip.inum())?;
            ig.dir_link("..", dp.inum())?;
        }
    }
    if kind == T_DIR {
        // The new directory's ".." links back to its parent.
        dg.d_mut().nlink += 1;
        dg.update();
    }
    if let Err(e) = dg.dir_link(name, ip.inum()) {
        // Roll back: with no links the inode is freed when `ip` drops.
        let mut ig = ip.lock();
        ig.d_mut().nlink = 0;
        ig.update();
        return Err(e);
    }
    Ok(ip)
}

pub fn unlink(path: &str) -> KResult<()> {
    let (dp, name) = nameiparent(path)?;
    if name == "." || name == ".." {
        return Err(EINVAL);
    }
    let mut dg = dp.lock();
    let (inum, off) = dg.dir_lookup(name)?.ok_or(ENOENT)?;
    let ip = iget(inum);
    let mut ig = ip.lock();
    assert!(ig.d().nlink > 0, "unlink: inode {inum} has no links");
    if ig.is_dir() && !ig.dir_is_empty()? {
        return Err(ENOTEMPTY);
    }
    dg.dir_clear(off)?;
    if ig.is_dir() {
        dg.d_mut().nlink -= 1;
        dg.update();
    }
    drop(dg);
    ig.d_mut().nlink -= 1;
    ig.update();
    Ok(())
}
