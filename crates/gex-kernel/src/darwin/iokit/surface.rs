//! `IOSurfaceRoot`: surfaces, memory of pixels named by a number that any
//! task may map and draw into, as IOSurface.framework asks for them
//! (`IOSurfaceRootUserClient`'s methods, IOSurface-255 of macOS 10.14).
//!
//! A surface is made from a dictionary of its properties (its size, its
//! pixel format, how many bytes a row and an element have, its planes), as
//! the driver computes them from what is given and what is not. Its memory
//! is the kernel's own shared memory (a `memfd`, as a named memory entry's
//! is, [`super::super::mement`]): each task that creates or looks up a
//! surface maps that object shared, so every task sees the same pixels, and
//! gex can read them without any task ([`pixels`]).
//!
//! After the pixels, on a page of their own, is what the driver shares with
//! the framework of each surface: a read-only part (`IOSurfaceSharedRO`:
//! its registry id, the seed, its use count, the generation of its values,
//! its matrix and flags, its attachments) and a writable part
//! (`IOSurfaceSharedRW`: indexed timestamps, the detach-mode code). The
//! lock result tells the framework where both are in its task.
//!
//! What a task opens is a connection with a table of the surfaces it has
//! (by id): each created, looked up, or received by port
//! (`IOSurfaceCreateMachPort` makes a send right a task can pass on).
//! `IOSurfaceLookup` finds a surface its own task made, one it has, or a
//! global one, as the driver allows.

use super::plist::{self, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// `sizeof(IOSurfaceLockResult)`: the header and 32 planes of 68 bytes.
pub const LOCK_RESULT: usize = 2256;
const PLANE: usize = 68;
const PLANES_AT: usize = 80;
/// Where the shared parts are on the page after the pixels.
const RO: u64 = 0;
const RW: u64 = 0x200;
/// `IOSurfaceSharedRO` fields.
const RO_SEED: u64 = 12;
const RO_USE_COUNT: u64 = 20;
const RO_GENERATION: u64 = 24;
const RO_YCBCR: u64 = 28;
const RO_VALUES_SIZE: u64 = 32;
const RO_CASTING: u64 = 36;
const RO_DISPLAYABLE: u64 = 37;
const RO_SYSMEM_ONLY: u64 = 38;
const RO_ATTACHMENTS: u64 = 48;
/// `IOSurfaceSharedRW`: the detach-mode code.
const RW_DETACH: u64 = 32;
/// The alignment masks the root tells (`get_limits`): rows and allocations
/// on a cache line.
pub const ROW_MASK: u32 = 63;
pub const ALLOC_MASK: u32 = 63;

/// `kIOReturnNoSpace`, `kIOReturnNoMemory`.
pub const NO_SPACE: i32 = 0xe000_02db_u32 as i32;
pub const NO_MEMORY: i32 = 0xe000_02bd_u32 as i32;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plane {
    pub width: u32,
    pub height: u32,
    pub base: u32,
    pub offset: u32,
    pub bytes_per_row: u32,
    pub size: u32,
    pub bytes_per_element: u16,
    pub element_width: u8,
    pub element_height: u8,
}

/// A surface's shape, as the driver computes it from its properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: u32,
    pub bytes_per_element: u16,
    pub element_width: u8,
    pub element_height: u8,
    pub offset: u32,
    pub pixel_format: u32,
    pub alloc_size: u32,
    pub ycbcr: u32,
    pub cache_mode: u32,
    pub subsampling: u8,
    pub planes: Vec<Plane>,
    pub global: bool,
    pub displayable: bool,
    pub sysmem_only: bool,
    pub casting: bool,
}

fn num(d: &Value, key: &str) -> Option<u64> {
    match d.get(key)? {
        Value::Num(n, _) => Some(*n),
        _ => None,
    }
}

fn flag(d: &Value, key: &str) -> Option<bool> {
    match d.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::Num(n, _) => Some(*n != 0),
        _ => None,
    }
}

