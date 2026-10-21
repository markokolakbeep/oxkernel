# oxkernel

A small Unix-like operating system kernel for 64-bit RISC-V, written from
scratch in Rust. It boots on QEMU's `virt` machine through OpenSBI and runs a
shell with pipes, redirection and background jobs, on up to 8 harts.

```
oxkernel 0.1.0 booting on hart 1
fdt: 256 MiB RAM at 0x80000000, 4 harts, timebase 10000000 Hz
mm: kernel page table active (satp 0x80000000000802b6)
virtio-blk: slot 0, 32 MiB
smp: hart 2 online (2 up)
smp: hart 3 online (4 up)
smp: hart 0 online (3 up)
oxfs: 8192 blocks (32 MiB), 1024 inodes, data from block 19
init: starting sh
$ echo hello oxkernel | wc
      1       2      15
$ mkdir /tmp/x; cd /tmp/x; echo inside > f; cat f; ls; cd /
inside
d   23       96 .
d    4       96 ..
-   24        7 f
$ spin 30 &
[14]
$ ps
  PID  PPID  PGID STATE     HART    MEM NAME
    1     0     1 sleeping     3   100K init
    2     1     1 sleeping     1   132K sh
   14     2    14 sleeping     0   132K sh
   15    14    14 running      1    92K spin
   16     2    16 sleeping     2   132K sh
   17    16    16 running      0    96K ps
```

Everything below the shell is our own code: boot, page tables, allocators, the
scheduler, the virtio-blk driver and the file system. The kernel has no
dependencies besides `core` and `alloc`.

## Features

| Area | What is there |
|---|---|
| Boot | Higher-half kernel (`0xffffffc080200000`), its own devicetree parser, harts started via SBI HSM, any hart may be the boot hart |
| Memory | Buddy frame allocator, slab heap for `alloc`, Sv39 paging with W^X kernel sections, a direct map of RAM, guard pages under every kernel stack |
| Processes | `fork` (eager copy), `exec` of static ELF64, `exit`/`wait`/`kill`, process groups, ^C to the foreground job |
| Scheduling | Preemptive (10 ms tick, kernel code preemptible too), SMP with a global run queue, sleep/wakeup channels, IPIs to idle harts, lazy address-space switching |
| Storage | virtio-blk (modern MMIO, split virtqueue, interrupt driven), LRU buffer cache, **oxfs**: bitmaps, 64-byte inodes with direct and indirect blocks, directories |
| Files | File descriptors shared across `dup`/`fork`, pipes, a console with line editing |
| User space | `ulib` runtime (syscalls, heap, `println!`), `init`, `sh`, `ls cat echo wc mkdir rm ps kill sleep sysinfo spin true` |
| Testing | 23 host unit tests, 9 in-kernel tests, 18 user-space stress tests on 1 and 4 harts; lint and tests run in GitHub Actions |

About 6,900 lines of Rust and assembly, not counting comments:

| Crate | Lines | Purpose |
|---|---|---|
| `kernel` | 3,700 | The kernel |
| `oxcore` | 1,300 | Code with no hardware access, unit-tested on the host: buddy + slab allocators, ELF and FDT parsers, oxfs layout, syscall ABI |
| `user` | 1,400 | `ulib` and the user programs |
| `mkfs`, `xtask` | 500 | Disk image builder; build/run/test driver |

## Running it

You need Rust (the toolchain is pinned in `rust-toolchain.toml`; `rustup`
installs it on first use) and QEMU:

```sh
sudo pacman -S qemu-system-riscv      # Arch
sudo apt install qemu-system-misc     # Debian / Ubuntu

cargo xtask run                 # boot to the shell on 4 harts (quit: Ctrl-A X)
cargo xtask run --smp 1         # single hart
cargo xtask test                # every test: host, in-kernel, usertests on 1 and 4 harts
cargo xtask lint                # rustfmt --check and clippy -D warnings on all crates
cargo xtask bench               # micro-benchmarks (release build)
cargo xtask debug               # QEMU waits for gdb on :1234
cargo xtask addr2line 0x...     # symbolize a panic backtrace
```

Things to try in the shell: `ls /bin`, `cat /README | wc`,
`echo hi > /tmp/f; cat /tmp/f`, `spin 20 &` and then `ps`, `spin` followed by
^C, `usertests`, `bench`.

## How it works

[docs/architecture.md](docs/architecture.md) covers the design in detail:
the memory map, how a trap enters and leaves the kernel, the scheduler's
locking, sleep/wakeup, the file system layout, and the syscall table. In
short:

