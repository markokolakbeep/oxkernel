//! mkfs: builds an oxfs image.
//!
//! ```text
//! mkfs OUTPUT --size-mb N [--file DEST=SRC]... [--dir DEST]...
//! ```
//!
//! The image always gets /, /bin, /dev, /tmp and the console device node
//! /dev/console.

use std::collections::HashMap;
use std::process::ExitCode;

use oxcore::abi::{CONSOLE_MAJOR, T_DEV, T_DIR, T_FILE};
use oxcore::fs::*;

struct Image {
    data: Vec<u8>,
    sb: SuperBlock,
    next_inode: u32,
    next_block: u32,
    /// Directory path -> inode number.
    dirs: HashMap<String, u32>,
}

impl Image {
    fn new(nblocks: u32, ninodes: u32) -> Self {
        let sb = SuperBlock::layout(nblocks, ninodes);
        let mut img = Self {
            data: vec![0; nblocks as usize * BLOCK_SIZE],
            sb,
            next_inode: ROOT_INO,
            next_block: sb.data_start,
            dirs: HashMap::new(),
        };
        sb.encode(img.block_mut(0));
        let root = img.alloc_inode(T_DIR, 0);
        assert_eq!(root, ROOT_INO);
        img.add_dirent(root, ".", root);
        img.add_dirent(root, "..", root);
        img.inode_mut(root, |i| i.nlink = 1);
        img.dirs.insert(String::new(), root);
        img
    }

    fn block_mut(&mut self, b: u32) -> &mut [u8] {
        let off = b as usize * BLOCK_SIZE;
        &mut self.data[off..off + BLOCK_SIZE]
    }

    fn inode(&self, inum: u32) -> DiskInode {
        let (b, off) = self.sb.inode_pos(inum);
        let start = b as usize * BLOCK_SIZE + off;
        DiskInode::decode(&self.data[start..start + INODE_SIZE])
    }

    fn inode_mut(&mut self, inum: u32, f: impl FnOnce(&mut DiskInode)) {
        let mut ino = self.inode(inum);
        f(&mut ino);
        let (b, off) = self.sb.inode_pos(inum);
        ino.encode(&mut self.block_mut(b)[off..off + INODE_SIZE]);
    }

    fn alloc_inode(&mut self, kind: u16, major: u16) -> u32 {
        let inum = self.next_inode;
        assert!(inum < self.sb.ninodes, "out of inodes");
        self.next_inode += 1;
        self.inode_mut(inum, |i| {
            *i = DiskInode { kind, major, ..Default::default() };
        });
        inum
    }

    fn alloc_block(&mut self) -> u32 {
        let b = self.next_block;
        assert!(b < self.sb.nblocks, "image full");
        self.next_block += 1;
        b
    }

    /// Appends `bytes` to inode `inum`.
    fn append(&mut self, inum: u32, mut bytes: &[u8]) {
        let mut ino = self.inode(inum);
        while !bytes.is_empty() {
            let bn = (ino.size / BLOCK_SIZE as u64) as usize;
            assert!(bn < MAX_FILE_BLOCKS, "file too big for oxfs");
            let block = if bn < NDIRECT {
                if ino.direct[bn] == 0 {
                    ino.direct[bn] = self.alloc_block();
                }
                ino.direct[bn]
            } else {
                if ino.indirect == 0 {
                    ino.indirect = self.alloc_block();
                }
                let ind = ino.indirect;
                let slot = (bn - NDIRECT) * 4;
                let cur = u32::from_le_bytes(self.block_mut(ind)[slot..slot + 4].try_into().unwrap());
                if cur == 0 {
                    let nb = self.alloc_block();
                    self.block_mut(ind)[slot..slot + 4].copy_from_slice(&nb.to_le_bytes());
                    nb
                } else {
                    cur
                }
            };
            let off = ino.size as usize % BLOCK_SIZE;
            let n = (BLOCK_SIZE - off).min(bytes.len());
            self.block_mut(block)[off..off + n].copy_from_slice(&bytes[..n]);
            ino.size += n as u64;
            bytes = &bytes[n..];
        }
        self.inode_mut(inum, |i| *i = ino);
    }

    fn add_dirent(&mut self, dir: u32, name: &str, inum: u32) {
        assert!(name.len() <= NAME_MAX, "name too long: {name}");
        let mut raw = [0u8; DIRENT_SIZE];
        Dirent::new(inum, name).encode(&mut raw);
        self.append(dir, &raw);
    }

