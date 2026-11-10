# gex-warp

gex-warp is a fork of [gex](https://github.com/HeyPuter/gex) that teaches
its kernel to be other operating systems' kernels too. The first of those
personalities is Darwin: a macOS program enters the kernel as it would
enter XNU. Everything gex does, this does, and the rest of this file is
gex's own with the fork's additions marked where they are.

gex runs unmodified Linux and Windows programs in a web browser. It
translates their machine code to WebAssembly and runs it against its own
implementation of the Linux kernel ABI. That kernel is written in Rust and
compiled to WebAssembly as well. A program needs no source changes and no
rebuild, and nothing in it knows it is not on a Linux machine. In this fork
the same goes for macOS programs, the statically linked ones so far
([below](#run-a-macos-program)).

```console
$ file hello
hello: ELF 64-bit LSB executable, ARM aarch64, statically linked

$ gexc run hello
hello from aarch64
```

The browser is the target. `gexc` also runs programs natively on wasmtime,
for debugging and for building what a page loads ahead of time.

## What runs

- **Linux programs for AArch64, x86-64 and i386.** Static and dynamically
  linked ELF images, with musl or glibc. gex supports threads, `fork`,
  `exec`, signals, pipes, sockets and a full filesystem (tmpfs, host
  passthrough, `/proc`, `/dev`). Each guest process gets its own address
  space. The kernel's shared state is one module that every process's
  Workers share.
- **wasm32 Linux programs.** `toolchain/gexcc` is a C/C++ compiler for gex's
  wasm32 Linux target. It is used like `gcc`. The debdemo repository
  builds Debian packages with it and assembles a Debian rootfs around them.
- **macOS programs for arm64 and x86-64.** A Mach-O executable enters the
  same kernel as it would enter XNU (`svc #0x80` or `syscall`, BSD calls
  and Mach traps, the carry flag for an error), and the kernel answers in
  Darwin's numbers and layouts: files, processes, signals through XNU's
  frame, threads as libpthread asks for them, `sysctl`, terminals, sockets,
  Mach ports. What a program finds above the kernel -- the dynamic linker,
  libSystem, the frameworks -- is [pyrus](https://github.com/MercuryWorkshop/pyrus),
  which is at its beginning: a program built the ordinary way runs if it
  prints and exits.
- **Windows programs, through Wine.** `wine` builds Wine's own
  Unix side as wasm32 for gex, plus a ready-to-run prefix. The Windows code
  itself is translated like any other x86 code. The build is 64-bit Wine
  with WoW64, which runs 64-bit and 32-bit Windows programs in one prefix.
  The older 32-bit-only Wine build is still there, marked legacy, until it
  is removed.
- **Graphics.** `gex-wayland` is a Wayland compositor, and `gex-xwm` the X
  window manager that a rootless Xwayland needs. Wayland and X11 programs
  share one desktop. Xwayland can run as native WebAssembly in the page,
  outside any guest. A guest's OpenGL calls go through gex's own libGL and
  libEGL (`guest/gl`) to Mesa, which is compiled to WebAssembly and draws
  with the browser's WebGPU (`third_party/mesa`, Gallium's `gex_wgpu`).
  Desktop GL is supported up to 4.6.
- **In the page, or ahead of time.** The same programs run in the browser
  page (`page/`) and natively under `gexc`. An ejected project (below) runs
  only what was built for it ahead of time, as a static site whose
  JavaScript library a page of your own imports.

## How it works

```
 guest ELF/PE ─▶ gex-ld ─▶ gex-xlate ─▶ gex-isa(-x86) ─▶ gex-lift(-x86) ─▶ gex-emit ─▶ wasm
                  load      discover     decode            GIR               tier 1
                                              │
                                              ├─▶ gex-fjit   function JIT (GIR → wasm)
                                              └─▶ gex-cgen   function tier (GIR → C → clang → wasm)

 page: hosts/browser (gex-kernel per process) + hosts/kernel (shared kernel) + Mesa + Xwayland
```

- **Guest virtual address equals linear-memory offset.** gex is the
  kernel, so it allocates every guest address itself, inside the guest's
  wasm memory. A guest load is a wasm load, with no translation arithmetic.
- **Three tiers, one ABI.** Tier 1 translates each basic block into a wasm
  function, at load time or when a branch first reaches it. Guest JITs and
  late-loaded libraries need no special handling. The function JIT
  (`gex-fjit`) turns a recovered guest function into one wasm function. The
  function tier (`gex-cgen`) does the same through C and clang, ahead of
  time. All three tiers call each other through one function table.
- **Discovery is never load-bearing.** Code the translator did not find
  lands on a stub, which translates it on demand. A wrong guess costs time,
  never a wrong branch.
- **Ahead-of-time builds.** `gexc run --record` records what a run loaded
  and executed. `gexc eject` turns a recording into a project that builds
  everything ahead of time, and runs only what that build made.
  `site/eject-site.mjs` makes the web build of an
  ejected project into a static site. The site is a JavaScript library that
  your own page imports.

## Layout

| path | what it is |
|------|------------|
| `crates/gexc` | the command-line driver |
| `crates/gex-isa`, `gex-isa-x86` | AArch64 and x86-64 decoders |
| `crates/gex-ir` | GIR, the SSA IR translation goes through |
| `crates/gex-lift`, `gex-lift-x86` | instruction semantics in GIR |
| `crates/gex-f80` | exact x87 80-bit arithmetic |
| `crates/gex-emit` | the wasm writer and tier-1 code generation |
| `crates/gex-fjit` | the function JIT |
| `crates/gex-cgen`, `gex-split` | the function tier, and splitting its units into hot and cold parts |
| `crates/gex-units` | what a build ships of its function-tier units: whole, instrumented, or cut to a slot set |
| `crates/gex-xlate` | code discovery and the translation driver |
| `crates/gex-ld`, `gex-dylink` | ELF, PE and Mach-O loading; shared libraries for wasm32 programs |
| `crates/gex-kernel` | the Linux personality: syscalls, VFS, signals, memory map, processes; and the Darwin one over it (`src/darwin`) |
| `crates/gex-abi`, `gex-mem`, `gex-atomic`, `gex-kmem` | the layout contract and guest-memory access |
| `crates/gex-cache`, `gex-key` | the compiled-module cache and the content keys every host agrees on |
| `crates/gex-wayland`, `gex-xwm`, `gex-gpu` | the compositor, the X window manager, WebGPU compositing |
| `crates/gex-gl` | OpenGL ES passthrough for the native host |
| `crates/gex-eject` | ejected projects, built and run ahead of time |
| `hosts/browser` | the browser host: the kernel compiled to wasm, beside each guest |
| `hosts/kernel` | the shared kernel module: process table, filesystem, pipes, sockets |
| `hosts/hostwl` | the compositor for an X server that runs in the page |
| `hosts/native` | the native host, on wasmtime |
| `guest/` | libraries installed into the guest: GL and EGL, GLX setup, `libsteam_api` |
| `page/` | the page: its source, runtime assets and build (`make page`) |
| `ports/` | the Mesa, Xwayland and Tint builds the page ships |
| `site/` | eject and demo sites built from the page, and recording a Windows program for eject |
| `toolchain/` | gexcc, the wasm32 C/C++ compiler, its sysroots, busybox, and `pins`, the version of every fetched toolchain |
| `wine/` | Wine for gex: dist, prefix, runner |
| `scripts/` | generators for the GL transport and compatibility tables, source fetching, the script lint, profile and unit helpers |
| `test/` | the runtime's tests and their runner, `test/run.mjs`, on two hosts: a headless browser page and Node |
| `third_party/` | wasmtime (with local changes), Mesa, the Khronos GL registry and CTS |

## Building

`make help` lists the targets. Every one of them builds from this tree and
the pins in it, and prints what it made on its last line.

### Run a Linux program

```console
$ make gexc
...
target/release/gexc target/x86/release/gexc

$ target/release/gexc run hello
hello from aarch64
```

`target/release/gexc` runs AArch64 guests, and `target/x86/release/gexc`
runs x86-64 and i386 guests. Some other `gexc` commands:

```console
$ gexc run --wayland SOCKET <elf>   # with the compositor, at SOCKET
$ gexc build <elf> -o out.wasm      # translate only
$ gexc disasm <elf>                 # disassemble
$ gexc ir <elf> --at 0x400abc       # the GIR for one block
$ gexc run --strace <elf>           # syscall trace
```

### Run a macOS program

```console
$ xcrun clang -arch arm64 -static -nostdlib -ffreestanding -e start \
    -Wl,-pagezero_size,0x4000 -Wl,-image_base,0x1000000 -o hello hello.c
$ target/release/gexc run hello
```

`gexc` and the page take a Mach-O where they take an ELF: a thin file for
the guest's processor, or its slice of a universal one (arm64 before
arm64e, x86_64 before x86_64h). An x86-64 program is built with
`-arch x86_64` and run by `target/x86/release/gexc`. The process is a Darwin
process from then on (`crates/gex-kernel/src/darwin`): its system calls are
XNU's, numbered from `bsd/kern/syscalls.master` (`make regen-darwin
XNU_SRC=<an xnu checkout>` regenerates the table), and each is rewritten
into the kernel's own call with its flags, error numbers and structures
respelled, or answered there when only Darwin has it.

A guest's memory is 4 GiB, or 16 GiB at most, and Apple's toolchain links
a program above 4 GiB with everything below unmapped, so the example puts
the image low. What works is what a program can do with the kernel alone:

- files and directories, `fork`, `execve`, `posix_spawn` (with its file
  actions and attributes), `wait4`, pipes, `mmap` and
  Mach's `mach_vm_*` traps on Darwin's 16 KiB page, the clocks, `select`
  and `poll`;
- signals, delivered through XNU's frame to a handler behind the libc's
  trampoline, and `sigreturn`;
- threads: `bsdthread_register`, `bsdthread_create` and
  `bsdthread_terminate`, thread ports, `ulock_wait` and `ulock_wake`, and
  the `psynch` calls, which are the kernel's half of libpthread's mutexes,
  condition variables and read-write locks: XNU's own
  (`libpthread/kern/kern_synch.c`) routine for routine, so that Apple's
  libpthread, unchanged, is what a program's threads run on;
- `sysctl` and `sysctlbyname`, `csops`, `proc_info`, `statfs`, extended
  attributes;
- `getattrlist`, `getattrlistbulk` (a directory's entries, which is how
  Apple's `fts` reads one) and `setattrlist` (by path, by descriptor and
  under one):
  a file's attributes asked for by bit and answered packed as XNU packs
  them, which is how Apple's libc finds a file's real path and sets its
  times; and the calls that carry an access list beside a mode
  (`stat64_extended`, `chmod_extended`), of which no file has one;
- work queues: the threads XNU makes for a process that asks for work to
  be done (`workq_kernreturn`), on stacks of the kernel's laying out,
  given more when they come back and let go when there is none, which is
  where libdispatch's threads come from;
- `kqueue` and `kevent` (and `kevent64`, `kevent_qos`): a descriptor to
  read or write, timers, events a program posts to itself, a process
  ending, a signal raised, a file or a directory changing (heard as the
  kernel's `inotify` hears it, so of what is done through gex), a Mach
  port or a set of them with a message, which the queue receives if asked
  to (into the note's buffer, or `kevent_qos`'s); told each time or once,
  and waited for;
- `ioctl` with BSD's `struct termios`, and pseudo-terminals;
- which system it is: `kern.osproductversion`, `kern.osversion` and
  `kern.osrelease` are what the root's
  `/System/Library/CoreServices/SystemVersion.plist` says (macOS 26.0 with
  none), so the version a program sees is a setting of the root it runs
  in, as a Windows version is of a Wine prefix;
- sockets: unix and TCP, descriptors passed in a control message;
- POSIX shared memory by name (`shm_open`), with XNU's rules for it (sized
  once and in whole pages, mapped shared and within its size). It is the
  kernel's own, a file of `/dev/shm`, so a Linux program and a Mac one
  that name the same memory share it;
- the commpage: the page of facts about the machine libSystem reads
  instead of asking, and on x86-64 computes the time from. XNU's is past
  the end of a guest's memory, so it is at `0xFFC000` (arm64's read-only
  half at `0xFF4000`), and a userland built for gex is built to look
  there;
- Mach ports within a task: names and the rights under them, `mach_msg2`
  between threads (a reply port, rights in the body, port sets, a receive
  that waits), timers the kernel sends a message for (`mk_timer_create`,
  what a run loop waits on), semaphores (and so `__semwait_signal`, which is how the
  libc sleeps), and the few MIG routines of the task's, the host's and the
  clock's ports that those and libpthread need (`host_info` among them),
  what a task and a thread say of themselves (`task_info`,
  `thread_info`: their memory and times, who they run as, which thread),
  the regions of a task's memory (`mach_vm_region`, and `_recurse`),
  and the kernel's notifications (`mach_port_request_notification`): that
  a port has no senders left, that the port a name was a right to has
  died or the name has gone, that a send-once right was never used;
- restartable ranges (`task_restartable_ranges_register`, and
  `_synchronize`): code a task says a thread may be taken out of, which is
  how the Objective-C runtime knows no thread still reads a method cache it
  is about to free. A thread is sent to a range's recovery address where
  its program counter is exact (the poll a loop makes), and the task's
  request waits until every other thread has been seen there or in a
  system call;
- the ring a task lists memory in for the kernel to take back
  (`mach_vm_reclaim`), where libmalloc keeps what it has freed and may
  want again: taken back when the task asks, and each time it says how
  much it holds.

What does not, yet:

- **Apple's own dyld and libSystem.** A dynamically linked program is
  loaded, slid into the guest's memory, and entered through the dynamic
  linker its root has at `/usr/lib/dyld` (`gexc run --sysroot ROOT`): pyrus
  builds such a root, with a linker and a libSystem of its own for now.
  Apple's read the commpage, which XNU maps at `0xFFFFFC000`, past the end
  of any WebAssembly memory, and a Mac's own libraries exist only in the
  dyld shared cache, which is mapped at 6 GiB.
- **Mach between tasks.** A port's queue lives in its own process, so no
  right crosses a `fork` or reaches another program: no bootstrap port, no
  launchd, no XPC. Within a task, messages carry ports and memory out of
  line but not arrays of ports, and what the kernel's own ports would
  answer with either (`task_threads`, `host_processor_info`) is
  `MIG_BAD_ID`, as much else of theirs is. The older `mach_msg_trap` is written
  and untested: the tests send as libsyscall does, with `mach_msg2`.
- A kqueue's events delivered to a work queue's threads, and workloops;
  a file made longer by `ftruncate`
  told apart from one written to, and one renamed over from one removed
  (`EVFILT_VNODE` says `NOTE_WRITE` of the first and `NOTE_LINK` too of
  the second);
  `mknod`; a mutex or condition shared between
  processes (each is its own process's).
- A floating-point environment beyond what WebAssembly has: the rounding
  mode and the exception flags a program sets are kept and read back, but
  arithmetic is to nearest and raises none.
- **Other Macs.** Three more processors have run Mac OS X, and none is
  here:
  - *i386* (through macOS 10.14). gex translates i386 and its kernel has
    32-bit layouts for Linux, so this is the nearest: a loader for 32-bit
    Mach-O, the `int 0x80`/`0x81`/`0x82` and `sysenter` entries with their
    arguments on the stack, and an ILP32 spelling of every structure this
    personality converts. Nothing on a current Mac runs i386, so its
    reference would have to be recorded on an old one.
  - *PowerPC and PowerPC 64* (through 10.5, and under Rosetta to 10.6).
    gex had a PowerPC guest, with the big-endian guest memory it needs,
    until 73601b3 removed it; `archive/nt-pewasm` (29471aa2) keeps the
    code. Bringing it back is the larger part; Darwin's entry there is
    `sc`, which returns one instruction further on when a call succeeds.
  - *x86_64h* is x86-64 for Haswell and later: its slice is taken only
    when a file has no plain one, and runs as far as the translator knows
    the instructions.
- **Old systems' programs.** The calls from before 64-bit inodes are
  answered (`stat`, `fstat`, `lstat`, `fstatat`, `getdirentries`, `statfs`,
  `fstatfs` and `getfsstat` under their first numbers, with the structures
  they had). What an old program needs beyond the kernel is the libSystem
  it was linked against, which is the userland's to provide.
- A page fault is not delivered where XNU would deliver one: nothing in a
  WebAssembly memory is protected, so a write to a read-only page succeeds.

The tests (`test/darwin`) build each program in `test/fixtures/darwin`
twice: statically for gex, and dynamically for the Mac the tests run on,
whose kernel is the reference. They pass when the two print the same, under
`gexc` and on the page's runtime. They need a Mac with Xcode's command-line
tools, and skip anywhere else. `--arch a64,x86` runs both processors; the
x86-64 programs' reference is Rosetta, which answers a few things for the
Mac under it (the page size, the processor, Mach's clock in the host's
ticks), and those the x86-64 tests do not ask.

#### Beside Linux programs

A Mac program in a system that is Linux's runs in a root of its own, as a
Windows program runs in a Wine prefix: a directory with a Mac's tree in it,
its dynamic linker at `usr/lib/dyld` (pyrus builds one). When a process is
about to become a Mach-O program, the kernel enters the root its
environment names (`GEX_DARWIN_ROOT`), or else the nearest directory above
the program that has a linker, and the program runs with that as its `/`.
What the two systems share is there too: the devices, `/tmp` (a Mac's
`/private/tmp`), and the whole of the system the root is in, as the volume
`/Volumes/Linux`; the working directory is the same directory by its name
there. A program in the root that is no Mac's leaves it for the tree it
was entered from.

```console
$ /opt/mac/bin/bash -c 'uname -s; pwd; /Volumes/Linux/bin/uname -s'
Darwin
/Volumes/Linux/home/user
Linux
```

This is the page's (`crates/gex-kernel/src/darwin/root.rs`), where each
process has a kernel of its own; `gexc` runs programs of one kind at a
time, and a Mac program there is given its root with `--sysroot`. A
system whose own tree is a Mac's enters nothing.

A page starts a Mac program as it starts any (`runProgram`), by its name
in a root that is a directory of the system's tree
(`test/darwin/prefix.test.js`), or in a tree that is a Mac's whole:
`test/darwin/terminal.test.js` runs Apple's bash from such a root on a
terminal in a browser tab, typing at its prompt.
And a Mac program draws where a Linux one does: the page's compositor is
a Wayland compositor at a socket in `/tmp`, which a Mac program reaches as
any program does. `test/darwin/window.test.js` runs one that speaks the
protocol itself (a Mac has no library for it), its pixels in shared memory
whose descriptor it passes, and the compositor shows its window.

### Build the page

```console
$ make page                         # the multi-process build, page/dist
$ make serve                        # http://localhost:8088/
```

`page/README.md` describes the two builds (`make page-sp` is the
single-thread one), the runtime package (`make runtime`) and ejected
programs' sites.

### Run a Windows program

```console
$ make wine
...
WINE64_DIST=...
WINE64_PREFIX=...
```

`wine/README.md` covers running a program on it.

### Build a program with gexcc

```console
$ make gexcc
...
out/gexcc/bin/gexcc
$ out/gexcc/bin/gexcc -O2 -o hello hello.c && target/x86/release/gexc run hello
```

### What the build needs

- the Rust toolchain from `rust-toolchain.toml` (it installs the
  `wasm32-unknown-unknown` target);
- clang and `wasm-ld` (LLVM 22), and Node.js 22;
- for the page: Emscripten (`emsdk`) and `wasm-tools`
  (`cargo install wasm-tools`);
- for Wine: the host's MinGW-w64 GCC 12 (`x86_64-w64-mingw32-gcc`, and
  `i686-w64-mingw32-gcc` for WoW64); it is a prerequisite, not a pin;
- for AArch64 tests: `qemu-aarch64` as the reference. The musl cross
  toolchains are fetched by `make toolchains` at the versions in
  `toolchain/pins`.

Fetched sources (musl, LLVM, Wine) are kept in bare mirrors under
`~/.cache/gex` (`CACHE=`). A rerun fetches nothing it already has. The
Makefile's header lists the settings.

## Tests

`make test` runs the runtime's tests (`node test/run.mjs`; `test/README.md`
gives its options, passed as `ARGS="..."`). `make check` runs each crate's
own tests (`cargo test`). Every other test lives in the gexdev repository.

## Licence

gex is free software under the GNU Affero General Public License, version 3
only (`LICENSE`). Copyright (C) 2026 Puter Technologies Inc. Third-party
code keeps its own licence; `NOTICE` lists it.