fn u32_of(d: &Value, key: &str) -> Result<Option<u32>, ()> {
    match num(d, key) {
        Some(n) if n > u32::MAX as u64 => Err(()),
        Some(n) => Ok(Some(n as u32)),
        None => Ok(None),
    }
}

fn align(v: u64, mask: u32) -> u64 {
    (v + mask as u64) & !(mask as u64)
}

impl Shape {
    /// What `IOSurface::parse_properties` makes of a creation dictionary,
    /// or `None` where it refuses one.
    pub fn parse(d: &Value) -> Option<Shape> {
        let mut s = Shape { bytes_per_element: 1, element_width: 1, element_height: 1, ..Shape::default() };
        let legacy = flag(d, "IOSurfaceCoreSurfaceLegacy").unwrap_or(false);
        s.displayable = flag(d, "IsDisplayable").unwrap_or(false);
        s.sysmem_only = flag(d, "IOSurfaceSysMemOnly").unwrap_or(false);
        s.casting = flag(d, "IOSurfacePixelSizeCastingAllowed").unwrap_or(false);
        s.global = flag(d, "IOSurfaceIsGlobal").unwrap_or(false);
        s.subsampling = num(d, "IOSurfaceSubsampling").unwrap_or(0) as u8;
        s.pixel_format = num(d, "IOSurfacePixelFormat").unwrap_or(0) as u32;
        if let Some(Value::Array(planes)) = d.get("IOSurfacePlaneInfo") {
            if planes.len() > 32 {
                return None;
            }
            for (i, p) in planes.iter().enumerate() {
                let mut pl = Plane { element_width: 1, element_height: 1, ..Plane::default() };
                pl.width = u32_of(p, "IOSurfacePlaneWidth").ok()??;
                pl.height = u32_of(p, "IOSurfacePlaneHeight").ok()??;
                let bpe = num(p, "IOSurfacePlaneBytesPerElement").or_else(|| num(p, "IOSurfacePlaneBitsPerElement").or_else(|| num(p, "IOSurfacePlaneBitsPerBlock")).map(|b| b / 8)).unwrap_or(1);
                if bpe == 0 || bpe > 0x1fff {
                    return None;
                }
                pl.bytes_per_element = bpe as u16;
                pl.element_width = num(p, "IOSurfacePlaneElementWidth").unwrap_or(1).min(255) as u8;
                pl.element_height = num(p, "IOSurfacePlaneElementHeight").unwrap_or(1).min(255) as u8;
                if pl.element_width == 0 || pl.element_height == 0 {
                    return None;
                }
                let across = (pl.width as u64).div_ceil(pl.element_width as u64);
                let min_row = across * bpe;
                pl.bytes_per_row = match u32_of(p, "IOSurfacePlaneBytesPerRow").ok()? {
                    Some(r) => r,
                    None => u32::try_from(align(min_row, ROW_MASK)).ok()?,
                };
                if min_row > pl.bytes_per_row as u64 {
                    return None;
                }
                let rows = (pl.height as u64).div_ceil(pl.element_height as u64);
                pl.size = match u32_of(p, "IOSurfacePlaneSize").ok()? {
                    Some(n) => n,
                    None => u32::try_from(rows * pl.bytes_per_row as u64).ok()?,
                };
                if pl.size == 0 {
                    return None;
                }
                pl.offset = match u32_of(p, "IOSurfacePlaneOffset").ok()? {
                    Some(o) => o,
                    None if i == 0 => 0,
                    None => {
                        let last = &s.planes[i - 1];
                        u32::try_from(align(last.base as u64 + last.size as u64, ALLOC_MASK)).ok()?
                    }
                };
                pl.base = u32_of(p, "IOSurfacePlaneBase").ok()?.unwrap_or(pl.offset);
                if pl.offset < pl.base || s.planes.iter().any(|q| (pl.base as u64) < q.base as u64 + q.size as u64) {
                    return None;
                }
                s.planes.push(pl);
            }
        }
        if let Some(b) = num(d, "IOSurfaceBitsPerBlock") {
            s.bytes_per_element = u16::try_from(b / 8).ok()?;
        }
        match num(d, "IOSurfaceBytesPerElement") {
            Some(b) => s.bytes_per_element = u16::try_from(b).ok()?,
            None if legacy => {
                s.bytes_per_element = match &s.pixel_format.to_be_bytes() {
                    b"BGRA" | b"RGBA" | b"ARGB" | b"ABGR" => 4,
                    b"RGhA" => 8,
                    b"RGfA" => 16,
                    _ => 1,
                }
            }
            None => {}
        }
        match u32_of(d, "IOSurfaceWidth").ok()? {
            Some(w) => s.width = w,
            None if !s.planes.is_empty() => return None,
            None => {}
        }
        match u32_of(d, "IOSurfaceHeight").ok()? {
            Some(h) => s.height = h,
            None if !s.planes.is_empty() => return None,
            None => {}
        }
        if let Some(w) = num(d, "IOSurfaceElementWidth") {
            s.element_width = u8::try_from(w).ok()?;
        }
        if let Some(h) = num(d, "IOSurfaceElementHeight") {
            s.element_height = u8::try_from(h).ok()?;
        }
        if s.bytes_per_element == 0 || s.element_width == 0 || s.element_height == 0 {
            return None;
        }
        let across = (s.width as u64).div_ceil(s.element_width as u64);
        let min_row = across * s.bytes_per_element as u64;
        s.bytes_per_row = match u32_of(d, "IOSurfaceBytesPerRow").ok()? {
            Some(r) => r,
            None => u32::try_from(align(min_row, ROW_MASK)).ok()?,
        };
        if min_row > s.bytes_per_row as u64 {
            return None;
        }
        s.offset = u32_of(d, "IOSurfaceOffset").ok()?.unwrap_or(0);
        s.alloc_size = match u32_of(d, "IOSurfaceAllocSize").ok()? {
            Some(n) => n,
            None => {
                let end = match s.planes.last() {
                    Some(p) => p.base as u64 + p.size as u64,
                    None => s.offset as u64 + s.bytes_per_row as u64 * s.height as u64,
                };
                u32::try_from(align(end, ALLOC_MASK)).ok()?
            }
        };
        if s.alloc_size == 0 {
            return None;
        }
        let need = match s.planes.last() {
            Some(p) => p.base as u64 + p.size as u64,
            None if s.height == 0 => 0,
            None => s.offset as u64 + s.bytes_per_row as u64 * (s.height as u64 - 1) + min_row,
        };
        if need > s.alloc_size as u64 {
            return None;
        }
        s.ycbcr = num(d, "IOSurfaceYCbCrMatrix").unwrap_or(0) as u32;
        s.cache_mode = num(d, "IOSurfaceCacheMode").unwrap_or(0) as u32 & 0x700;
        Some(s)
    }

