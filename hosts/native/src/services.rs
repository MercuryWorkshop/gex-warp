//! `/dev/gex-services`: the services a host's shell provides to guest
//! programs -- notifications first; a program's location, Bluetooth and
//! others are to follow -- natively, with a stand-in shell for each service
//! a test asks for.
//!
//! The channel is one device node of the kernel's, for every service and
//! every guest: each open is one connection to one service, a byte stream
//! each way, independent (a program reads its events on a thread of its own
//! while it writes requests; a reply may take as long as a prompt the user
//! answers). What crosses it is frames, the same for every service:
//!
//! ```text
//! u32 LE  J, the length of the JSON text      (at most JSON_MAX)
//! u32 LE  D, the length of the data after it  (at most DATA_MAX)
//! J bytes a JSON object, UTF-8
//! D bytes the frame's binary payloads, which the object refers to as
//!         {"at": OFFSET, "size": N, "type": "..."}
//! ```
//!
//! The first frame a program sends names the service, `{"service":
//! "notifications", "version": 1}`; the host answers `{"service":
//! "notifications", "version": V}`, V the version it speaks (no higher than
//! asked), or 0 when it provides no such service, after which the stream
//! ends. A host without the service and one that refuses it are told alike.
//! After that every frame is the service's: by convention a message with a
//! `"seq"` asks for an answer, which carries the same `"seq"`, and one
//! without is an event, in either direction. A frame over the limits, or one
//! whose JSON is not an object, ends the connection. No text a program sends
//! is interpreted by the host: each service says what it bounds and checks.
//!
//! The page runtime's `Services` (page/runtime/README.md, Services) is the
//! same channel with the page's shell at the other end; this module is the
//! native host's. A service here is a [`Service`] in [`registry`], offered
//! when gexc's environment asks for it (`GEX_NOTIFY=1` for notifications,
//! [`crate::notify`]); each prints what a shell would be given and answers
//! from a script of its own. A new service is a module with a `Service`
//! implementation and a line in `registry`.
//!
//! A read with nothing to read is `EAGAIN` (`poll_readable` says when it
//! would not be), which is how a reader parks; a service's answers that come
//! later (from its script's timer) are queued with [`Link::send`], which
//! wakes the parked readers.

use gex_kernel::errno::{Errno, SysResult};
use gex_kernel::vfs::{FileHandle, FileType, Metadata};
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// The largest JSON text a frame carries.
pub const JSON_MAX: usize = 64 << 10;
/// The most binary data a frame carries.
pub const DATA_MAX: usize = 4 << 20;

/// A service the host provides.
pub trait Service: Send + Sync {
    /// Its name, as a program asks for it.
    fn name(&self) -> &'static str;
    /// The highest version of its messages it speaks.
    fn version(&self) -> u32;
    /// A program's connection to it, speaking `version`.
    fn open(&self, link: Link, version: u32) -> Box<dyn Session>;
}

/// One connection's half on the host.
pub trait Session: Send {
    /// A frame from the program: its JSON object and its data.
    fn message(&mut self, m: &Json, data: &[u8]);
    /// The program closed the connection (or ended).
    fn closed(&mut self) {}
}

/// The services this host provides, as gexc's environment asks.
pub fn registry() -> &'static [Box<dyn Service>] {
    static R: OnceLock<Vec<Box<dyn Service>>> = OnceLock::new();
    R.get_or_init(|| {
        let mut v: Vec<Box<dyn Service>> = Vec::new();
        if crate::notify::presented() {
            v.push(Box::new(crate::notify::Notifications));
        }
        v
    })
}

/// A session's way to its program: frames queued for it to read.
#[derive(Clone, Debug)]
pub struct Link {
    conn: Weak<Mutex<Conn>>,
}

impl Link {
    /// Queue a frame; false when the program has gone.
    pub fn send(&self, json: &str, data: &[u8]) -> bool {
        let Some(c) = self.conn.upgrade() else { return false };
        lock(&c).output.extend(frame(json, data));
        crate::ready::notify();
        true
    }
    /// End the connection: the program reads the end once it has read what
    /// was sent.
    pub fn end(&self) {
        if let Some(c) = self.conn.upgrade() {
            lock(&c).ended = true;
            crate::ready::notify();
        }
    }
}

