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
- **macOS programs for arm64 and x86-64, the static ones so far.** A Mach-O
  executable enters the same kernel as it would enter XNU (`svc #0x80` or
  `syscall`, BSD calls and Mach traps, the carry flag for an error), and
  the kernel answers in
  Darwin's numbers and layouts: files, processes, signals through XNU's
  frame, threads as libpthread asks for them, `sysctl`, terminals, sockets.
  What is not there yet is what a dynamically linked program starts with:
  dyld and libSystem need the commpage and more of Mach than there is
  (below).
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

- files and directories, `fork`, `execve`, `wait4`, pipes, `mmap` and
  Mach's `mach_vm_*` traps on Darwin's 16 KiB page, the clocks, `select`
  and `poll`;
- signals, delivered through XNU's frame to a handler behind the libc's
  trampoline, and `sigreturn`;
- threads: `bsdthread_register`, `bsdthread_create` and
  `bsdthread_terminate`, thread ports, `ulock_wait` and `ulock_wake`;
- `sysctl` and `sysctlbyname`, `csops`, `proc_info`, `statfs`, extended
  attributes;
- `ioctl` with BSD's `struct termios`, and pseudo-terminals;
- which system it is: `kern.osproductversion`, `kern.osversion` and
  `kern.osrelease` are what the root's
  `/System/Library/CoreServices/SystemVersion.plist` says (macOS 26.0 with
  none), so the version a program sees is a setting of the root it runs
  in, as a Windows version is of a Wine prefix;
- sockets: unix and TCP, descriptors passed in a control message;
- Mach ports within a task: names and the rights under them, `mach_msg2`
  between threads (a reply port, rights in the body, port sets, a receive
  that waits), semaphores (and so `__semwait_signal`, which is how the
  libc sleeps), and the few MIG routines of the task's, the host's and the
  clock's ports that those need.

What does not, yet:

- **dyld and libSystem**, and so any program built the ordinary way. They
  read the commpage, which XNU maps at `0xFFFFFC000`, past the end of any
  WebAssembly memory, and the system's libraries exist only in the dyld
  shared cache, which is mapped at 6 GiB.
- **Mach between tasks.** A port's queue lives in its own process, so no
  right crosses a `fork` or reaches another program: no bootstrap port, no
  launchd, no XPC. Within a task, messages carry ports but not memory out
  of line; nothing is sent when a port dies or loses its senders; and most
  of what the kernel's own ports are asked (`task_info`, `host_info`,
  `vm_region`) answers `MIG_BAD_ID`. The older `mach_msg_trap` is written
  and untested: the tests send as libsyscall does, with `mach_msg2`.
- `kqueue`, `posix_spawn`, the `psynch` calls behind a contended pthread
  mutex, work queues, `getattrlist`, `mknod`.
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