    /// The pixels' memory: the allocation in whole pages.
    pub fn pixel_bytes(&self, page: u64) -> u64 {
        (self.alloc_size as u64).next_multiple_of(page).max(page)
    }
}

/// A surface: its shape, its memory, its values.
pub struct Surface {
    pub id: u32,
    pub shape: Shape,
    pub file: Arc<crate::file::OpenFile>,
    /// Bytes of pixels (whole pages), and the page of shared parts after.
    pub pixels: u64,
    pub total: u64,
    /// The task that made it.
    pub creator: i32,
    /// Its values (`IOSurfaceSetValue`): the creation properties first.
    pub values: Vec<(String, Value)>,
    pub generation: u32,
    pub seed: u32,
    pub use_count: u32,
    /// How many tasks' tables have it (and send rights made of it): it
    /// goes when none has.
    pub holders: u32,
    pub attachments: Vec<u8>,
    /// The registry id the shared part tells.
    pub registry_id: u64,
    /// Where a task has it mapped (its creator's, or the first's): what
    /// [`pixels`] reads, since what a task draws is in its pages and the
    /// object's only once written back, which nothing does.
    pub live: Option<(Arc<crate::proc::PeerVm>, u64)>,
}

impl Surface {
    pub fn values_bytes(&self) -> Vec<u8> {
        plist::to_binary(&Value::Dict(self.values.clone()))
    }

