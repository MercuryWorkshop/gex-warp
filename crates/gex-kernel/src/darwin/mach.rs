//! Mach traps.
//!
//! What a Darwin process asks of Mach directly rather than through a BSD
//! call: its own ports' names, virtual memory, and time. A trap answers
//! with a `kern_return_t` (or the value itself, for the traps that return
//! a port name or a time), never through the carry flag.

use super::abi;
use super::nr::trap as t;
use super::*;
use crate::syscalls::nr;

pub const KERN_SUCCESS: u64 = 0;
pub const KERN_INVALID_ADDRESS: u64 = 1;
pub const KERN_PROTECTION_FAILURE: u64 = 2;
pub const KERN_NO_SPACE: u64 = 3;
pub const KERN_INVALID_ARGUMENT: u64 = 4;
pub const KERN_FAILURE: u64 = 5;
pub const KERN_ABORTED: u64 = 14;
pub const KERN_INVALID_NAME: u64 = 15;
pub const KERN_NOT_SUPPORTED: u64 = 46;
pub const KERN_OPERATION_TIMED_OUT: u64 = 49;

/// `MACH_SEND_INVALID_DEST`: what a message to a port nobody serves gets.
pub const MACH_SEND_INVALID_DEST: u64 = 0x1000_0003;

/// `mach_absolute_time` and `mach_continuous_time`, which arm64 reaches by
/// trap numbers of their own.
const ABSTIME: u64 = 3;
const CONTTIME: u64 = 4;

/// `VM_FLAGS_ANYWHERE`: the kernel chooses the address.
const VM_FLAGS_ANYWHERE: u64 = 1;
/// `VM_FLAGS_OVERWRITE`: a fixed allocation replaces what is there.
const VM_FLAGS_OVERWRITE: u64 = 0x4000;

/// The names a task's own ports have. XNU hands out names by index and
/// generation; these are the ones a new task has there too, which programs
/// print and compare but never construct.
pub mod port {
    /// `mach_task_self()`.
    pub const TASK: u32 = 0x203;
    /// `mach_host_self()`.
    pub const HOST: u32 = 0xa03;
    /// The first thread's port; thread `n` after it has `THREAD + n * 0x100`.
    pub const THREAD: u32 = 0x103;
    /// Where the names a task allocates start (reply ports, receive rights).
    pub const FIRST_ALLOCATED: u32 = 0x1003;
}

/// The name of thread `tid`'s port in process `pid`: the first thread's is
/// [`port::THREAD`], and each later one's a step further.
pub fn thread_port(pid: i32, tid: i32) -> u32 {
    port::THREAD.wrapping_add((tid - pid).unsigned_abs().wrapping_mul(0x100))
}

/// The thread of process `pid` a port name is for, if it is a thread's.
pub fn port_thread(pid: i32, name: u32) -> Option<i32> {
    let step = name.checked_sub(port::THREAD)?;
    (step % 0x100 == 0 && step / 0x100 < 1 << 20).then(|| pid + (step / 0x100) as i32)
}

fn kern(r: Result<u64, Errno>) -> u64 {
    match r {
        Ok(_) => KERN_SUCCESS,
        Err(Errno::ENOMEM) => KERN_NO_SPACE,
        Err(Errno::EEXIST) => KERN_NO_SPACE,
        Err(Errno::EINVAL) => KERN_INVALID_ARGUMENT,
        Err(Errno::EFAULT) => KERN_INVALID_ADDRESS,
        Err(Errno::EACCES) => KERN_PROTECTION_FAILURE,
        // Not this task's port: what the trap says of a port it cannot use.
        Err(Errno::ESRCH) => MACH_SEND_INVALID_DEST,
        Err(_) => KERN_FAILURE,
    }
}

impl Kernel {
    /// Nanoseconds on the clock `mach_absolute_time` reads. Its timebase is
    /// one to one ([`t::MACH_TIMEBASE_INFO`]), as an Intel Mac's is.
    pub(super) fn darwin_abstime(&self) -> u64 {
        clock_ns(self.clock, 1)
    }

    /// The name of the calling thread's own port.
    pub(super) fn darwin_thread_port(&self) -> u32 {
        thread_port(self.pid, self.tid)
    }

    /// The name of the port of a process's first thread, which XNU tells a
    /// new process in its `apple` strings (`th_port=`).
    pub fn darwin_main_thread_port(&self) -> u32 {
        port::THREAD
    }