    /// Creates directory `path` (and its parents), returning its inode.
    fn mkdir_p(&mut self, path: &str) -> u32 {
        let path = path.trim_matches('/');
        if let Some(&inum) = self.dirs.get(path) {
            return inum;
        }
        let (parent_path, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = self.mkdir_p(parent_path);
        let inum = self.alloc_inode(T_DIR, 0);
        self.add_dirent(inum, ".", inum);
        self.add_dirent(inum, "..", parent);
        self.add_dirent(parent, name, inum);
        self.inode_mut(inum, |i| i.nlink = 1);
        self.inode_mut(parent, |i| i.nlink += 1);
        self.dirs.insert(path.to_string(), inum);
        inum
    }

    fn add_node(&mut self, path: &str, kind: u16, major: u16) -> u32 {
        let path = path.trim_matches('/');
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = self.mkdir_p(dir);
        let inum = self.alloc_inode(kind, major);
        self.inode_mut(inum, |i| i.nlink = 1);
        self.add_dirent(parent, name, inum);
        inum
    }

    /// Writes the bitmaps for everything allocated so far.
    fn finish(mut self) -> Vec<u8> {
        let sb = self.sb;
        for inum in 0..self.next_inode {
            let (b, byte, bit) = bitmap_pos(sb.inode_bitmap_start, inum);
            self.block_mut(b)[byte] |= bit;
        }
        for blk in 0..self.next_block {
            let (b, byte, bit) = bitmap_pos(sb.block_bitmap_start, blk);
            self.block_mut(b)[byte] |= bit;
        }
        println!(
            "mkfs: {} blocks, {} inodes; used {} inodes, {} blocks ({} data)",
            sb.nblocks,
            sb.ninodes,
            self.next_inode - 1,
            self.next_block,
            self.next_block - sb.data_start
        );
        self.data
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let output = args.next().ok_or("usage: mkfs OUTPUT --size-mb N [--file DEST=SRC]... [--dir DEST]...")?;
    let mut size_mb = 32u32;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--size-mb" => size_mb = val()?.parse().map_err(|e| format!("--size-mb: {e}"))?,
            "--file" => {
                let v = val()?;
                let (dest, src) = v.split_once('=').ok_or("--file wants DEST=SRC")?;
                files.push((dest.to_string(), src.to_string()));
            }
            "--dir" => dirs.push(val()?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let nblocks = size_mb * (1 << 20) / BLOCK_SIZE as u32;
    let mut img = Image::new(nblocks, 1024);
    for d in ["bin", "dev", "tmp"].iter().map(|s| s.to_string()).chain(dirs) {
        img.mkdir_p(&d);
    }
    img.add_node("/dev/console", T_DEV, CONSOLE_MAJOR);
    for (dest, src) in files {
        let bytes = std::fs::read(&src).map_err(|e| format!("{src}: {e}"))?;
        let inum = img.add_node(&dest, T_FILE, 0);
        img.append(inum, &bytes);
    }
    std::fs::write(&output, img.finish()).map_err(|e| format!("{output}: {e}"))?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mkfs: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(img: &[u8], sb: &SuperBlock, dir: u32, name: &str) -> Option<u32> {
        let (b, off) = sb.inode_pos(dir);
        let start = b as usize * BLOCK_SIZE + off;
        let ino = DiskInode::decode(&img[start..start + INODE_SIZE]);
        let blk = ino.direct[0] as usize * BLOCK_SIZE;
        img[blk..blk + ino.size as usize]
            .chunks_exact(DIRENT_SIZE)
            .map(Dirent::decode)
            .find(|d| d.inum != 0 && d.name_is(name))
            .map(|d| d.inum)
    }

    #[test]
    fn builds_a_consistent_image() {
        let mut img = Image::new(2048, 128);
        img.mkdir_p("bin");
        img.add_node("/dev/console", T_DEV, CONSOLE_MAJOR);
        let big: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let f = img.add_node("/bin/big", T_FILE, 0);
        img.append(f, &big);
        let sb = img.sb;
        let data = img.finish();

        assert_eq!(SuperBlock::decode(&data[..BLOCK_SIZE]), sb);
        let bin = lookup(&data, &sb, ROOT_INO, "bin").unwrap();
        assert_eq!(lookup(&data, &sb, bin, ".."), Some(ROOT_INO));
        assert_eq!(lookup(&data, &sb, bin, "big"), Some(f));
        assert!(lookup(&data, &sb, ROOT_INO, "dev").is_some());

        // The big file spills into the indirect block and reads back intact.
        let (b, off) = sb.inode_pos(f);
        let ino = DiskInode::decode(&data[b as usize * BLOCK_SIZE + off..][..INODE_SIZE]);
        assert_eq!(ino.size, big.len() as u64);
        assert_ne!(ino.indirect, 0);
        let block_of = |bn: usize| -> usize {
            if bn < NDIRECT {
                ino.direct[bn] as usize
            } else {
                let ind = ino.indirect as usize * BLOCK_SIZE + (bn - NDIRECT) * 4;
                u32::from_le_bytes(data[ind..ind + 4].try_into().unwrap()) as usize
            }
        };
        let mut back = Vec::new();
        for bn in 0..big.len().div_ceil(BLOCK_SIZE) {
            back.extend_from_slice(&data[block_of(bn) * BLOCK_SIZE..][..BLOCK_SIZE]);
        }
        back.truncate(big.len());
        assert_eq!(back, big);

        // Every allocated block is marked in the bitmap; the next one is not.
        let used = |n: u32| {
            let (blk, byte, bit) = bitmap_pos(sb.block_bitmap_start, n);
            data[blk as usize * BLOCK_SIZE + byte] & bit != 0
        };
        assert!(used(0) && used(sb.data_start) && used(block_of(big.len() / BLOCK_SIZE) as u32));
        assert!(!used(sb.nblocks - 1));
    }
}