fn frame(json: &str, data: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(8 + json.len() + data.len());
    f.extend_from_slice(&(json.len() as u32).to_le_bytes());
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(json.as_bytes());
    f.extend_from_slice(data);
    f
}

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Default)]
struct Conn {
    /// Answers not yet read.
    output: VecDeque<u8>,
    ended: bool,
}

/// One open of the node.
pub struct ServicesDevice {
    conn: Arc<Mutex<Conn>>,
    /// What the program wrote that is not yet a whole frame.
    input: Mutex<Vec<u8>>,
    /// The service's half, once the first frame named one it provides.
    session: Mutex<Option<Box<dyn Session>>>,
}

impl std::fmt::Debug for ServicesDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServicesDevice")
    }
}

impl ServicesDevice {
    pub fn new() -> Arc<ServicesDevice> {
        Arc::new(ServicesDevice { conn: Arc::new(Mutex::new(Conn::default())), input: Mutex::new(Vec::new()), session: Mutex::new(None) })
    }

    fn link(&self) -> Link {
        Link { conn: Arc::downgrade(&self.conn) }
    }

    fn end(&self, why: &str) {
        eprintln!("[gex] /dev/gex-services: {why}: the connection is ended");
        self.link().end();
    }

    /// The whole frames written so far, each handled as it comes.
    fn take_frames(&self) {
        loop {
            let mut input = lock(&self.input);
            if input.len() < 8 {
                return;
            }
            let jl = u32::from_le_bytes(input[0..4].try_into().unwrap()) as usize;
            let dl = u32::from_le_bytes(input[4..8].try_into().unwrap()) as usize;
            if jl > JSON_MAX || dl > DATA_MAX {
                input.clear();
                drop(input);
                return self.end(&format!("a frame of {jl} + {dl} bytes, over the limits"));
            }
            if input.len() < 8 + jl + dl {
                return;
            }
            let rest = input.split_off(8 + jl + dl);
            let f = std::mem::replace(&mut *input, rest);
            drop(input);
            let (json, data) = f[8..].split_at(jl);
            let m = match std::str::from_utf8(json).map_err(|_| "not UTF-8".to_string()).and_then(parse) {
                Ok(m @ Json::Obj(_)) => m,
                _ => return self.end("a frame that is not a JSON object"),
            };
            let mut session = lock(&self.session);
            match session.as_mut() {
                Some(s) => s.message(&m, data),
                None => {
                    let name = m.str("service").unwrap_or("");
                    let asked = m.num("version").unwrap_or(1.0).clamp(0.0, u32::MAX as f64) as u32;
                    let found = registry().iter().find(|s| s.name() == name);
                    let version = found.map_or(0, |s| s.version().min(asked));
                    self.link().send(&format!("{{\"service\":{},\"version\":{version}}}", quote(&text(name, 64))), &[]);
                    match found.filter(|_| version > 0) {
                        Some(s) => *session = Some(s.open(self.link(), version)),
                        None => {
                            self.link().end();
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl FileHandle for ServicesDevice {
    fn read_at(&self, _offset: u64, buf: &mut [u8]) -> SysResult {
        let mut c = lock(&self.conn);
        if c.output.is_empty() {
            return if c.ended { Ok(0) } else { Err(Errno::EAGAIN) };
        }
        let n = buf.len().min(c.output.len());
        for (d, s) in buf.iter_mut().zip(c.output.drain(..n)) {
            *d = s;
        }
        Ok(n as u64)
    }

    fn write_at(&self, _offset: u64, buf: &[u8]) -> SysResult {
        if lock(&self.conn).ended {
            return Err(Errno::EPIPE);
        }
        lock(&self.input).extend_from_slice(buf);
        self.take_frames();
        crate::ready::notify();
        Ok(buf.len() as u64)
    }

    fn metadata(&self) -> Result<Metadata, Errno> {
        Ok(Metadata { file_type: FileType::CharDevice, size: 0, mode: 0o666, ino: 0, mtime: 0, nlink: 1, rdev: 0, times: Default::default() })
    }

    fn is_stream(&self) -> bool {
        true
    }

    fn poll_readable(&self) -> bool {
        let c = lock(&self.conn);
        !c.output.is_empty() || c.ended
    }

    fn available(&self) -> Option<u64> {
        Some(lock(&self.conn).output.len() as u64)
    }

    fn poll_hup(&self) -> bool {
        let c = lock(&self.conn);
        c.output.is_empty() && c.ended
    }
}

impl Drop for ServicesDevice {
    fn drop(&mut self) {
        if let Some(mut s) = lock(&self.session).take() {
            s.closed();
        }
    }
}

// ---------------------------------------------------------------------------
// JSON, as much as the messages need: a parser bounded by its input, and
// strings written escaped.

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key) {
            Some(Json::Str(s)) => Some(s),
            _ => None,
        }
    }
    pub fn num(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(Json::Num(n)) if n.is_finite() => Some(*n),
            _ => None,
        }
    }
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Json::Bool(true)))
    }
}

