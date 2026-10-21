# oxkernel architecture

This document follows the kernel from power-on to a running shell, then
covers each subsystem. File references are relative to `kernel/src` unless
noted.

## 1. Boot

```
QEMU  ->  OpenSBI (M-mode, 0x80000000)  ->  _start (S-mode, 0x80200000, MMU off)
                                              a0 = hart id, a1 = DTB physical address
```

OpenSBI picks the boot hart at random, so `_start` (`arch/riscv/entry.S`) makes
no assumptions about which hart it is running on:

1. It loads `boot_pagetable`, a static Sv39 root table with four 1 GiB
   gigapages that map physical 0–4 GiB at `KERNEL_OFFSET`
   (`0xffffffc000000000`), plus one identity gigapage over the kernel. Then it
   writes `satp`.
2. It jumps to the high alias of the next instruction. `la` is PC-relative,
   so before the jump it yields physical addresses and after it virtual ones.
3. It clears `.bss`, sets `sp` to this hart's 64 KiB boot stack and
   `tp = hart id`, and calls `kmain`.

`kmain` (`main.rs`) then:

- parses the devicetree with `oxcore::fdt` (RAM, harts, timebase, bootargs);
- starts the frame allocator on the RAM after the kernel, keeping the DTB
  reserved;
- builds the real kernel page table (`mm/paging.rs::init_kernel`): text R+X,
  rodata R, data and bss R+W, the rest of RAM R+W, the MMIO devices, and all
  64 kernel stacks. Then it switches to it, dropping the identity map;
- sets up traps, the PLIC, the timer, virtio-blk and the buffer cache;
- creates pid 1 and starts the other harts with SBI HSM `hart_start`, at
  `_start_secondary`'s physical address;
- enters the scheduler.

Pid 1 begins in `init_task_entry`. That runs in task context, because
mounting the file system sleeps on disk I/O. It mounts oxfs and execs
`/bin/init`, passing the kernel command line (`-append`) as arguments.

## 2. Memory

### Virtual memory map (Sv39)

```
0xffff_ffff_0000_0000  kernel stacks: 64 slots of [guard page | 16 KiB stack]
        ...            (unmapped)
0xffff_ffc0_8020_0000  kernel image: .text .rodata .data .stack .bss
0xffff_ffc0_8000_0000  RAM direct map (OpenSBI's 2 MiB left unmapped)
0xffff_ffc0_1000_1000  virtio-mmio slots
0xffff_ffc0_1000_0000  UART
0xffff_ffc0_0c00_0000  PLIC
0xffff_ffc0_0010_0000  SiFive test device (power off with an exit code)
0xffff_ffc0_0000_0000  KERNEL_OFFSET: physical address 0
        ...            (non-canonical hole)
0x0000_003f_ffff_f000  user stack top (64 KiB stack below, guard page below that)
0x0000_0020_0000_0000  heap limit
        ...            heap (sbrk) grows up from the end of the image
0x0000_0000_0001_0000  user program (.text, .rodata, .data, .bss)
0x0000_0000_0000_0000  never mapped (null pointers fault)
```

Every process has its own root table. Slots 256–511, the kernel half, are
copied from the kernel's root table when an address space is created. The
kernel creates every second-level table it will ever need at boot, so those
shared slots never change afterwards. The result is that a trap needs no
`satp` switch and the kernel has no trampoline page.

### Allocators

- **Frames:** `oxcore::buddy`, a binary buddy allocator with orders 0–10
  (4 KiB to 4 MiB). Free blocks sit on intrusive doubly linked lists. A side
  table of one byte per page records "head of a free block of order k". That
  makes the buddy check O(1) and catches double frees.
- **Kernel heap:** `oxcore::slab` with power-of-two classes from 16 B to
  2 KiB, carved out of single pages; anything larger is a buddy allocation.
  User space uses the same slab code, fed by `sbrk` (`user/src/heap.rs`).

Both algorithms live in `oxcore`, which is unit-tested on the host. That
includes a randomized stress test that checks no two live blocks overlap.

### Touching user memory

The kernel never dereferences a user pointer. `AddressSpace::copy_in`,
`copy_out` and `copy_in_str` walk the process's page table one page at a time,
check `U` plus `R` or `W`, and copy through the direct map. A bad pointer turns
into `EFAULT` instead of a kernel page fault, and `sstatus.SUM` is never
set.

