# Boot entry points. OpenSBI enters in S-mode with the MMU off:
#   a0 = hart id, a1 = physical address of the devicetree blob.

    .equ KERNEL_OFFSET, 0xffffffc000000000
    .equ BOOT_STACK_SHIFT, 16       # 64 KiB; keep in sync with config::BOOT_STACK_SIZE
    .equ BOOT_STACK_SIZE, 1 << BOOT_STACK_SHIFT
    .equ MAX_HARTS, 8               # keep in sync with config::MAX_HARTS
    .equ SATP_SV39, 8 << 60

# Turns on Sv39 with boot_pagetable, which maps the kernel both at its
# physical address and in the higher half, then jumps to the high alias.
.macro ENABLE_PAGING_AND_GO_HIGH
    .option push
    .option norelax
    la      t0, boot_pagetable      # PC-relative, so this is physical
    srli    t0, t0, 12
    li      t1, SATP_SV39
    or      t0, t0, t1
    csrw    satp, t0
    sfence.vma
    li      t1, KERNEL_OFFSET
    la      t0, 1f
    add     t0, t0, t1
    jr      t0
1:
    .option pop
.endm

# sp = top of this hart's boot stack, tp = hart id. Parks harts we have no
# stack for.
.macro SETUP_HART_STACK
    li      t0, MAX_HARTS
    bgeu    a0, t0, park
    la      sp, boot_stacks
    addi    t0, a0, 1
    slli    t0, t0, BOOT_STACK_SHIFT
    add     sp, sp, t0
    mv      tp, a0
    mv      fp, zero
.endm

    .section .text.entry
    .globl _start
_start:
    ENABLE_PAGING_AND_GO_HIGH
    la      t0, __bss_start
    la      t1, __bss_end
2:  bgeu    t0, t1, 3f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       2b
3:
    SETUP_HART_STACK
    call    kmain
    j       park

    .text
    .globl _start_secondary
_start_secondary:
    ENABLE_PAGING_AND_GO_HIGH
    SETUP_HART_STACK
    call    kmain_secondary
park:
    wfi
    j       park

# Sv39 root table used only while booting. 1 GiB gigapages, V|R|W|X|A|D.
#   entry 2:        identity map of 0x80000000..0xc0000000 (the jump high)
#   entries 256-259: physical 0..4 GiB at KERNEL_OFFSET (the direct map)
    .section .data.boot_pagetable, "aw"
    .balign 4096
    .globl boot_pagetable
boot_pagetable:
    .quad 0
    .quad 0
    .quad (0x80000 << 10) | 0xcf
    .zero 8 * 253
    .quad (0x00000 << 10) | 0xcf
    .quad (0x40000 << 10) | 0xcf
    .quad (0x80000 << 10) | 0xcf
    .quad (0xc0000 << 10) | 0xcf
    .zero 8 * 252

    .section .bss.stack, "aw", @nobits
    .balign 4096
    .globl boot_stacks
boot_stacks:
    .space BOOT_STACK_SIZE * MAX_HARTS