/// Parse one JSON value, the whole of `text`. Nesting is bounded.
pub fn parse(text: &str) -> Result<Json, String> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
        }
        fn lit(&mut self, w: &str, v: Json) -> Result<Json, String> {
            if self.s[self.i..].starts_with(w.as_bytes()) {
                self.i += w.len();
                Ok(v)
            } else {
                Err(format!("unexpected text at {}", self.i))
            }
        }
        fn hex4(&mut self) -> Result<u32, String> {
            let h = self.s.get(self.i..self.i + 4).ok_or("a short \\u escape")?;
            self.i += 4;
            u32::from_str_radix(std::str::from_utf8(h).map_err(|_| "a bad \\u escape")?, 16).map_err(|_| "a bad \\u escape".into())
        }
        fn string(&mut self) -> Result<String, String> {
            self.i += 1;
            let mut out = String::new();
            loop {
                let start = self.i;
                while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' {
                    self.i += 1;
                }
                out.push_str(std::str::from_utf8(&self.s[start..self.i]).map_err(|_| "a string that is not UTF-8")?);
                match self.s.get(self.i) {
                    None => return Err("an unterminated string".into()),
                    Some(b'"') => {
                        self.i += 1;
                        return Ok(out);
                    }
                    _ => {}
                }
                self.i += 1;
                let c = *self.s.get(self.i).ok_or("an unterminated escape")?;
                self.i += 1;
                match c {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let mut u = self.hex4()?;
                        if (0xD800..0xDC00).contains(&u) && self.s[self.i..].starts_with(b"\\u") {
                            self.i += 2;
                            let lo = self.hex4()?;
                            u = 0x10000 + ((u - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                        }
                        out.push(char::from_u32(u).unwrap_or('\u{FFFD}'));
                    }
                    _ => return Err("a bad escape".into()),
                }
            }
        }
        fn value(&mut self, depth: u32) -> Result<Json, String> {
            if depth > 32 {
                return Err("nested too deeply".into());
            }
            self.ws();
            match self.s.get(self.i) {
                None => Err("nothing".into()),
                Some(b'n') => self.lit("null", Json::Null),
                Some(b't') => self.lit("true", Json::Bool(true)),
                Some(b'f') => self.lit("false", Json::Bool(false)),
                Some(b'"') => self.string().map(Json::Str),
                Some(b'[') => {
                    self.i += 1;
                    let mut v = Vec::new();
                    self.ws();
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        return Ok(Json::Arr(v));
                    }
                    loop {
                        v.push(self.value(depth + 1)?);
                        self.ws();
                        match self.s.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b']') => {
                                self.i += 1;
                                return Ok(Json::Arr(v));
                            }
                            _ => return Err("a bad array".into()),
                        }
                    }
                }
                Some(b'{') => {
                    self.i += 1;
                    let mut m = Vec::new();
                    self.ws();
                    if self.s.get(self.i) == Some(&b'}') {
                        self.i += 1;
                        return Ok(Json::Obj(m));
                    }
                    loop {
                        self.ws();
                        if self.s.get(self.i) != Some(&b'"') {
                            return Err("a bad object".into());
                        }
                        let k = self.string()?;
                        self.ws();
                        if self.s.get(self.i) != Some(&b':') {
                            return Err("a bad object".into());
                        }
                        self.i += 1;
                        let v = self.value(depth + 1)?;
                        m.push((k, v));
                        self.ws();
                        match self.s.get(self.i) {
                            Some(b',') => self.i += 1,
                            Some(b'}') => {
                                self.i += 1;
                                return Ok(Json::Obj(m));
                            }
                            _ => return Err("a bad object".into()),
                        }
                    }
                }
                Some(_) => {
                    let start = self.i;
                    while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                        self.i += 1;
                    }
                    std::str::from_utf8(&self.s[start..self.i]).ok().and_then(|t| t.parse().ok()).map(Json::Num).ok_or_else(|| format!("unexpected text at {start}"))
                }
            }
        }
    }
    let mut p = P { s: text.as_bytes(), i: 0 };
    let v = p.value(0)?;
    p.ws();
    if p.i != p.s.len() {
        return Err("text after the value".into());
    }
    Ok(v)
}

