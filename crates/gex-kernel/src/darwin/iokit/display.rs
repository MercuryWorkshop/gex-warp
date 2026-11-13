//! The framebuffer: an `IONDRVFramebuffer`'s user client, as IOGraphics
//! (`IOFramebufferUserClient`) and IOKit's IOGraphicsLib speak to it.
//!
//! It has one display mode, the size [`super::registry::mode`] says, 32
//! bits a pixel (`--------RRRRRRRRGGGGGGGGBBBBBBBB`), at 60 Hz. Its pixels
//! are memory mapped into the task that asks (`IOConnectMapMemory` of the
//! system aperture or of `kIOFBVRAMMemory`), which the kernel keeps the
//! address of with the connection ([`super::framebuffer`]); and a shared
//! cursor (`IOFBCreateSharedCursor`, `kIOFBCursorMemory`): a
//! `StdFBShmem_t` and room for the cursor's images.

use super::registry::Mode;

/// `kIOFBCursorMemory`, `kIOFBVRAMMemory`; any other number is an
/// aperture (`kIOFBSystemAperture`, 0, the one there is).
pub const CURSOR_MEMORY: u32 = 100;
pub const VRAM_MEMORY: u32 = 110;
/// `sizeof(StdFBShmem_t)` and `kIOFBCurrentShmemVersion`.
const SHMEM_SIZE: usize = 5360;
const SHMEM_VERSION: i32 = 2;
/// The one mode's id and depth index.
pub const MODE: i32 = 1;

pub struct Fb {
    /// `kIOFBServerConnectType` (0) or `kIOFBSharedConnectType` (1).
    pub kind: u32,
    pub mode: Mode,
    /// The largest cursor `IOFBCreateSharedCursor` asked room for.
    pub cursor: (u32, u32),
    pub cursor_at: (i32, i32),
    pub cursor_visible: bool,
}

impl Fb {
    pub fn new(kind: u32) -> Fb {
        Fb { kind, mode: super::registry::mode(), cursor: (32, 32), cursor_at: (0, 0), cursor_visible: false }
    }

    pub fn row_bytes(&self) -> u64 {
        self.mode.width as u64 * 4
    }

    /// The pixels' size, in whole pages.
    pub fn vram_size(&self) -> u64 {
        (self.row_bytes() * self.mode.height as u64).next_multiple_of(0x4000)
    }

    /// What memory of `kind` begins as: zeroed pixels, or the shared cursor
    /// as `IOFramebuffer::setupCursor` leaves it -- its version and size,
    /// the screen's bounds, the cursor hidden.
    pub fn memory(&self, kind: u32) -> Option<Vec<u8>> {
        match kind {
            CURSOR_MEMORY => {
                let (w, h) = self.cursor;
                // (Four frames of a cursor of the largest size, 32-bit.)
                let size = (SHMEM_SIZE + (w * h * 4 * 4) as usize).next_multiple_of(0x1000);
                let mut b = vec![0u8; size];
                b[56..60].copy_from_slice(&SHMEM_VERSION.to_le_bytes());
                b[60..64].copy_from_slice(&(SHMEM_SIZE as i32).to_le_bytes());
                // screenBounds: IOGBounds (minx, maxx, miny, maxy)
                let bounds = [0i16, self.mode.width as i16, 0, self.mode.height as i16];
                for (i, v) in bounds.iter().enumerate() {
                    b[48 + 2 * i..50 + 2 * i].copy_from_slice(&v.to_le_bytes());
                }
                Some(b)
            }
            _ => Some(vec![0u8; self.vram_size() as usize]),
        }
    }

    /// `IOPixelInformation` for the mode.
    fn pixel_information(&self) -> Vec<u8> {
        let mut b = vec![0u8; 172];
        let mut put = |at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(0, self.row_bytes() as u32);
        put(8, 32);
        put(12, 2); // kIORGBDirectPixels
        put(16, 3);
        put(20, 8);
        put(24, 0x00ff_0000);
        put(28, 0x0000_ff00);
        put(32, 0x0000_00ff);
        put(156, self.mode.width);
        put(160, self.mode.height);
        b[88..120].copy_from_slice(b"--------RRRRRRRRGGGGGGGGBBBBBBBB");
        b
    }

