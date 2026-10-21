/*
 * oxkernel is linked in the higher half: virtual 0xffffffc080200000 maps to
 * physical 0x80200000, where OpenSBI jumps. KERNEL_OFFSET is also the base of
 * the direct map of physical memory (see mm/mod.rs).
 */
OUTPUT_ARCH(riscv)

KERNEL_OFFSET = 0xffffffc000000000;
KERNEL_BASE = 0xffffffc080200000;

SECTIONS
{
    . = KERNEL_BASE;
    __kernel_start = .;

    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        __text_start = .;
        *(.text.entry)
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    }

    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        __rodata_start = .;
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
        __data_start = .;
        *(.data.boot_pagetable)
        *(.data .data.* .sdata .sdata.*)
        *(.got .got.*)
    }

    /* Boot stacks sit outside [__bss_start, __bss_end) so that clearing .bss
     * does not wipe the stack we are running on. */
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_OFFSET) {
        . = ALIGN(4096);
        *(.bss.stack)
    }

    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET) {
        . = ALIGN(8);
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.*)
        . = ALIGN(8);
        __bss_end = .;
    }

    . = ALIGN(4096);
    __kernel_end = .;

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}

/* QEMU starts the image at its physical entry point. */
_start_phys = _start - KERNEL_OFFSET;
ENTRY(_start_phys)
