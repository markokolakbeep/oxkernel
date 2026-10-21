# __switch(old: *mut Context, new: *const Context)
# Saves the callee-saved registers of the running thread into *old and
# resumes the thread described by *new.

    .text
    .globl __switch
__switch:
    sd      ra, 0(a0)
    sd      sp, 8(a0)
    .irp n, 0,1,2,3,4,5,6,7,8,9,10,11
    sd      s\n, (16 + \n*8)(a0)
    .endr
    ld      ra, 0(a1)
    ld      sp, 8(a1)
    .irp n, 0,1,2,3,4,5,6,7,8,9,10,11
    ld      s\n, (16 + \n*8)(a1)
    .endr
    ret
