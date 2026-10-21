//! On-disk layout of oxfs.
//!
//! ```text
//! block 0      superblock
//! block 1..    inode bitmap   (1 bit per inode)
//!       ..     block bitmap   (1 bit per block, metadata blocks marked used)
//!       ..     inode table    (64-byte inodes, 64 per block)
//!       ..     data blocks
//! ```
//!
//! Inode 0 is reserved so that 0 can mean "no inode" in directory entries.
//! Inode 1 is the root directory. Integers are little endian.

pub const BLOCK_SIZE: usize = 4096;
pub const SECTORS_PER_BLOCK: u64 = (BLOCK_SIZE / 512) as u64;
pub const MAGIC: u32 = 0x6f78_6673; // "oxfs"
pub const ROOT_INO: u32 = 1;

pub const NDIRECT: usize = 11;
pub const NINDIRECT: usize = BLOCK_SIZE / 4;
pub const MAX_FILE_BLOCKS: usize = NDIRECT + NINDIRECT;
pub const MAX_FILE_SIZE: u64 = (MAX_FILE_BLOCKS * BLOCK_SIZE) as u64;

pub const INODE_SIZE: usize = 64;
pub const INODES_PER_BLOCK: usize = BLOCK_SIZE / INODE_SIZE;
pub const BITS_PER_BLOCK: usize = BLOCK_SIZE * 8;

pub const DIRENT_SIZE: usize = 32;
pub const NAME_MAX: usize = DIRENT_SIZE - 4;
pub const DIRENTS_PER_BLOCK: usize = BLOCK_SIZE / DIRENT_SIZE;

fn get_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn get_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn get_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SuperBlock {
    pub magic: u32,
    pub nblocks: u32,
    pub ninodes: u32,
    pub inode_bitmap_start: u32,
    pub block_bitmap_start: u32,
    pub inode_start: u32,
    pub data_start: u32,
}

impl SuperBlock {
    /// Computes the layout for a disk of `nblocks` blocks holding `ninodes`
    /// inodes.
    pub fn layout(nblocks: u32, ninodes: u32) -> Self {
        let inode_bitmap_blocks = (ninodes as usize).div_ceil(BITS_PER_BLOCK) as u32;
        let block_bitmap_blocks = (nblocks as usize).div_ceil(BITS_PER_BLOCK) as u32;
        let inode_blocks = (ninodes as usize).div_ceil(INODES_PER_BLOCK) as u32;
        let inode_bitmap_start = 1;
        let block_bitmap_start = inode_bitmap_start + inode_bitmap_blocks;
        let inode_start = block_bitmap_start + block_bitmap_blocks;
        let data_start = inode_start + inode_blocks;
        assert!(data_start < nblocks, "disk too small for its metadata");
        Self { magic: MAGIC, nblocks, ninodes, inode_bitmap_start, block_bitmap_start, inode_start, data_start }
    }

    pub fn decode(b: &[u8]) -> Self {
        Self {
            magic: get_u32(b, 0),
            nblocks: get_u32(b, 4),
            ninodes: get_u32(b, 8),
            inode_bitmap_start: get_u32(b, 12),
            block_bitmap_start: get_u32(b, 16),
            inode_start: get_u32(b, 20),
            data_start: get_u32(b, 24),
        }
    }

    pub fn encode(&self, b: &mut [u8]) {
        let fields = [
            self.magic,
            self.nblocks,
            self.ninodes,
            self.inode_bitmap_start,
            self.block_bitmap_start,
            self.inode_start,
            self.data_start,
        ];
        for (i, f) in fields.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&f.to_le_bytes());
        }
    }

    /// Block holding inode `inum`, and the inode's byte offset inside it.
    pub fn inode_pos(&self, inum: u32) -> (u32, usize) {
        let inum = inum as usize;
        (self.inode_start + (inum / INODES_PER_BLOCK) as u32, (inum % INODES_PER_BLOCK) * INODE_SIZE)
    }
}