    /// Mach trap `n` (its positive number) with arguments `x`.
    pub(super) fn darwin_mach(&mut self, m: &mut GuestMem<'_>, n: u64, x: [u64; 8]) -> SysResult {
        Ok(match n {
            ABSTIME | CONTTIME => self.darwin_abstime(),
            t::TASK_SELF => port::TASK as u64,
            t::HOST_SELF => port::HOST as u64,
            t::THREAD_SELF => self.darwin_thread_port() as u64,
            t::MACH_REPLY_PORT | t::THREAD_GET_SPECIAL_REPLY_PORT => {
                self.darwin_port_names.fetch_add(0x100, std::sync::atomic::Ordering::Relaxed) as u64
            }
            // { numer, denom }: a tick is a nanosecond.
            t::MACH_TIMEBASE_INFO => kern(m.write_u32(x[0], 1).and_then(|_| m.write_u32(x[0] + 4, 1)).map(|_| 0)),
            t::MACH_VM_ALLOCATE => kern(self.darwin_vm_allocate(m, x[0], x[1], x[2], 0, x[3], 3)),
            t::MACH_VM_MAP => kern(self.darwin_vm_allocate(m, x[0], x[1], x[2], x[3], x[4], x[5] as u32 & 7)),
            t::MACH_VM_DEALLOCATE => {
                if x[0] as u32 != port::TASK {
                    MACH_SEND_INVALID_DEST
                } else if x[2] == 0 {
                    KERN_SUCCESS
                } else {
                    let (lo, hi) = (x[1] & !(abi::PAGE_SIZE - 1), (x[1] + x[2]).next_multiple_of(abi::PAGE_SIZE));
                    kern(self.map_io().call(m, nr::MUNMAP, [lo, hi - lo, 0, 0, 0, 0]).unwrap_or(Err(Errno::EINVAL)))
                }
            }
            // (target, address, size, set_maximum, new_protection); a
            // maximum is not kept, so setting one changes nothing.
            t::MACH_VM_PROTECT => {
                if x[0] as u32 != port::TASK {
                    MACH_SEND_INVALID_DEST
                } else if x[3] != 0 {
                    KERN_SUCCESS
                } else {
                    let (lo, hi) = (x[1] & !(abi::PAGE_SIZE - 1), (x[1] + x[2]).next_multiple_of(abi::PAGE_SIZE));
                    kern(self.map_io().call(m, nr::MPROTECT, [lo, hi - lo, x[4] & 7, 0, 0, 0]).unwrap_or(Err(Errno::EINVAL)))
                }
            }
            // The processor is given up for a moment.
            t::SWTCH_PRI | t::SWTCH | t::THREAD_SWITCH => {
                std::thread::yield_now();
                KERN_SUCCESS
            }
            // A message to a port that nothing serves.
            t::MACH_MSG | t::MACH_MSG_OVERWRITE | t::MACH_MSG2 => self.darwin_unimplemented_trap(n, MACH_SEND_INVALID_DEST),
            _ => self.darwin_unimplemented_trap(n, KERN_NOT_SUPPORTED),
        })
    }

    fn darwin_unimplemented_trap(&mut self, n: u64, answer: u64) -> u64 {
        if !self.unimplemented.contains(&(MACH + n)) {
            self.unimplemented.push(MACH + n);
            if self.trace || std::env::var_os("GEX_DARWIN_UNIMPLEMENTED").is_some() {
                eprintln!("[gex:{}] unimplemented Mach trap {n} ({}) -> {answer:#x}", self.pid, t::name(n).unwrap_or("?"));
            }
        }
        answer
    }

    /// `mach_vm_allocate` and `mach_vm_map` of anonymous memory: `size`
    /// bytes of zeroes in `target`, at the address `*addr_ptr` names or,
    /// with `VM_FLAGS_ANYWHERE`, wherever there is room on a boundary of
    /// `mask + 1`; the address goes back through `addr_ptr`.
    #[allow(clippy::too_many_arguments)]
    fn darwin_vm_allocate(&mut self, m: &mut GuestMem<'_>, target: u64, addr_ptr: u64, size: u64, mask: u64, flags: u64, prot_bits: u32) -> Result<u64, Errno> {
        if target as u32 != port::TASK {
            return Err(Errno::ESRCH);
        }
        let page = abi::PAGE_SIZE;
        let want = m.read_u64(addr_ptr)?;
        if size == 0 {
            m.write_u64(addr_ptr, 0)?;
            return Ok(0);
        }
        let len = size.checked_next_multiple_of(page).ok_or(Errno::ENOMEM)?;
        let at = if flags & VM_FLAGS_ANYWHERE != 0 {
            let at = self.darwin_place(want, len, prot_bits, false, mask)?;
            self.mem.mmap_anon(m, at, len, prot_bits, 0, false, "")?
        } else {
            let at = want & !(page - 1);
            let extra = if flags & VM_FLAGS_OVERWRITE != 0 { mflags::FIXED } else { mflags::FIXED_NOREPLACE };
            self.mem.mmap_anon(m, at, len, prot_bits, extra, false, "")?
        };
        m.write_u64(addr_ptr, at)?;
        Ok(0)
    }
}
