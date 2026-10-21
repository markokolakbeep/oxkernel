//! NS16550A UART, the QEMU `virt` console.

use crate::config::UART0;
use crate::mm::phys_to_virt;

const RBR: usize = 0; // receive buffer (read)
const THR: usize = 0; // transmit holding (write)
const IER: usize = 1; // interrupt enable
const FCR: usize = 2; // FIFO control
const LCR: usize = 3; // line control
const LSR: usize = 5; // line status

const LSR_DATA_READY: u8 = 1 << 0;
const LSR_THR_EMPTY: u8 = 1 << 5;

fn reg(r: usize) -> *mut u8 {
    phys_to_virt(UART0 + r) as *mut u8
}

fn read(r: usize) -> u8 {
    unsafe { reg(r).read_volatile() }
}

fn write(r: usize, v: u8) {
    unsafe { reg(r).write_volatile(v) }
}

pub fn init() {
    write(IER, 0);
    // 38400 baud (divisor 3); irrelevant under QEMU but keeps real 16550s happy.
    write(LCR, 0x80);
    write(0, 3);
    write(1, 0);
    write(LCR, 0x03); // 8N1
    write(FCR, 0x07); // enable and clear FIFOs
    write(IER, 0x01); // receive interrupts
}

pub fn putc(c: u8) {
    while read(LSR) & LSR_THR_EMPTY == 0 {
        core::hint::spin_loop();
    }
    write(THR, c);
}

pub fn getc() -> Option<u8> {
    (read(LSR) & LSR_DATA_READY != 0).then(|| read(RBR))
}

pub fn handle_irq() {
    while let Some(c) = getc() {
        crate::console::on_input(c);
    }
}
