//! Platform-Level Interrupt Controller.

use crate::config::{PLIC_BASE, UART0_IRQ, VIRTIO_SLOTS, VIRTIO0_IRQ};
use crate::mm::phys_to_virt;

fn reg(off: usize) -> *mut u32 {
    phys_to_virt(PLIC_BASE + off) as *mut u32
}

/// S-mode context of the current hart.
fn context() -> usize {
    2 * crate::arch::hartid() + 1
}

fn device_irqs() -> impl Iterator<Item = u32> {
    core::iter::once(UART0_IRQ).chain(VIRTIO0_IRQ..VIRTIO0_IRQ + VIRTIO_SLOTS as u32)
}

pub fn init() {
    for irq in device_irqs() {
        unsafe { reg(irq as usize * 4).write_volatile(1) };
    }
}

pub fn init_hart() {
    let mask = device_irqs().fold(0u32, |m, irq| m | (1 << irq));
    unsafe {
        reg(0x2000 + context() * 0x80).write_volatile(mask);
        reg(0x20_0000 + context() * 0x1000).write_volatile(0);
    }
}

fn claim() -> u32 {
    unsafe { reg(0x20_0004 + context() * 0x1000).read_volatile() }
}

fn complete(irq: u32) {
    unsafe { reg(0x20_0004 + context() * 0x1000).write_volatile(irq) };
}

pub fn handle() {
    loop {
        let irq = claim();
        if irq == 0 {
            break;
        }
        match irq {
            UART0_IRQ => super::uart::handle_irq(),
            i if (VIRTIO0_IRQ..VIRTIO0_IRQ + VIRTIO_SLOTS as u32).contains(&i) => super::virtio_blk::handle_irq(),
            other => println!("plic: unexpected irq {other}"),
        }
        complete(irq);
    }
}
