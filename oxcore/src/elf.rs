//! Minimal ELF64 parser: just enough to load static RISC-V executables.

pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;

pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

const EM_RISCV: u16 = 243;
const ET_EXEC: u16 = 2;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ElfError {
    Truncated,
    BadMagic,
    Not64BitLittleEndian,
    NotRiscv,
    NotExecutable,
    BadProgramHeaders,
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub entry: u64,
    pub phoff: u64,
    pub phnum: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramHeader {
    pub kind: u32,
    pub flags: u32,
    pub offset: u64,
    pub vaddr: u64,
    pub filesz: u64,
    pub memsz: u64,
}

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

impl Header {
    pub fn parse(b: &[u8]) -> Result<Self, ElfError> {
        if b.len() < EHDR_SIZE {
            return Err(ElfError::Truncated);
        }
        if b[0..4] != *b"\x7fELF" {
            return Err(ElfError::BadMagic);
        }
        if b[4] != 2 || b[5] != 1 {
            return Err(ElfError::Not64BitLittleEndian);
        }
        if u16_at(b, 18) != EM_RISCV {
            return Err(ElfError::NotRiscv);
        }
        if u16_at(b, 16) != ET_EXEC {
            return Err(ElfError::NotExecutable);
        }
        if u16_at(b, 54) as usize != PHDR_SIZE {
            return Err(ElfError::BadProgramHeaders);
        }
        Ok(Self { entry: u64_at(b, 24), phoff: u64_at(b, 32), phnum: u16_at(b, 56) })
    }

    /// File offset of program header `i`.
    pub fn phdr_offset(&self, i: u16) -> u64 {
        self.phoff + i as u64 * PHDR_SIZE as u64
    }
}

impl ProgramHeader {
    pub fn parse(b: &[u8]) -> Result<Self, ElfError> {
        if b.len() < PHDR_SIZE {
            return Err(ElfError::Truncated);
        }
        let ph = Self {
            kind: u32_at(b, 0),
            flags: u32_at(b, 4),
            offset: u64_at(b, 8),
            vaddr: u64_at(b, 16),
            filesz: u64_at(b, 32),
            memsz: u64_at(b, 40),
        };
        if ph.kind == PT_LOAD && (ph.filesz > ph.memsz || ph.vaddr.checked_add(ph.memsz).is_none()) {
            return Err(ElfError::BadProgramHeaders);
        }
        Ok(ph)
    }

    pub fn is_load(&self) -> bool {
        self.kind == PT_LOAD
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ehdr(machine: u16, kind: u16) -> Vec<u8> {
        let mut b = vec![0u8; EHDR_SIZE];
        b[0..4].copy_from_slice(b"\x7fELF");
        b[4] = 2;
        b[5] = 1;
        b[16..18].copy_from_slice(&kind.to_le_bytes());
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        b[24..32].copy_from_slice(&0x10_0000u64.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());
        b[54..56].copy_from_slice(&(PHDR_SIZE as u16).to_le_bytes());
        b[56..58].copy_from_slice(&2u16.to_le_bytes());
        b
    }

    #[test]
    fn parses_header() {
        let h = Header::parse(&ehdr(EM_RISCV, ET_EXEC)).unwrap();
        assert_eq!(h.entry, 0x10_0000);
        assert_eq!(h.phnum, 2);
        assert_eq!(h.phdr_offset(1), 64 + 56);
    }

    #[test]
    fn rejects_bad_headers() {
        assert_eq!(Header::parse(&[0; 10]).unwrap_err(), ElfError::Truncated);
        let mut b = ehdr(EM_RISCV, ET_EXEC);
        b[1] = b'X';
        assert_eq!(Header::parse(&b).unwrap_err(), ElfError::BadMagic);
        assert_eq!(Header::parse(&ehdr(62, ET_EXEC)).unwrap_err(), ElfError::NotRiscv);
        assert_eq!(Header::parse(&ehdr(EM_RISCV, 3)).unwrap_err(), ElfError::NotExecutable);
    }

    #[test]
    fn parses_program_header() {
        let mut b = vec![0u8; PHDR_SIZE];
        b[0..4].copy_from_slice(&PT_LOAD.to_le_bytes());
        b[4..8].copy_from_slice(&(PF_R | PF_X).to_le_bytes());
        b[8..16].copy_from_slice(&0x1000u64.to_le_bytes());
        b[16..24].copy_from_slice(&0x10000u64.to_le_bytes());
        b[32..40].copy_from_slice(&0x200u64.to_le_bytes());
        b[40..48].copy_from_slice(&0x300u64.to_le_bytes());
        let ph = ProgramHeader::parse(&b).unwrap();
        assert!(ph.is_load());
        assert_eq!((ph.offset, ph.vaddr, ph.filesz, ph.memsz), (0x1000, 0x10000, 0x200, 0x300));
        // filesz > memsz is malformed.
        b[32..40].copy_from_slice(&0x400u64.to_le_bytes());
        assert_eq!(ProgramHeader::parse(&b).unwrap_err(), ElfError::BadProgramHeaders);
    }
}
