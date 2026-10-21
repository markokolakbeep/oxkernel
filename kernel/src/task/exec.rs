//! exec: replace the current process image with an ELF executable.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use oxcore::abi::{T_FILE, errno::*};
use oxcore::elf::{EHDR_SIZE, Header, PF_R, PF_W, PF_X, PHDR_SIZE, ProgramHeader};
use oxcore::{PAGE_SIZE, page_round_down, page_round_up};

use super::{current, set_name};
use crate::KResult;
use crate::arch::csr;
use crate::config::*;
use crate::fs::{self, ILocked};
use crate::mm::AddressSpace;
use crate::mm::paging::{PTE_R, PTE_W, PTE_X};
use crate::mm::phys_to_virt;
use crate::trap::TrapFrame;

fn read_exact(ip: &mut ILocked<'_>, off: u64, buf: &mut [u8]) -> KResult<()> {
    if ip.read(off, buf)? != buf.len() {
        return Err(ENOEXEC);
    }
    Ok(())
}

fn load_segment(aspace: &mut AddressSpace, ip: &mut ILocked<'_>, ph: &ProgramHeader) -> KResult<usize> {
    let start = ph.vaddr as usize;
    let end = start + ph.memsz as usize;
    if start < PAGE_SIZE || end > USER_HEAP_LIMIT {
        return Err(ENOEXEC);
    }
    let mut flags = 0;
    if ph.flags & PF_R != 0 {
        flags |= PTE_R;
    }
    if ph.flags & PF_W != 0 {
        flags |= PTE_W | PTE_R;
    }
    if ph.flags & PF_X != 0 {
        flags |= PTE_X;
    }
    let file_end = start + ph.filesz as usize;
    let mut va = page_round_down(start);
    while va < end {
        let pa = aspace.map_zeroed(va, flags)?;
        // The part of this page backed by the file.
        let lo = va.max(start);
        let hi = (va + PAGE_SIZE).min(file_end);
        if lo < hi {
            let page = unsafe { core::slice::from_raw_parts_mut(phys_to_virt(pa) as *mut u8, PAGE_SIZE) };
            let off = ph.offset + (lo - start) as u64;
            read_exact(ip, off, &mut page[lo - va..hi - va])?;
        }
        va += PAGE_SIZE;
    }
    Ok(end)
}

/// Lays out argv on the new stack: the strings at the top, then the
/// NULL-terminated pointer array. Returns (sp, argv).
fn push_args(aspace: &mut AddressSpace, args: &[String]) -> KResult<(usize, usize)> {
    let stack_bottom = USER_STACK_TOP - USER_STACK_PAGES * PAGE_SIZE;
    let mut va = stack_bottom;
    while va < USER_STACK_TOP {
        aspace.map_zeroed(va, PTE_R | PTE_W)?;
        va += PAGE_SIZE;
    }
    let mut sp = USER_STACK_TOP;
    let mut ptrs = Vec::with_capacity(args.len() + 1);
    for a in args {
        sp -= a.len() + 1;
        aspace.copy_out(sp, a.as_bytes())?;
        aspace.copy_out(sp + a.len(), &[0])?;
        ptrs.push(sp);
    }
    ptrs.push(0);
    sp &= !7;
    sp -= ptrs.len() * 8;
    sp &= !15;
    let argv = sp;
    for (i, p) in ptrs.iter().enumerate() {
        aspace.copy_out_val(argv + i * 8, p)?;
    }
    if sp < stack_bottom + PAGE_SIZE {
        return Err(E2BIG);
    }
    Ok((sp, argv))
}

/// Replaces the current image. On success the trap frame is set up to enter
/// the new program, and argc is returned (it lands in a0).
pub fn exec(tf: &mut TrapFrame, path: &str, args: &[String]) -> KResult<usize> {
    let ip = fs::namei(path)?;
    let mut g = ip.lock();
    if g.d().kind != T_FILE {
        return Err(EACCES);
    }
    let mut hdr = [0u8; EHDR_SIZE];
    read_exact(&mut g, 0, &mut hdr)?;
    let header = Header::parse(&hdr).map_err(|_| ENOEXEC)?;

    let mut aspace = AddressSpace::new()?;
    let mut image_end = 0;
    for i in 0..header.phnum {
        let mut raw = [0u8; PHDR_SIZE];
        read_exact(&mut g, header.phdr_offset(i), &mut raw)?;
        let ph = ProgramHeader::parse(&raw).map_err(|_| ENOEXEC)?;
        if ph.is_load() && ph.memsz > 0 {
            image_end = image_end.max(load_segment(&mut aspace, &mut g, &ph)?);
        }
    }
    drop(g);
    drop(ip);
    if header.entry as usize >= image_end {
        return Err(ENOEXEC);
    }
    aspace.heap_start = page_round_up(image_end);
    aspace.brk = aspace.heap_start;
    let (sp, argv) = push_args(&mut aspace, args)?;

    // Point of no return: swap in the new image.
    let task = current();
    let satp = aspace.satp();
    let old = {
        let mut inner = task.inner.lock();
        set_name(&mut inner, oxcore::path::basename(path));
        task.satp.store(satp, Ordering::Relaxed);
        crate::mm::paging::load_satp(satp);
        core::mem::replace(&mut inner.aspace, aspace)
    };
    drop(old);

    *tf = TrapFrame::zeroed();
    tf.sepc = header.entry as usize;
    tf.x[2] = sp;
    tf.x[11] = argv;
    // SIE stays clear: __return_to_user must not take interrupts once
    // sscratch points at the kernel stack.
    tf.sstatus = (csr::sstatus() & !(csr::SSTATUS_SPP | csr::SSTATUS_FS | csr::SSTATUS_SIE)) | csr::SSTATUS_SPIE;
    Ok(args.len())
}