## 3. Traps

```
U-mode trap:  sscratch = kernel stack top  --csrrw-->  sp = kstack top, sscratch = user sp
S-mode trap:  sscratch = 0                 --csrrw-->  sp = 0  =>  swap back, push frame
```

`arch/riscv/trap.S` uses a single `stvec` entry. After the swap, a non-zero
`sp` means the trap came from user mode.

- **From U-mode** the `TrapFrame` (all registers, `sstatus`, `sepc`) goes
  at the very top of the task's kernel stack. `tp` is reloaded from the frame's
  `kernel_tp` slot, which `__return_to_user` filled in when the task last
  left the kernel. `user_trap` handles the syscall, interrupt or fault and
  leaves through `__return_to_user`.
- **From S-mode** the frame is pushed on the current stack, and
  `kernel_trap` handles interrupts. A timer tick preempts kernel code too,
  as long as the task holds no spinlocks. On the way back, `tp` is **not**
  restored: the task may have been rescheduled onto a different hart while it
  was preempted, and `tp` must name the hart it is on now.
- `__return_to_user` disables interrupts before writing `sscratch`, and the
  `sstatus` it installs has `SIE` clear. If an interrupt could arrive between
  those steps, it would see a non-zero `sscratch` and take the user path while
  still in S-mode.
- A fault on a kernel stack guard page is reported as a kernel stack
  overflow. User faults kill only the offending process.

## 4. Tasks and scheduling

Locks are taken in this order:
`WAIT_LOCK -> TABLE -> Task::inner -> SCHED`, with the heap and frame locks
as leaves.

### Spinlocks

A `SpinLock` disables interrupts while held, with xv6-style
`push_off`/`pop_off` nesting kept per hart. An interrupt handler can
therefore never spin on a lock its own hart holds. Each lock records its
owner, so taking it recursively panics instead of hanging.

### The switch protocol

The run queue and every task's scheduling state (`state`, `chan`, `hart`) sit
behind one `SCHED` lock.

```
task:      lock SCHED; state = Runnable/Sleeping/Zombie; __switch(task -> scheduler)
scheduler: (now holds SCHED) push the task onto runq / sleepers; unlock SCHED
scheduler: lock SCHED; pop a task; state = Running; __switch(scheduler -> task)
task:      (now holds SCHED) unlock SCHED
```

The lock is handed across `__switch` in both directions. A task goes back on
the run queue only *after* its hart has switched completely off its stack.
That is the property that stops two harts from ever running the same task.
New tasks start in `fork_return`, which releases the lock they inherited.

### Sleep and wakeup

`sleep(chan, guard)` takes SCHED *before* releasing the caller's lock. A
`wakeup(chan)` needs SCHED, so it cannot slip in between the caller's last
check of its condition and the task being marked asleep. Every sleep sits
in a `while !condition` loop, and every loop that can block for a long time
also checks the `killed` flag.

### Idle and IPIs

An idle hart polls the run queue for 50 µs and then executes `wfi`. A
wakeup or a new task sends an SBI IPI to one idle hart.

### Lazy address-space switching

Each hart records the `satp` it has loaded (`mm/paging.rs::LOADED`). The
scheduler skips the `satp` write and TLB flush when it resumes the task this
hart ran last, *and* that task has not run anywhere else since. Only a task
changes its own mappings, and it always flushes the hart it is running on, so
in that case the TLB is still correct for it. Two rules keep this safe:

- an idle hart, or one whose task just exited, loads the kernel's table;
- `AddressSpace::drop` waits until no hart has the table loaded before it
  frees the root page. Otherwise a hart could walk a freed page table.

### Process lifecycle

- **fork** copies every user page eagerly and shares open files (`Arc<File>`)
  and the working directory.
- **exec** reads the ELF header and program headers through the inode layer
  and maps each `PT_LOAD` segment page by page, never as one big buffer. It
  places argv on a fresh stack, swaps the address space in, and rewrites the
  trap frame. argc goes back through a0 as the syscall's result.
- **exit** closes files first, because that may sleep. Then, under
  `WAIT_LOCK`, it hands any children to init, wakes the parent, and switches
  away as a zombie.
- **wait** reaps a zombie child. The child's memory and kernel stack are freed
  when its last `Arc<Task>` drops.
