//! `AppleGPUWrangler`: the service that tells who asks (CoreDisplay, in
//! WindowServer, through the GPUWrangler framework) which GPUs the Mac has
//! and which framebuffers each drives. CoreDisplay waits for it before it
//! starts its displays (`GPUWranglerOpen` looks it up until it is there).
//!
//! Its user client has three methods GPUWrangler asks:
//!
//! - `0xA0000001`: the framework's version in, the driver's out (4 bytes);
//!   the same version back is a driver that speaks it.
//! - `0xA0000002`: the GPUs, 9148 bytes: their number at 0, and from 60
//!   up to sixteen records of 568 bytes each: the GPU's id (16 bits), its
//!   flags at 4 (`0x10` integrated, `0x20` discrete, `2` removable, `1`
//!   ejected), the registry ids of its accelerator at 8 and its PCI device
//!   at 16, how many framebuffers it has at 24, and from 32 a pair of words
//!   for each, the framebuffer's registry id and its accelerator's index.
//! - `0xA0000004`: wait until the GPUs are published, which they are.
//!
//! gex's machine has one GPU, the PCI display, integrated, driving its one
//! framebuffer.

/// The registry ids [`method`] tells of: the GPU's PCI device and its
/// framebuffer.
pub struct Gpu {
    pub device: u64,
    pub framebuffer: u64,
}

const GPUS: usize = 9148;
const FIRST: usize = 60;

/// Method `selector` with structure `input`: the scalars and the structure
/// it answers, or an `IOReturn`.
pub fn method(selector: u32, input: &[u8], gpu: &Gpu) -> Result<(Vec<u64>, Vec<u8>), i32> {
    match selector {
        0xA000_0001 => Ok((Vec::new(), input.get(..4).unwrap_or(&[0; 4]).to_vec())),
        0xA000_0002 => {
            let mut b = vec![0u8; GPUS];
            b[0..4].copy_from_slice(&1u32.to_le_bytes());
            let r = &mut b[FIRST..];
            r[0..2].copy_from_slice(&1u16.to_le_bytes());
            r[4..8].copy_from_slice(&0x10u32.to_le_bytes());
            r[8..16].copy_from_slice(&gpu.device.to_le_bytes());
            r[16..24].copy_from_slice(&gpu.device.to_le_bytes());
            r[24..32].copy_from_slice(&1u64.to_le_bytes());
            r[32..40].copy_from_slice(&gpu.framebuffer.to_le_bytes());
            r[40..48].copy_from_slice(&0u64.to_le_bytes());
            Ok((Vec::new(), b))
        }
        0xA000_0004 => Ok((Vec::new(), Vec::new())),
        _ => Err(super::UNSUPPORTED),
    }
}
