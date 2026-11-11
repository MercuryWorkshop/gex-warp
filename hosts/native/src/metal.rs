//! `/dev/gex-metal`: a guest's way to a Metal implementation on the host.
//!
//! A guest's Metal.framework (mink's, in a pyrus root) does not draw: it
//! turns what the program asks into a stream of messages for a replayer
//! that runs on a real GPU, and reads the replayer's answers (the pixels a
//! program reads back, the completion of each command buffer) as they come.
//! This device is that channel and nothing more: each open starts the
//! replayer the host was told about (`gexc run --metal PATH`, or
//! `GEX_METAL_HOST` in gexc's own environment) as a child process;
//! what the guest writes goes to the child's standard input and what the
//! child prints is what the guest reads. gex-warp knows nothing of the
//! messages and has no build dependency on the replayer.
//!
//! Unlike `/dev/gex-gl` (one batch in, one reply, per ioctl) the two
//! directions are independent, because a command buffer completes when the
//! GPU has finished it, not when the guest next calls: the framework reads
//! on a thread of its own while other threads write.
//!
//! - `read` blocks while there is nothing, without the kernel lock: it
//!   answers `EAGAIN` and says through `poll_readable` when that would be
//!   wrong, which is how a stream parks here (the kernel does the rest, and
//!   gives a non-blocking descriptor its `EAGAIN`). The thread that reads the
//!   child wakes the sleepers.
//! - `write` never waits for the child: the bytes go to a queue that a
//!   thread empties into the child's pipe. (A write that blocked would hold
//!   the process's kernel lock while the child, its own output pipe full,
//!   waited for a reader that needs that lock.)
//! - The last close kills the child. End of file reads as 0 once the child
//!   has gone and its output is drained.

use gex_kernel::errno::{Errno, SysResult};
use gex_kernel::vfs::{FileHandle, FileType, Metadata};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};

/// What the child has printed and the guest has not yet read.
#[derive(Debug, Default)]
struct Output {
    bytes: VecDeque<u8>,
    /// The child closed its output (it exited), or could not be started.
    ended: bool,
}

#[derive(Debug)]
struct Running {
    child: Child,
    /// To the thread that writes the child's input. Dropped to close it.
    input: Sender<Vec<u8>>,
}

#[derive(Debug)]
pub struct MetalDevice {
    helper: PathBuf,
    running: Mutex<Option<Running>>,
    output: Arc<Mutex<Output>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl MetalDevice {
    /// A handle for one open. The child is started by the first read or
    /// write, not here: a `stat` of the node makes a handle too.
    pub fn new(helper: PathBuf) -> Arc<MetalDevice> {
        Arc::new(MetalDevice { helper, running: Mutex::new(None), output: Arc::new(Mutex::new(Output::default())) })
    }

    fn start(&self) -> Result<(), Errno> {
        let mut running = lock(&self.running);
        if running.is_some() {
            return Ok(());
        }
        if lock(&self.output).ended {
            return Err(Errno::EIO);
        }
        let spawned = Command::new(&self.helper).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                eprintln!("gexc: /dev/gex-metal: cannot start {}: {e}", self.helper.display());
                lock(&self.output).ended = true;
                crate::ready::notify();
                return Err(Errno::EIO);
            }
        };
        let (mut stdin, mut stdout) = (child.stdin.take().expect("piped"), child.stdout.take().expect("piped"));
        let (input, queued) = channel::<Vec<u8>>();
        let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| std::thread::Builder::new().name(name.into()).spawn(f).map_err(|_| Errno::EAGAIN);
        spawn(
            "gex-metal-in",
            Box::new(move || {
                // Ends when the device is dropped (the sender goes) or the child stops reading.
                for bytes in queued {
                    if stdin.write_all(&bytes).is_err() {
                        break;
                    }
                }
            }),
        )?;
        let output = Arc::clone(&self.output);
        spawn(
            "gex-metal-out",
            Box::new(move || {
                let mut buf = vec![0u8; 1 << 16];
                loop {
                    let n = match stdout.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    lock(&output).bytes.extend(&buf[..n]);
                    crate::ready::notify();
                }
                lock(&output).ended = true;
                crate::ready::notify();
            }),
        )?;
        *running = Some(Running { child, input });
        Ok(())
    }
}

impl FileHandle for MetalDevice {
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> SysResult {
        self.start()?;
        let mut out = lock(&self.output);
        if out.bytes.is_empty() {
            return if out.ended { Ok(0) } else { Err(Errno::EAGAIN) };
        }
        let n = buf.len().min(out.bytes.len());
        for (d, s) in buf.iter_mut().zip(out.bytes.drain(..n)) {
            *d = s;
        }
        Ok(n as u64)
    }

    fn write_at(&self, _offset: u64, buf: &[u8]) -> SysResult {
        self.start()?;
        let running = lock(&self.running);
        match running.as_ref().map(|r| r.input.send(buf.to_vec())) {
            Some(Ok(())) => Ok(buf.len() as u64),
            // The child stopped reading: it has gone.
            _ => Err(Errno::EPIPE),
        }
    }

    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { file_type: FileType::CharDevice, size: 0, mode: 0o666, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }

    fn is_stream(&self) -> bool {
        true
    }

    fn poll_readable(&self) -> bool {
        let out = lock(&self.output);
        !out.bytes.is_empty() || out.ended
    }

    fn available(&self) -> Option<u64> {
        Some(lock(&self.output).bytes.len() as u64)
    }

    fn poll_hup(&self) -> bool {
        let out = lock(&self.output);
        out.bytes.is_empty() && out.ended
    }
}

impl Drop for MetalDevice {
    fn drop(&mut self) {
        if let Some(mut r) = lock(&self.running).take() {
            // Closing its input is the polite way; then make sure.
            drop(r.input);
            let _ = r.child.kill();
            let _ = r.child.wait();
        }
    }
}