/// `s` as a JSON string.
pub fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ---------------------------------------------------------------------------
// What a shell takes of a message (each service's own checks use these): text cut at a character boundary, control
// characters other than a line break and a tab dropped; an image whose bytes
// are within its limit and are a PNG, JPEG, GIF or WebP.

/// `s` with control characters dropped, cut to `max` bytes of UTF-8.
pub fn text(s: &str, max: usize) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if o.len() + c.len_utf8() > max {
            break;
        }
        o.push(c);
    }
    o
}


/// The type an image's bytes say they are.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The image a message's `{"at", "size"}` names in its frame's data, if it
/// is one a shell takes: its type and length.
pub fn image(m: &Json, key: &str, data: &[u8], max: usize) -> Result<Option<(&'static str, usize)>, String> {
    let Some(r) = m.get(key) else { return Ok(None) };
    if *r == Json::Null {
        return Ok(None);
    }
    let (Some(at), Some(size)) = (r.num("at"), r.num("size")) else { return Err(format!("{key}: not a reference to the data")) };
    let (at, size) = (at as usize, size as usize);
    if size > max {
        return Err(format!("{key}: {size} bytes, over the {max} a shell takes"));
    }
    let bytes = data.get(at..at.checked_add(size).unwrap_or(usize::MAX)).ok_or_else(|| format!("{key}: past the frame's data"))?;
    match sniff(bytes) {
        Some(t) => Ok(Some((t, size))),
        None => Err(format!("{key}: not a PNG, JPEG, GIF or WebP image")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trips_what_the_messages_carry() {
        let v = parse(r#"{"op":"post","id":"a\"b","n":-1.5e2,"l":[true,false,null],"u":"\u00e9\ud83d\ude00"}"#).unwrap();
        assert_eq!(v.str("op"), Some("post"));
        assert_eq!(v.str("id"), Some("a\"b"));
        assert_eq!(v.num("n"), Some(-150.0));
        assert_eq!(v.str("u"), Some("é😀"));
        assert_eq!(parse(&quote("a\"\n\u{1}")).unwrap(), Json::Str("a\"\n\u{1}".into()));
        assert!(parse("{\"a\":1} x").is_err());
        assert!(parse(&"[".repeat(100)).is_err());
    }

    #[test]
    fn text_is_cut_on_a_character_and_loses_its_controls() {
        assert_eq!(text("ab\u{7}c\nd", 100), "abc\nd");
        assert_eq!(text("ééé", 5), "éé");
    }

    #[test]
    fn images_are_sniffed() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff(b"<svg"), None);
    }
}