    /// The read-only shared part as it is now.
    pub fn shared_ro(&self) -> Vec<u8> {
        let mut b = vec![0u8; 0x200];
        b[0..8].copy_from_slice(&self.registry_id.to_le_bytes());
        b[RO_SEED as usize..][..4].copy_from_slice(&self.seed.to_le_bytes());
        b[RO_USE_COUNT as usize..][..4].copy_from_slice(&self.use_count.to_le_bytes());
        b[RO_GENERATION as usize..][..4].copy_from_slice(&self.generation.to_le_bytes());
        b[RO_YCBCR as usize..][..4].copy_from_slice(&self.shape.ycbcr.to_le_bytes());
        b[RO_VALUES_SIZE as usize..][..4].copy_from_slice(&(self.values_bytes().len() as u32).to_le_bytes());
        b[RO_CASTING as usize] = self.shape.casting as u8;
        b[RO_DISPLAYABLE as usize] = self.shape.displayable as u8;
        b[RO_SYSMEM_ONLY as usize] = self.shape.sysmem_only as u8;
        let n = self.attachments.len().min(0x200 - RO_ATTACHMENTS as usize);
        b[RO_ATTACHMENTS as usize..][..n].copy_from_slice(&self.attachments[..n]);
        b
    }
}

/// The surfaces, by id; and the next id.
pub struct Surfaces {
    pub all: HashMap<u32, Surface>,
    next: u32,
}

pub static SURFACES: Mutex<Option<Surfaces>> = Mutex::new(None);

pub fn table() -> std::sync::MutexGuard<'static, Option<Surfaces>> {
    SURFACES.lock().unwrap_or_else(|e| e.into_inner())
}

impl Surfaces {
    pub fn get() -> Surfaces {
        Surfaces { all: HashMap::new(), next: 1 }
    }

    pub fn new_id(&mut self) -> u32 {
        while self.all.contains_key(&self.next) || self.next == 0 {
            self.next = self.next.wrapping_add(1);
        }
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        id
    }

    /// One fewer holder of `id`: the surface goes with the last.
    pub fn let_go(&mut self, id: u32) {
        if let Some(s) = self.all.get_mut(&id) {
            s.holders = s.holders.saturating_sub(1);
            if s.holders == 0 {
                self.all.remove(&id);
            }
        }
    }
}

/// A surface in a task's table: where its memory is mapped there, and how
/// many uses this task counts of it.
#[derive(Clone, Debug)]
pub struct Held {
    pub at: u64,
    pub uses: u32,
    pub created: bool,
}

/// A connection to `IOSurfaceRoot`: the surfaces its task has.
#[derive(Default)]
pub struct Client {
    pub held: HashMap<u32, Held>,
    /// The graphics comm page, once mapped.
    pub comm_page: u64,
    pub next_event: u64,
}

/// `IOSurfaceLockResult` for a surface mapped at `at`.
pub fn lock_result(s: &Surface, at: u64) -> Vec<u8> {
    let mut b = vec![0u8; LOCK_RESULT];
    let sh = &s.shape;
    let put32 = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    b[0..8].copy_from_slice(&at.to_le_bytes());
    b[8..16].copy_from_slice(&(at + s.pixels + RO).to_le_bytes());
    b[16..24].copy_from_slice(&(at + s.pixels + RW).to_le_bytes());
    put32(&mut b, 24, s.id);
    put32(&mut b, 28, sh.alloc_size);
    put32(&mut b, 32, sh.width);
    put32(&mut b, 36, sh.height);
    put32(&mut b, 40, sh.bytes_per_row);
    put32(&mut b, 44, sh.offset);
    put32(&mut b, 48, sh.pixel_format);
    put32(&mut b, 52, sh.alloc_size);
    put32(&mut b, 56, sh.planes.len() as u32);
    put32(&mut b, 60, sh.ycbcr);
    b[64..66].copy_from_slice(&sh.bytes_per_element.to_le_bytes());
    b[66] = sh.element_width;
    b[67] = sh.element_height;
    put32(&mut b, 68, sh.cache_mode);
    put32(&mut b, 72, s.seed);
    b[76] = sh.subsampling;
    for (i, p) in sh.planes.iter().enumerate() {
        let o = PLANES_AT + PLANE * i;
        put32(&mut b, o, p.width);
        put32(&mut b, o + 4, p.height);
        put32(&mut b, o + 8, p.base);
        put32(&mut b, o + 12, p.offset);
        put32(&mut b, o + 16, p.bytes_per_row);
        put32(&mut b, o + 20, p.size);
        b[o + 24..o + 26].copy_from_slice(&p.bytes_per_element.to_le_bytes());
        b[o + 26] = p.element_width;
        b[o + 27] = p.element_height;
    }
    b
}