/// Bitmap block, byte and bit for item `n` in a bitmap starting at `start`.
pub fn bitmap_pos(start: u32, n: u32) -> (u32, usize, u8) {
    let n = n as usize;
    (start + (n / BITS_PER_BLOCK) as u32, (n % BITS_PER_BLOCK) / 8, 1 << (n % 8))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskInode {
    /// One of `abi::T_FILE`, `T_DIR`, `T_DEV`; 0 means free.
    pub kind: u16,
    pub nlink: u16,
    pub major: u16,
    pub size: u64,
    pub direct: [u32; NDIRECT],
    pub indirect: u32,
}

impl DiskInode {
    pub fn decode(b: &[u8]) -> Self {
        let mut direct = [0; NDIRECT];
        for (i, d) in direct.iter_mut().enumerate() {
            *d = get_u32(b, 16 + i * 4);
        }
        Self {
            kind: get_u16(b, 0),
            nlink: get_u16(b, 2),
            major: get_u16(b, 4),
            size: get_u64(b, 8),
            direct,
            indirect: get_u32(b, 16 + NDIRECT * 4),
        }
    }

    pub fn encode(&self, b: &mut [u8]) {
        b[..INODE_SIZE].fill(0);
        b[0..2].copy_from_slice(&self.kind.to_le_bytes());
        b[2..4].copy_from_slice(&self.nlink.to_le_bytes());
        b[4..6].copy_from_slice(&self.major.to_le_bytes());
        b[8..16].copy_from_slice(&self.size.to_le_bytes());
        for (i, d) in self.direct.iter().enumerate() {
            b[16 + i * 4..20 + i * 4].copy_from_slice(&d.to_le_bytes());
        }
        b[16 + NDIRECT * 4..20 + NDIRECT * 4].copy_from_slice(&self.indirect.to_le_bytes());
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dirent {
    pub inum: u32,
    pub name: [u8; NAME_MAX],
}

impl Dirent {
    pub fn new(inum: u32, name: &str) -> Self {
        let mut d = Self { inum, name: [0; NAME_MAX] };
        let n = name.len().min(NAME_MAX);
        d.name[..n].copy_from_slice(&name.as_bytes()[..n]);
        d
    }

    pub fn decode(b: &[u8]) -> Self {
        let mut name = [0; NAME_MAX];
        name.copy_from_slice(&b[4..DIRENT_SIZE]);
        Self { inum: get_u32(b, 0), name }
    }

    pub fn encode(&self, b: &mut [u8]) {
        b[0..4].copy_from_slice(&self.inum.to_le_bytes());
        b[4..DIRENT_SIZE].copy_from_slice(&self.name);
    }

    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(NAME_MAX);
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }

    pub fn name_is(&self, name: &str) -> bool {
        self.name() == name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_is_contiguous() {
        let sb = SuperBlock::layout(8192, 1024);
        assert_eq!(sb.inode_bitmap_start, 1);
        assert_eq!(sb.block_bitmap_start, 2);
        assert_eq!(sb.inode_start, 3);
        assert_eq!(sb.data_start, 3 + 1024 / 64);
        let big = SuperBlock::layout(100_000, 70_000);
        assert_eq!(big.block_bitmap_start, 1 + 3);
        assert_eq!(big.inode_start, big.block_bitmap_start + 4);
    }

    #[test]
    fn roundtrips() {
        let sb = SuperBlock::layout(4096, 256);
        let mut b = [0u8; BLOCK_SIZE];
        sb.encode(&mut b);
        assert_eq!(SuperBlock::decode(&b), sb);

        let mut ino = DiskInode { kind: 2, nlink: 3, major: 0, size: 123_456, indirect: 99, ..Default::default() };
        ino.direct[0] = 7;
        ino.direct[NDIRECT - 1] = 0xdead;
        let mut ib = [0u8; INODE_SIZE];
        ino.encode(&mut ib);
        assert_eq!(DiskInode::decode(&ib), ino);

        let d = Dirent::new(42, "hello.txt");
        let mut db = [0u8; DIRENT_SIZE];
        d.encode(&mut db);
        let back = Dirent::decode(&db);
        assert_eq!(back.inum, 42);
        assert_eq!(back.name(), "hello.txt");
        assert!(back.name_is("hello.txt"));
    }

    #[test]
    fn positions() {
        let sb = SuperBlock::layout(8192, 1024);
        assert_eq!(sb.inode_pos(0), (sb.inode_start, 0));
        assert_eq!(sb.inode_pos(65), (sb.inode_start + 1, 64));
        assert_eq!(bitmap_pos(5, 0), (5, 0, 1));
        assert_eq!(bitmap_pos(5, 9), (5, 1, 2));
        assert_eq!(bitmap_pos(5, BITS_PER_BLOCK as u32 + 3), (6, 0, 8));
    }

    #[test]
    fn long_names_are_truncated() {
        let d = Dirent::new(1, "a-very-long-file-name-that-does-not-fit");
        assert_eq!(d.name().len(), NAME_MAX);
    }
}