    /// `IODisplayModeInformation` for the mode: its size, 60 Hz, one depth,
    /// valid, safe and the default; a display about 34 cm across.
    fn mode_information(&self) -> Vec<u8> {
        let mut b = vec![0u8; 36];
        b[0..4].copy_from_slice(&self.mode.width.to_le_bytes());
        b[4..8].copy_from_slice(&self.mode.height.to_le_bytes());
        b[8..12].copy_from_slice(&(60u32 << 16).to_le_bytes());
        b[16..20].copy_from_slice(&(0x1u32 | 0x2 | 0x4).to_le_bytes());
        b[20..22].copy_from_slice(&340u16.to_le_bytes());
        b[22..24].copy_from_slice(&((340 * self.mode.height / self.mode.width.max(1)) as u16).to_le_bytes());
        b
    }

    /// `IOFramebufferUserClient`'s methods (the shared client's are the
    /// same few that read).
    pub fn method(&mut self, selector: u32, scalars: &[u64], inband: &[u8]) -> Result<(Vec<u64>, Vec<u8>), i32> {
        let none = Ok((Vec::new(), Vec::new()));
        let arg = |i: usize| scalars.get(i).copied().unwrap_or(0);
        match selector {
            // extCreateSharedCursor(version, max width, max height)
            0 => {
                self.cursor = ((arg(1) as u32).clamp(16, 256), (arg(2) as u32).clamp(16, 256));
                none
            }
            // extGetPixelInformation(mode, depth, aperture)
            1 if arg(0) as i32 == MODE && arg(1) == 0 => Ok((Vec::new(), self.pixel_information())),
            1 => Err(super::BAD_ARGUMENT),
            // extGetCurrentDisplayMode: the mode and its depth
            2 => Ok((vec![MODE as u64, 0], Vec::new())),
            // extSetStartupDisplayMode, extSetDisplayMode: the one there is
            3 | 4 if arg(0) as i32 == MODE => none,
            3 | 4 => Err(super::BAD_ARGUMENT),
            // extGetInformationForDisplayMode(mode)
            5 if arg(0) as i32 == MODE => Ok((Vec::new(), self.mode_information())),
            5 => Err(super::BAD_ARGUMENT),
            // extGetDisplayModeCount, extGetDisplayModes
            6 => Ok((vec![1], Vec::new())),
            7 => Ok((Vec::new(), MODE.to_le_bytes().to_vec())),
            // extGetVRAMMapOffset(aperture): the pixels begin the memory
            8 => Ok((vec![0], Vec::new())),
            // extSetNewCursor, extSetCursorVisible, extSetCursorPosition
            12 => {
                self.cursor_visible = arg(0) != 0;
                none
            }
            13 => {
                self.cursor_at = (arg(0) as i32, arg(1) as i32);
                none
            }
            // extSetBounds, gamma, acknowledging a notification, colour
            // tables: taken
            9 | 10 | 11 | 14 | 15 | 16 | 19 | 20 => none,
            // extValidateDetailedTiming: no timing but the mode's own
            17 => Err(super::UNSUPPORTED),
            // extGetAttribute(attribute): none of them is set
            18 => {
                let _ = inband;
                Ok((vec![0], Vec::new()))
            }
            _ => Err(super::BAD_ARGUMENT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_mode_of_32_bit_pixels() {
        let mut fb = Fb { kind: 0, mode: Mode { width: 800, height: 600 }, cursor: (32, 32), cursor_at: (0, 0), cursor_visible: false };
        assert_eq!(fb.method(6, &[], &[]), Ok((vec![1], Vec::new())));
        assert_eq!(fb.method(2, &[], &[]), Ok((vec![1, 0], Vec::new())));
        let (_, info) = fb.method(1, &[1, 0, 0], &[]).unwrap();
        assert_eq!(u32::from_le_bytes(info[0..4].try_into().unwrap()), 3200);
        assert_eq!(u32::from_le_bytes(info[156..160].try_into().unwrap()), 800);
        let (_, mode) = fb.method(5, &[1], &[]).unwrap();
        assert_eq!(u32::from_le_bytes(mode[4..8].try_into().unwrap()), 600);
        assert_eq!(fb.vram_size(), (3200 * 600u64).next_multiple_of(0x4000));
        let cursor = fb.memory(CURSOR_MEMORY).unwrap();
        assert_eq!(i32::from_le_bytes(cursor[56..60].try_into().unwrap()), 2);
    }
}