/// A surface's pixels as they are now, with its shape: what gex reads of
/// one without any task.
pub fn pixels(id: u32) -> Option<(Shape, Vec<u8>)> {
    let t = table();
    let s = t.as_ref()?.all.get(&id)?;
    let mut buf = vec![0u8; s.shape.alloc_size as usize];
    if s.live.as_ref().is_some_and(|(vm, at)| vm.mem.read(*at, &mut buf)) {
        return Some((s.shape.clone(), buf));
    }
    let n = s.file.handle.read_at(0, &mut buf).ok()? as usize;
    buf.truncate(n);
    Some((s.shape.clone(), buf))
}

/// The ids of the surfaces there are now, and their shapes.
pub fn list() -> Vec<(u32, Shape)> {
    let t = table();
    let mut v: Vec<(u32, Shape)> = t.as_ref().map(|t| t.all.values().map(|s| (s.id, s.shape.clone())).collect()).unwrap_or_default();
    v.sort_by_key(|(id, _)| *id);
    v
}

/// The limits `get_limits` tells: the alignments of allocations and rows,
/// no largest width, height or size, no protected access, and the
/// framework's version (1), which it checks.
pub fn limits() -> Vec<u8> {
    let mut b = vec![0u8; 24];
    b[0..4].copy_from_slice(&ALLOC_MASK.to_le_bytes());
    b[4..8].copy_from_slice(&ROW_MASK.to_le_bytes());
    for i in 0..3 {
        b[8 + 4 * i..12 + 4 * i].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
    }
    b[22..24].copy_from_slice(&1u16.to_le_bytes());
    b
}

/// The methods' names, for the trace.
pub fn method_name(selector: u32) -> &'static str {
    const NAMES: [&str; 39] = [
        "create_surface",
        "release_surface",
        "lock_surface",
        "unlock_surface",
        "lookup_surface",
        "set_ycbcrmatrix",
        "create_surface_fast_path",
        "create_surface_client_mem",
        "get_ycbcrmatrix",
        "set_value",
        "get_value",
        "remove_value",
        "bind_accel",
        "get_limits",
        "increment_surface_use_count",
        "decrement_surface_use_count",
        "get_surface_use_count",
        "set_surface_notify",
        "remove_surface_notify",
        "log",
        "set_purgeable",
        "get_data_value",
        "set_bulk_attachments",
        "get_bulk_attachments",
        "gather_iosurface_data",
        "get_graphics_comm_page_address",
        "set_indexed_timestamp",
        "lookup_surface_from_port",
        "create_port_from_surface",
        "create_shared_event",
        "signal_shared_event",
        "query_shared_event",
        "notify_shared_event",
        "add_shared_event_notification_port",
        "remove_shared_event_notification_port",
        "signal_shared_event_operation",
        "set_gpu_policy_dict",
        "get_pid_gpu_policy_dict",
        "set_detach_mode_code",
    ];
    NAMES.get(selector as usize).copied().unwrap_or("?")
}

