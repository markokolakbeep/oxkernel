//! Flattened devicetree (DTB) parser.
//!
//! The parser walks the structure block and reports every property together
//! with the path of the node it belongs to. [`Fdt::machine_info`] builds on
//! that to pull out what the kernel needs at boot: RAM, harts, timer
//! frequency and the command line.

const MAGIC: u32 = 0xd00d_feed;
const BEGIN_NODE: u32 = 1;
const END_NODE: u32 = 2;
const PROP: u32 = 3;
const NOP: u32 = 4;
const END: u32 = 9;

const MAX_DEPTH: usize = 8;
pub const MAX_HARTS: usize = 8;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum FdtError {
    BadMagic,
    Truncated,
    BadToken(u32),
    TooDeep,
}

pub struct Fdt<'a> {
    data: &'a [u8],
    structs: &'a [u8],
    strings: &'a [u8],
}

/// The names of the nodes from the root down to the current one; the root
/// itself has the empty name and is not included.
pub struct NodePath<'a> {
    names: [&'a str; MAX_DEPTH],
    depth: usize,
}

impl<'a> NodePath<'a> {
    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn names(&self) -> &[&'a str] {
        &self.names[..self.depth]
    }

    pub fn last(&self) -> &'a str {
        if self.depth == 0 { "" } else { self.names[self.depth - 1] }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MachineInfo<'a> {
    pub mem_start: usize,
    pub mem_size: usize,
    pub harts: [usize; MAX_HARTS],
    pub nharts: usize,
    pub timebase_hz: u64,
    pub bootargs: &'a str,
}

fn be32(b: &[u8], off: usize) -> Result<u32, FdtError> {
    b.get(off..off + 4).map(|s| u32::from_be_bytes(s.try_into().unwrap())).ok_or(FdtError::Truncated)
}

fn cstr(b: &[u8]) -> &str {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    core::str::from_utf8(&b[..end]).unwrap_or("")
}

/// Reads a big-endian number made of `cells` 32-bit cells.
pub fn read_cells(b: &[u8], cells: usize) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..cells {
        v = (v << 32) | u32::from_be_bytes(b.get(i * 4..i * 4 + 4)?.try_into().unwrap()) as u64;
    }
    Some(v)
}

impl<'a> Fdt<'a> {
    pub fn from_bytes(data: &'a [u8]) -> Result<Self, FdtError> {
        if be32(data, 0)? != MAGIC {
            return Err(FdtError::BadMagic);
        }
        let total = be32(data, 4)? as usize;
        let off_struct = be32(data, 8)? as usize;
        let off_strings = be32(data, 12)? as usize;
        let size_strings = be32(data, 32)? as usize;
        let size_struct = be32(data, 36)? as usize;
        let data = data.get(..total).ok_or(FdtError::Truncated)?;
        let structs = data.get(off_struct..off_struct + size_struct).ok_or(FdtError::Truncated)?;
        let strings = data.get(off_strings..off_strings + size_strings).ok_or(FdtError::Truncated)?;
        Ok(Self { data, structs, strings })
    }

    /// # Safety
    /// `ptr` must point to a valid DTB that stays mapped for `'a`.
    pub unsafe fn from_ptr(ptr: *const u8) -> Result<Self, FdtError> {
        let header = unsafe { core::slice::from_raw_parts(ptr, 8) };
        if be32(header, 0)? != MAGIC {
            return Err(FdtError::BadMagic);
        }
        let total = be32(header, 4)? as usize;
        Self::from_bytes(unsafe { core::slice::from_raw_parts(ptr, total) })
    }

    pub fn total_size(&self) -> usize {
        self.data.len()
    }

