/* User programs: static executables loaded at 0x10000. Each output section
 * starts on a page so segments never share a page with different rights. */
OUTPUT_ARCH(riscv)
ENTRY(_start)

SECTIONS
{
    . = 0x10000;
    .text : {
        *(.text.entry)
        *(.text .text.*)
    }
    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.* .srodata .srodata.*)
    }
    . = ALIGN(4096);
    .data : {
        *(.data .data.* .sdata .sdata.*)
        *(.got .got.*)
    }
    .bss : {
        *(.sbss .sbss.* .bss .bss.*)
    }
    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