/// Where a field of the shared parts is, from a surface's mapping.
pub fn ro_at(s: &Surface, at: u64, field: u64) -> u64 {
    at + s.pixels + RO + field
}

pub fn rw_at(s: &Surface, at: u64, field: u64) -> u64 {
    at + s.pixels + RW + field
}

pub const SEED: u64 = RO_SEED;
pub const USE_COUNT: u64 = RO_USE_COUNT;
pub const GENERATION: u64 = RO_GENERATION;
pub const YCBCR: u64 = RO_YCBCR;
pub const VALUES_SIZE: u64 = RO_VALUES_SIZE;
pub const ATTACHMENTS: u64 = RO_ATTACHMENTS;
pub const DETACH: u64 = RW_DETACH;

#[cfg(test)]
mod tests {
    use super::*;

    fn d(items: Vec<(&str, u64)>) -> Value {
        Value::Dict(items.into_iter().map(|(k, v)| (k.to_string(), Value::n32(v))).collect())
    }

    #[test]
    fn a_surface_s_rows_and_size_are_computed_as_the_driver_does() {
        let s = Shape::parse(&d(vec![("IOSurfaceWidth", 100), ("IOSurfaceHeight", 10), ("IOSurfaceBytesPerElement", 4), ("IOSurfacePixelFormat", u32::from_be_bytes(*b"BGRA") as u64)])).unwrap();
        assert_eq!(s.bytes_per_row, 448); // 400 to a cache line
        assert_eq!(s.alloc_size, 4480);
        let s = Shape::parse(&d(vec![("IOSurfaceWidth", 100), ("IOSurfaceHeight", 10), ("IOSurfaceBytesPerElement", 4), ("IOSurfaceBytesPerRow", 400)])).unwrap();
        assert_eq!(s.bytes_per_row, 400);
        // Rows too short, or no room for the pixels: refused.
        assert!(Shape::parse(&d(vec![("IOSurfaceWidth", 100), ("IOSurfaceHeight", 10), ("IOSurfaceBytesPerElement", 4), ("IOSurfaceBytesPerRow", 399)])).is_none());
        assert!(Shape::parse(&d(vec![("IOSurfaceWidth", 100), ("IOSurfaceHeight", 10), ("IOSurfaceBytesPerElement", 4), ("IOSurfaceAllocSize", 100)])).is_none());
    }

    #[test]
    fn planes_follow_one_another() {
        let plane = |w: u64, h: u64, bpe: u64| d(vec![("IOSurfacePlaneWidth", w), ("IOSurfacePlaneHeight", h), ("IOSurfacePlaneBytesPerElement", bpe)]);
        let dict = Value::Dict(vec![
            ("IOSurfaceWidth".to_string(), Value::n32(64)),
            ("IOSurfaceHeight".to_string(), Value::n32(64)),
            ("IOSurfacePlaneInfo".to_string(), Value::Array(vec![plane(64, 64, 1), plane(32, 32, 2)])),
        ]);
        let s = Shape::parse(&dict).unwrap();
        assert_eq!(s.planes.len(), 2);
        assert_eq!(s.planes[1].offset, 4096);
        assert_eq!(s.alloc_size, 4096 + 2048);
        let r = lock_result(&Surface { id: 7, shape: s, file: crate::file::OpenFile::new(Arc::new(crate::unix::MemFd::new("t")), "/memfd:t".into(), 2, true, true), pixels: 0x4000, total: 0x8000, creator: 1, values: Vec::new(), generation: 1, seed: 0, use_count: 0, holders: 1, attachments: Vec::new(), registry_id: 0, live: None }, 0x10000);
        assert_eq!(r.len(), LOCK_RESULT);
        assert_eq!(u32::from_le_bytes(r[24..28].try_into().unwrap()), 7);
        assert_eq!(u64::from_le_bytes(r[8..16].try_into().unwrap()), 0x14000);
        assert_eq!(u32::from_le_bytes(r[56..60].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(r[80 + 68 + 12..80 + 68 + 16].try_into().unwrap()), 4096);
    }
}