    /// Calls `f(path, property name, value)` for every property, in order.
    pub fn for_each_prop(&self, mut f: impl FnMut(&NodePath<'a>, &'a str, &'a [u8])) -> Result<(), FdtError> {
        let s = self.structs;
        let mut path = NodePath { names: [""; MAX_DEPTH], depth: 0 };
        // -1 until the root node has been entered.
        let mut level: isize = -1;
        let mut off = 0;
        loop {
            let token = be32(s, off)?;
            off += 4;
            match token {
                BEGIN_NODE => {
                    let rest = s.get(off..).ok_or(FdtError::Truncated)?;
                    let name = cstr(rest);
                    off += (name.len() + 1).next_multiple_of(4);
                    level += 1;
                    if level > 0 {
                        if path.depth == MAX_DEPTH {
                            return Err(FdtError::TooDeep);
                        }
                        path.names[path.depth] = name;
                        path.depth += 1;
                    }
                }
                END_NODE => {
                    if level > 0 {
                        path.depth -= 1;
                    }
                    level -= 1;
                }
                PROP => {
                    let len = be32(s, off)? as usize;
                    let nameoff = be32(s, off + 4)? as usize;
                    off += 8;
                    let value = s.get(off..off + len).ok_or(FdtError::Truncated)?;
                    off += len.next_multiple_of(4);
                    let name = cstr(self.strings.get(nameoff..).ok_or(FdtError::Truncated)?);
                    f(&path, name, value);
                }
                NOP => {}
                END => return Ok(()),
                t => return Err(FdtError::BadToken(t)),
            }
        }
    }

    pub fn machine_info(&self) -> Result<MachineInfo<'a>, FdtError> {
        let mut info =
            MachineInfo { mem_start: 0, mem_size: 0, harts: [0; MAX_HARTS], nharts: 0, timebase_hz: 0, bootargs: "" };
        let (mut addr_cells, mut size_cells) = (2usize, 1usize);
        self.for_each_prop(|path, name, value| {
            let names = path.names();
            match (names, name) {
                ([], "#address-cells") => addr_cells = read_cells(value, 1).unwrap_or(2) as usize,
                ([], "#size-cells") => size_cells = read_cells(value, 1).unwrap_or(1) as usize,
                ([node], "reg") if node.starts_with("memory") && info.mem_size == 0 => {
                    info.mem_start = read_cells(value, addr_cells).unwrap_or(0) as usize;
                    info.mem_size =
                        value.get(addr_cells * 4..).and_then(|v| read_cells(v, size_cells)).unwrap_or(0) as usize;
                }
                (["cpus"], "timebase-frequency") => info.timebase_hz = read_cells(value, value.len() / 4).unwrap_or(0),
                (["cpus", cpu], "reg") if cpu.starts_with("cpu@") && info.nharts < MAX_HARTS => {
                    info.harts[info.nharts] = read_cells(value, 1).unwrap_or(0) as usize;
                    info.nharts += 1;
                }
                (["chosen"], "bootargs") => info.bootargs = cstr(value),
                _ => {}
            }
        })?;
        Ok(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny DTB writer, good enough to produce test blobs.
    struct Builder {
        structs: Vec<u8>,
        strings: Vec<u8>,
    }

    impl Builder {
        fn new() -> Self {
            Self { structs: Vec::new(), strings: Vec::new() }
        }
        fn word(&mut self, w: u32) {
            self.structs.extend_from_slice(&w.to_be_bytes());
        }
        fn pad(&mut self) {
            while !self.structs.len().is_multiple_of(4) {
                self.structs.push(0);
            }
        }
        fn begin(&mut self, name: &str) -> &mut Self {
            self.word(BEGIN_NODE);
            self.structs.extend_from_slice(name.as_bytes());
            self.structs.push(0);
            self.pad();
            self
        }
        fn end(&mut self) -> &mut Self {
            self.word(END_NODE);
            self
        }
        fn prop(&mut self, name: &str, value: &[u8]) -> &mut Self {
            let nameoff = self.strings.len() as u32;
            self.strings.extend_from_slice(name.as_bytes());
            self.strings.push(0);
            self.word(PROP);
            self.word(value.len() as u32);
            self.word(nameoff);
            self.structs.extend_from_slice(value);
            self.pad();
            self
        }
        fn prop_u32(&mut self, name: &str, v: &[u32]) -> &mut Self {
            let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_be_bytes()).collect();
            self.prop(name, &bytes)
        }
        fn finish(&mut self) -> Vec<u8> {
            self.word(END);
            let header = 40;
            let off_struct = header;
            let off_strings = off_struct + self.structs.len();
            let total = off_strings + self.strings.len();
            let mut out = Vec::new();
            for w in [
                MAGIC,
                total as u32,
                off_struct as u32,
                off_strings as u32,
                0,
                17,
                16,
                0,
                self.strings.len() as u32,
                self.structs.len() as u32,
            ] {
                out.extend_from_slice(&w.to_be_bytes());
            }
            out.extend_from_slice(&self.structs);
            out.extend_from_slice(&self.strings);
            out
        }
    }

    fn qemu_like() -> Vec<u8> {
        let mut b = Builder::new();
        b.begin("")
            .prop_u32("#address-cells", &[2])
            .prop_u32("#size-cells", &[2])
            .begin("chosen")
            .prop("bootargs", b"test smp\0")
            .end()
            .begin("memory@80000000")
            .prop("device_type", b"memory\0")
            .prop_u32("reg", &[0, 0x8000_0000, 0, 0x1000_0000])
            .end()
            .begin("cpus")
            .prop_u32("#address-cells", &[1])
            .prop_u32("timebase-frequency", &[10_000_000]);
        for hart in [0, 1, 3] {
            b.begin(&format!("cpu@{hart}")).prop_u32("reg", &[hart]).prop("status", b"okay\0").end();
        }
        b.begin("cpu-map").begin("cluster0").end().end().end().end();
        b.finish()
    }

    #[test]
    fn extracts_machine_info() {
        let blob = qemu_like();
        let fdt = Fdt::from_bytes(&blob).unwrap();
        assert_eq!(fdt.total_size(), blob.len());
        let info = fdt.machine_info().unwrap();
        assert_eq!(info.mem_start, 0x8000_0000);
        assert_eq!(info.mem_size, 0x1000_0000);
        assert_eq!(info.timebase_hz, 10_000_000);
        assert_eq!(&info.harts[..info.nharts], &[0, 1, 3]);
        assert_eq!(info.bootargs, "test smp");
    }

    #[test]
    fn reports_paths() {
        let blob = qemu_like();
        let fdt = Fdt::from_bytes(&blob).unwrap();
        let mut seen = Vec::new();
        fdt.for_each_prop(|path, name, _| seen.push(format!("/{}:{}", path.names().join("/"), name))).unwrap();
        assert!(seen.contains(&"/:#size-cells".to_string()));
        assert!(seen.contains(&"/cpus/cpu@3:status".to_string()));
        assert!(seen.contains(&"/memory@80000000:reg".to_string()));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Fdt::from_bytes(&[0u8; 64]).err(), Some(FdtError::BadMagic));
        let mut blob = qemu_like();
        blob.truncate(50);
        assert_eq!(Fdt::from_bytes(&blob).err(), Some(FdtError::Truncated));
    }
}