- **kill** sets a flag and wakes the task if it is asleep. The task exits the
  next time it crosses the kernel/user boundary or checks the flag in a sleep
  loop. The console sends ^C to every process in the foreground process group,
  which the shell sets with `setfg`.

## 5. Storage

### virtio-blk (`drivers/virtio_blk.rs`)

The driver speaks virtio-mmio version 2 with one 16-entry split virtqueue.
The descriptor table, avail and used rings, request headers and status bytes
all share one page. A request is a three-descriptor chain: header, data,
status. The submitting task sleeps on a per-request channel, and the
interrupt handler walks the used ring and wakes it. Several harts can have
requests in flight at once.

The driver negotiates `VIRTIO_F_VERSION_1` and `VIRTIO_BLK_F_FLUSH`. Without
FLUSH, QEMU runs the disk write-through, which made every write 26× slower.

### Buffer cache (`fs/bcache.rs`)

There are 64 block buffers. Block number, reference count and validity are
kept under a spinlock, and each buffer's contents behind a sleep lock. Free
buffers are recycled least-recently-used first. Writes go straight to disk.

### oxfs (`fs/inode.rs`, layout in `oxcore/src/fs.rs`)

```
| superblock | inode bitmap | block bitmap | inode table (64 B inodes) | data blocks |
```

- Blocks are 4 KiB. An inode has 11 direct pointers and 1 indirect block,
  so a file can be up to 4.2 MB.
- A directory entry is 32 bytes: an inode number and a name of up to 28
  bytes. Every directory holds `.` and `..`.
- In-memory inodes are cached, xv6 style. When the last reference to an
  inode with no links drops, its blocks and its inode are freed.
- Path lookup locks one inode at a time. `create` and `unlink` lock the parent
  directory before the child.

`mkfs` (host side) builds the image from the same `oxcore::fs` layout code,
and its unit test reads the image back through that code.

There is no journal: a crash in the middle of an operation can leave the bitmaps
and the inodes out of step.

## 6. System calls

`a7` holds the number and `a0`–`a5` the arguments. The result comes back in
`a0`, negative errno on failure. The numbers are in `oxcore/src/abi.rs`, which
the kernel and `ulib` share.

| # | Call | # | Call |
|---|---|---|---|
| 1 | `exit(code)` | 14 | `close(fd)` |
| 2 | `fork()` | 15 | `dup(fd)` |
| 3 | `wait(*status)` | 16 | `pipe(*fds)` |
| 4 | `exec(path, argv)` | 17 | `fstat(fd, *stat)` |
| 5 | `getpid()` | 18 | `mkdir(path)` |
| 6 | `sbrk(incr)` | 19 | `chdir(path)` |
| 7 | `sleep(ms)` | 20 | `unlink(path)` |
| 8 | `time_us()` | 21 | `procinfo(*buf, max)` |
| 9 | `yield()` | 22 | `setpgid()` |
| 10 | `kill(pid)` | 23 | `setfg(pgid)` |
| 11 | `read(fd, buf, n)` | 24 | `sysinfo(*info)` |
| 12 | `write(fd, buf, n)` | 25 | `shutdown(code)` (init only) |
| 13 | `open(path, flags)` | | |

User code may also read the `time`, `cycle` and `instret` CSRs directly;
the kernel enables them in `scounteren`.

## 7. Testing

`cargo xtask test` runs three layers:

1. **Host unit tests** (`oxcore`, `mkfs`): the allocators, the parsers, the
   on-disk format and the image builder.
2. **In-kernel tests** (`ktest.rs`, `--features ktest`): frames, heap,
   address-space copies and fork, interrupt nesting, raw disk I/O, files,
   pipes, the timer and SMP bring-up. They run in pid 1 and power QEMU off
   through the SiFive test device. The exit code is the result.
3. **usertests** (`user/src/bin/usertests.rs`), booted with
   `-append test` on 1 and 4 harts. These 18 stress tests each run in their
   own process. They cover fork storms up to process-table exhaustion, exec
   arguments, 100 KB through a pipe, `EPIPE`, sbrk, faulting children,
   `EFAULT` on hostile pointers, killing spinning and sleeping children,
   300 KiB files through the indirect block, reclaiming disk space, directory
   semantics, concurrent writers, fd limits and shared offsets, and a check
   that free memory is unchanged across repeated fork/exec/pipe rounds.

The run fails on a timeout, a non-zero exit code, or a missing
`ALL TESTS PASSED` line.
