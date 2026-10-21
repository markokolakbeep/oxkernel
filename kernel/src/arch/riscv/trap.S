# Trap entry and exit.
#
# sscratch is 0 while the hart runs in S-mode. While it runs in U-mode it
# holds the top of the current task's kernel stack, where the user TrapFrame
# lives. TrapFrame layout (see trap.rs): x0..x31 at 8*i, sstatus at 256,
# sepc at 264, kernel tp at 272.

    .equ TF_SIZE, 288
    .equ TF_SSTATUS, 256
    .equ TF_SEPC, 264
    .equ TF_KTP, 272

.macro SAVE_GPRS
    .irp n, 1,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    sd      x\n, \n*8(sp)
    .endr
.endm

    .text
    .globl __trap_entry
    .balign 4
__trap_entry:
    csrrw   sp, sscratch, sp
    bnez    sp, from_user

    # From S-mode: undo the swap; sscratch stays 0.
    csrrw   sp, sscratch, sp
    addi    sp, sp, -TF_SIZE
    SAVE_GPRS
    addi    t0, sp, TF_SIZE
    sd      t0, 2*8(sp)
    csrr    t0, sstatus
    sd      t0, TF_SSTATUS(sp)
    csrr    t0, sepc
    sd      t0, TF_SEPC(sp)
    mv      a0, sp
    call    kernel_trap

    # Writing sstatus first turns interrupts off (SIE was 0 when trapping)
    # before sepc is reloaded.
    ld      t0, TF_SSTATUS(sp)
    csrw    sstatus, t0
    ld      t0, TF_SEPC(sp)
    csrw    sepc, t0
    # Restore everything but sp and tp. The task may have been rescheduled
    # onto another hart while it was in the handler, so tp must keep the
    # current hart id.
    .irp n, 1,3,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld      x\n, \n*8(sp)
    .endr
    addi    sp, sp, TF_SIZE
    sret

from_user:
    # sp = kernel stack top, sscratch = user sp.
    addi    sp, sp, -TF_SIZE
    SAVE_GPRS
    csrr    t0, sscratch
    sd      t0, 2*8(sp)
    csrw    sscratch, zero
    csrr    t0, sstatus
    sd      t0, TF_SSTATUS(sp)
    csrr    t0, sepc
    sd      t0, TF_SEPC(sp)
    ld      tp, TF_KTP(sp)
    mv      a0, sp
    call    user_trap
    # user_trap leaves through __return_to_user.
1:  j       1b

# __return_to_user(tf: *mut TrapFrame) -> !
# tf must sit at the very top of the current task's kernel stack.
    .globl __return_to_user
__return_to_user:
    csrci   sstatus, 2
    mv      sp, a0
    sd      tp, TF_KTP(sp)
    addi    t0, sp, TF_SIZE
    csrw    sscratch, t0
    ld      t0, TF_SSTATUS(sp)
    csrw    sstatus, t0
    ld      t0, TF_SEPC(sp)
    csrw    sepc, t0
    .irp n, 1,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    ld      x\n, \n*8(sp)
    .endr
    ld      sp, 2*8(sp)
    sret
