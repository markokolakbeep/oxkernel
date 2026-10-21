//! virtio-blk over virtio-mmio (modern, version 2) with one split virtqueue.
//!
//! A request is a three-descriptor chain: header (device reads), data buffer,
//! status byte (device writes). Requests complete through the used ring and
//! the interrupt handler wakes the task that issued them, so several harts
//! can have I/O in flight at once.

use core::ptr::{addr_of_mut, read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use oxcore::PAGE_SIZE;
use oxcore::fs::{BLOCK_SIZE, SECTORS_PER_BLOCK};

use crate::config::{VIRTIO_SLOTS, VIRTIO_STRIDE, VIRTIO0};
use crate::mm::{frame, phys_to_virt, virt_to_phys};
use crate::sync::SpinLock;
use crate::task;

// MMIO register offsets.
const MAGIC: usize = 0x000;
const VERSION: usize = 0x004;
const DEVICE_ID: usize = 0x008;
const DEVICE_FEATURES: usize = 0x010;
const DEVICE_FEATURES_SEL: usize = 0x014;
const DRIVER_FEATURES: usize = 0x020;
const DRIVER_FEATURES_SEL: usize = 0x024;
const QUEUE_SEL: usize = 0x030;
const QUEUE_NUM_MAX: usize = 0x034;
const QUEUE_NUM: usize = 0x038;
const QUEUE_READY: usize = 0x044;
const QUEUE_NOTIFY: usize = 0x050;
const INTERRUPT_STATUS: usize = 0x060;
const INTERRUPT_ACK: usize = 0x064;
const STATUS: usize = 0x070;
const QUEUE_DESC_LOW: usize = 0x080;
const QUEUE_DESC_HIGH: usize = 0x084;
const QUEUE_DRIVER_LOW: usize = 0x090;
const QUEUE_DRIVER_HIGH: usize = 0x094;
const QUEUE_DEVICE_LOW: usize = 0x0a0;
const QUEUE_DEVICE_HIGH: usize = 0x0a4;
const CONFIG: usize = 0x100;

const STATUS_ACKNOWLEDGE: u32 = 1;
const STATUS_DRIVER: u32 = 2;
const STATUS_DRIVER_OK: u32 = 4;
const STATUS_FEATURES_OK: u32 = 8;

/// Feature bit 32, i.e. bit 0 of the high feature word.
const VIRTIO_F_VERSION_1: u32 = 1;
const DEVICE_BLK: u32 = 2;

const QSIZE: usize = 16;

const DESC_F_NEXT: u16 = 1;
const DESC_F_WRITE: u16 = 2;

const BLK_T_IN: u32 = 0;
const BLK_T_OUT: u32 = 1;
const BLK_T_FLUSH: u32 = 4;

/// The device has a write cache that FLUSH empties. Without negotiating
/// this, QEMU makes its cache write-through and syncs on every write.
const VIRTIO_BLK_F_FLUSH: u32 = 1 << 9;

#[repr(C)]
#[derive(Clone, Copy)]
struct Desc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct Avail {
    flags: u16,
    idx: u16,
    ring: [u16; QSIZE],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct Used {
    flags: u16,
    idx: u16,
    ring: [UsedElem; QSIZE],
    avail_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ReqHeader {
    kind: u32,
    reserved: u32,
    sector: u64,
}

/// Everything the device reads or writes, in one physically contiguous page.
#[repr(C, align(4096))]
struct Ring {
    desc: [Desc; QSIZE],
    avail: Avail,
    _pad: [u8; 2],
    used: Used,
    headers: [ReqHeader; QSIZE],
    status: [u8; QSIZE],
}

const _: () = assert!(size_of::<Ring>() <= PAGE_SIZE);

struct Blk {
    base: usize,
    ring: *mut Ring,
    free: [bool; QSIZE],
    done: [bool; QSIZE],
    used_idx: u16,
    capacity_blocks: u64,
    can_flush: bool,
}

unsafe impl Send for Blk {}

static BLK: SpinLock<Option<Blk>> = SpinLock::new("virtio_blk", None);

fn rd(base: usize, off: usize) -> u32 {
    unsafe { read_volatile((base + off) as *const u32) }
}

fn wr(base: usize, off: usize, v: u32) {
    unsafe { write_volatile((base + off) as *mut u32, v) }
}

fn split(pa: usize) -> (u32, u32) {
    (pa as u32, (pa >> 32) as u32)
}

/// Finds the first virtio-blk device and brings it up.
pub fn init() -> bool {
    for slot in 0..VIRTIO_SLOTS {
        let base = phys_to_virt(VIRTIO0 + slot * VIRTIO_STRIDE);
        if rd(base, MAGIC) != 0x7472_6976 || rd(base, DEVICE_ID) != DEVICE_BLK {
            continue;
        }
        if rd(base, VERSION) != 2 {
            println!("virtio: slot {slot} is a legacy device; run QEMU with -global virtio-mmio.force-legacy=false");
            continue;
        }
        let blk = setup(base);
        println!("virtio-blk: slot {slot}, {} MiB", (blk.capacity_blocks * BLOCK_SIZE as u64) >> 20);
        *BLK.lock() = Some(blk);
        return true;
    }
    println!("virtio-blk: no disk found");
    false
}

fn setup(base: usize) -> Blk {
    let mut status = 0;
    wr(base, STATUS, status);
    status |= STATUS_ACKNOWLEDGE;
    wr(base, STATUS, status);
    status |= STATUS_DRIVER;
    wr(base, STATUS, status);

    // VERSION_1 is required; FLUSH is taken when offered.
    wr(base, DEVICE_FEATURES_SEL, 1);
    let high = rd(base, DEVICE_FEATURES);
    assert!(high & VIRTIO_F_VERSION_1 != 0, "virtio-blk: device lacks VERSION_1");
    wr(base, DEVICE_FEATURES_SEL, 0);
    let low = rd(base, DEVICE_FEATURES) & VIRTIO_BLK_F_FLUSH;
    wr(base, DRIVER_FEATURES_SEL, 0);
    wr(base, DRIVER_FEATURES, low);
    wr(base, DRIVER_FEATURES_SEL, 1);
    wr(base, DRIVER_FEATURES, VIRTIO_F_VERSION_1);
    status |= STATUS_FEATURES_OK;
    wr(base, STATUS, status);
    assert!(rd(base, STATUS) & STATUS_FEATURES_OK != 0, "virtio-blk: features rejected");

    wr(base, QUEUE_SEL, 0);
    assert!(rd(base, QUEUE_READY) == 0, "virtio-blk: queue already live");
    assert!(rd(base, QUEUE_NUM_MAX) as usize >= QSIZE, "virtio-blk: queue too small");
    wr(base, QUEUE_NUM, QSIZE as u32);

    let pa = frame::alloc_frame().expect("virtio-blk: no memory for queue");
    let ring = phys_to_virt(pa) as *mut Ring;
    let (desc, avail, used) = unsafe {
        (
            virt_to_phys(addr_of_mut!((*ring).desc) as usize),
            virt_to_phys(addr_of_mut!((*ring).avail) as usize),
            virt_to_phys(addr_of_mut!((*ring).used) as usize),
        )
    };
    for (lo_reg, hi_reg, addr) in [
        (QUEUE_DESC_LOW, QUEUE_DESC_HIGH, desc),
        (QUEUE_DRIVER_LOW, QUEUE_DRIVER_HIGH, avail),
        (QUEUE_DEVICE_LOW, QUEUE_DEVICE_HIGH, used),
    ] {
        let (lo, hi) = split(addr);
        wr(base, lo_reg, lo);
        wr(base, hi_reg, hi);
    }
    wr(base, QUEUE_READY, 1);
    status |= STATUS_DRIVER_OK;
    wr(base, STATUS, status);

    let capacity_sectors = rd(base, CONFIG) as u64 | (rd(base, CONFIG + 4) as u64) << 32;
    Blk {
        base,
        ring,
        free: [true; QSIZE],
        done: [false; QSIZE],
        used_idx: 0,
        capacity_blocks: capacity_sectors / SECTORS_PER_BLOCK,
        can_flush: low != 0,
    }
}

#[cfg_attr(not(feature = "ktest"), allow(dead_code))]
pub fn capacity_blocks() -> u64 {
    BLK.lock().as_ref().map_or(0, |b| b.capacity_blocks)
}

fn alloc3(b: &mut Blk) -> Option<[usize; 3]> {
    let mut out = [0; 3];
    let mut n = 0;
    for (i, f) in b.free.iter().enumerate() {
        if *f {
            out[n] = i;
            n += 1;
            if n == 3 {
                break;
            }
        }
    }
    if n < 3 {
        return None;
    }
    for i in out {
        b.free[i] = false;
    }
    Some(out)
}

fn free_chan() -> usize {
    &BLK as *const _ as usize + 1
}

fn done_chan(head: usize) -> usize {
    &BLK as *const _ as usize + 0x100 + head
}

/// Reads or writes one 4 KiB block, sleeping until the device finishes.
pub fn rw(blockno: u32, buf: &mut [u8; BLOCK_SIZE], write: bool) {
    submit(if write { BLK_T_OUT } else { BLK_T_IN }, blockno, Some(buf));
}

/// Asks the device to write back its cache.
pub fn flush() {
    if BLK.lock().as_ref().is_some_and(|b| b.can_flush) {
        submit(BLK_T_FLUSH, 0, None);
    }
}

fn submit(kind: u32, blockno: u32, buf: Option<&mut [u8; BLOCK_SIZE]>) {
    let mut g = BLK.lock();
    let ids = loop {
        let b = g.as_mut().expect("virtio-blk: no disk");
        if let Some(ids) = alloc3(b) {
            break ids;
        }
        g = task::sleep(free_chan(), g);
    };
    let b = g.as_mut().unwrap();
    let head = ids[0];
    assert!((blockno as u64) < b.capacity_blocks, "virtio-blk: block {blockno} past end of disk");
    unsafe {
        let r = &mut *b.ring;
        r.headers[head] = ReqHeader { kind, reserved: 0, sector: blockno as u64 * SECTORS_PER_BLOCK };
        r.status[head] = 0xff;
        let status_desc = Desc {
            addr: virt_to_phys(&r.status[head] as *const _ as usize) as u64,
            len: 1,
            flags: DESC_F_WRITE,
            next: 0,
        };
        r.desc[ids[0]] = Desc {
            addr: virt_to_phys(&r.headers[head] as *const _ as usize) as u64,
            len: size_of::<ReqHeader>() as u32,
            flags: DESC_F_NEXT,
            next: ids[1] as u16,
        };
        match buf {
            Some(buf) => {
                r.desc[ids[1]] = Desc {
                    addr: virt_to_phys(buf.as_ptr() as usize) as u64,
                    len: BLOCK_SIZE as u32,
                    flags: DESC_F_NEXT | if kind == BLK_T_IN { DESC_F_WRITE } else { 0 },
                    next: ids[2] as u16,
                };
                r.desc[ids[2]] = status_desc;
            }
            // A flush carries no data: header, then status.
            None => r.desc[ids[1]] = status_desc,
        }
        b.done[head] = false;
        let idx = read_volatile(&r.avail.idx);
        write_volatile(&mut r.avail.ring[idx as usize % QSIZE], head as u16);
        fence(Ordering::SeqCst);
        write_volatile(&mut r.avail.idx, idx.wrapping_add(1));
        fence(Ordering::SeqCst);
    }
    wr(b.base, QUEUE_NOTIFY, 0);

    while !g.as_ref().unwrap().done[head] {
        g = task::sleep(done_chan(head), g);
    }
    let b = g.as_mut().unwrap();
    let status = unsafe { read_volatile(&(*b.ring).status[head]) };
    assert!(status == 0, "virtio-blk: request {kind} for block {blockno} failed with status {status}");
    for i in ids {
        b.free[i] = true;
    }
    drop(g);
    task::wakeup(free_chan());
}

pub fn handle_irq() {
    let mut g = BLK.lock();
    let Some(b) = g.as_mut() else { return };
    let isr = rd(b.base, INTERRUPT_STATUS);
    wr(b.base, INTERRUPT_ACK, isr & 0x3);
    fence(Ordering::SeqCst);
    loop {
        let used_idx = unsafe { read_volatile(&(*b.ring).used.idx) };
        if b.used_idx == used_idx {
            break;
        }
        fence(Ordering::SeqCst);
        let id = unsafe { read_volatile(&(*b.ring).used.ring[b.used_idx as usize % QSIZE]).id } as usize;
        b.done[id] = true;
        task::wakeup(done_chan(id));
        b.used_idx = b.used_idx.wrapping_add(1);
    }
}