- **Boot.** OpenSBI enters `_start` at physical `0x80200000` with paging off.
  A static boot page table maps the kernel both at its physical address and in
  the higher half using 1 GiB pages. `_start` enables it, jumps to the high
  alias, and Rust code then builds the real page table with 4 KiB
  permissions.
- **One page table per process, no trampoline.** The kernel half of the
  root table (slots 256..511) is shared by every address space, so a trap does
  not have to switch `satp`. User memory is reached through the direct map
  after a permission-checked page-table walk, never by dereferencing user
  pointers.
- **Traps.** `sscratch` is 0 in S-mode and holds the task's kernel stack
  top in U-mode; one `csrrw` tells the two cases apart. Kernel traps do not
  restore `tp` (the hart id), because a preempted task may resume on another
  hart.
- **Scheduler.** A task switching out holds the SCHED lock across `__switch`
  and the scheduler releases it. A task goes back on the run queue only after
  its hart is completely off its stack, so two harts can never run it at
  once.

## Measurements

From `cargo xtask bench` (release build) under QEMU 11.1 TCG on a 4-core
host. This is **emulated** time: the numbers are useful for comparing with
each other and for spotting regressions, not as hardware figures.

| Benchmark | 1 hart | 4 harts |
|---|---|---|
| null syscall (`getpid`) | 3.1 µs | 3.3 µs |
| `yield`, nothing else runnable | 3.9 µs | 11.4 µs ¹ |
| switch between two processes | 18.5 µs | 12.3 µs |
| pipe ping-pong round trip | 105 µs | 88 µs |
| `fork` + `exit` + `wait` | 0.6 ms | 1.1 ms |
| `fork` + `exec` + `wait` | 3.5 ms | 5.1 ms |
| pipe throughput | 30 MiB/s | 60 MiB/s |
| file write / read | 1.0 / 6.2 MiB/s | 3.4 / 19 MiB/s |
| CPU-bound jobs, 1 → 4 | | ×3.3–3.4 typical ² |

¹ With several harts, an idle hart that is polling can pick up the
yielded task, so the task migrates and its page table has to be reloaded.
² Same work per job; perfect scaling would be ×4. The run-to-run spread is
large because QEMU's 4 vCPU threads share the 4 host cores with its I/O
thread (one run gave ×2.2).

Switching between processes costs about 15 µs more than a `yield` that stays
in one address space. That gap is the `satp` write plus `sfence.vma`: QEMU
throws away its whole software TLB and refills it page by page. It dominates
the pipe round trip, which is two such switches plus four syscalls.

Finding these performance bugs was the best part of the measurement work:

- **Disk writes were 26× slower than reads** (3 ms against 0.11 ms per
  block). The driver did not negotiate `VIRTIO_BLK_F_FLUSH`, and a device whose
  guest cannot flush has to behave as write-through, so QEMU synced the image to
  the host disk on every write. Negotiating FLUSH (and flushing at shutdown)
  brought writes to 0.12 ms; the `disk_space_reclaimed` stress test went
  from 227 s to 11 s.
- **`yield` cost 25.6 µs against 3 µs for a syscall.** Every switch wrote
  `satp` twice (task → kernel → task). The scheduler now switches lazily: a
  hart keeps a task's page table loaded and skips the reload when it resumes
  that same task, as long as the task has not run on another hart in between
  (that hart could have changed its mappings, and our TLB would be stale). A
  per-hart record of the loaded `satp` makes sure no address space is freed
  while some hart can still walk it. `yield` went to 3.9 µs, 6.5× faster.
- **Cross-hart ping-pong took 204 µs**, because every hop woke a hart out of
  `wfi` with an IPI. Idle harts now poll the run queue for 50 µs before
  sleeping: 88 µs.
- **The `parallel_cpu` test finished 4 M multiplications in 9 ms.** The
  compiler had deleted the loop, because `(x & 1) * 0` does not depend on it;
  the scaling benchmark had the same bug. `black_box` fixed both.

## Limitations and next steps

- `fork` copies eagerly; copy-on-write would make `fork + exec` much cheaper.
- oxfs has no journal, so a crash mid-write can leave it inconsistent. A
  write-ahead log, as in xv6, is the natural next step.
- One global run queue and one SCHED lock; per-hart queues with work stealing
  would scale further.
- Every address space uses ASID 0, so each switch flushes the TLB. Per-process
  ASIDs would avoid that, but recycling an ASID safely needs cross-hart TLB
  shootdowns (SBI remote fences).
- No floating point in user space (the targets are `riscv64imac`), no signals
  beyond kill, no `mmap`.

## License

MIT
