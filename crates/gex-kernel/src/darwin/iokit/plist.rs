//! What a registry entry's properties are, and how they cross to a program
//! and back: libkern's containers (`OSDictionary`, `OSArray`, `OSString`,
//! `OSNumber`, `OSData`, `OSBoolean`), in the binary serialization that
//! IOKit's user side speaks to a kernel that says it can
//! (`io_server_version` of 2015 and after), and in the older XML, which a
//! matching dictionary is still sent in by the calls that predate it.
//!
//! The binary form is a signature word, then one word for each object: its
//! kind in the top byte (the high bit marks the last member of the
//! collection it is in), its length or count in the low three, and, for a
//! number, a string or data, the bytes after it to the next word. An object
//! seen before is sent again as its index among those sent.

/// A property's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Keys in the order they were set, as an `OSDictionary` keeps them.
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    Set(Vec<Value>),
    Str(String),
    Data(Vec<u8>),
    /// The value and how many bits it has (8, 16, 32 or 64).
    Num(u64, u32),
    Bool(bool),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.to_string())
    }

    pub fn n32(v: u64) -> Value {
        Value::Num(v, 32)
    }

    pub fn n64(v: u64) -> Value {
        Value::Num(v, 64)
    }

    pub fn dict(items: Vec<(&str, Value)>) -> Value {
        Value::Dict(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Num(v, _) => Some(*v),
            _ => None,
        }
    }

    /// Whether `self` is `want` as IOKit compares properties when matching
    /// (`OSObject::isEqualTo`): numbers by value whatever their widths, a
    /// string and data by their bytes, collections member by member.
    pub fn same(&self, want: &Value) -> bool {
        match (self, want) {
            (Value::Num(a, _), Value::Num(b, _)) => a == b,
            (Value::Dict(a), Value::Dict(_)) => a.len() == dict_len(want) && a.iter().all(|(k, v)| want.get(k).is_some_and(|w| v.same(w))),
            (Value::Array(a), Value::Array(b)) | (Value::Set(a), Value::Set(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same(y)),
            (a, b) => a == b,
        }
    }
}

fn dict_len(v: &Value) -> usize {
    match v {
        Value::Dict(d) => d.len(),
        _ => 0,
    }
}

const DICTIONARY: u32 = 0x0100_0000;
const ARRAY: u32 = 0x0200_0000;
const SET: u32 = 0x0300_0000;
const NUMBER: u32 = 0x0400_0000;
const SYMBOL: u32 = 0x0800_0000;
const STRING: u32 = 0x0900_0000;
const DATA: u32 = 0x0a00_0000;
const BOOLEAN: u32 = 0x0b00_0000;
const OBJECT: u32 = 0x0c00_0000;
const TYPE_MASK: u32 = 0x7f00_0000;
const DATA_MASK: u32 = 0x00ff_ffff;
const END: u32 = 0x8000_0000;
/// `kOSSerializeBinarySignature`: "\323\0\0" and its terminator.
pub const SIGNATURE: [u8; 4] = [0xd3, 0, 0, 0];

/// `v` in the binary form. (Nothing is sent again by reference: what the
/// kernel serializes here has no object twice.)
pub fn to_binary(v: &Value) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    put(&mut out, v, true);
    out
}

fn word(out: &mut Vec<u8>, key: u32, last: bool, bytes: &[u8]) {
    out.extend_from_slice(&(key | if last { END } else { 0 }).to_le_bytes());
    out.extend_from_slice(bytes);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

fn put(out: &mut Vec<u8>, v: &Value, last: bool) {
    match v {
        Value::Dict(d) => {
            word(out, DICTIONARY | d.len() as u32, last, &[]);
            for (i, (k, v)) in d.iter().enumerate() {
                let mut key = k.as_bytes().to_vec();
                key.push(0);
                word(out, SYMBOL | key.len() as u32, false, &key);
                put(out, v, i + 1 == d.len());
            }
        }
        Value::Array(a) | Value::Set(a) => {
            word(out, if matches!(v, Value::Set(_)) { SET } else { ARRAY } | a.len() as u32, last, &[]);
            for (i, v) in a.iter().enumerate() {
                put(out, v, i + 1 == a.len());
            }
        }
        Value::Str(s) => word(out, STRING | s.len() as u32, last, s.as_bytes()),
        Value::Data(b) => word(out, DATA | b.len() as u32, last, b),
        Value::Num(n, bits) => word(out, NUMBER | bits, last, &n.to_le_bytes()),
        Value::Bool(b) => word(out, BOOLEAN | *b as u32, last, &[]),
    }
}

/// A collection being filled while unserializing.
enum Open {
    Dict(Vec<(String, Value)>, Option<String>),
    List(Vec<Value>, bool),
}

/// The binary form back into a value, as `OSUnserializeBinary` reads it;
/// `None` for anything it would refuse.
pub fn from_binary(b: &[u8]) -> Option<Value> {
    if b.len() < 4 || b[..4] != SIGNATURE {
        return None;
    }
    let mut at = 4;
    let next = |at: &mut usize| -> Option<u32> {
        let w = u32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
        *at += 4;
        Some(w)
    };
    // Every object made, by index, for a reference to one; the open
    // collections, innermost last, each with whether it ends with what
    // ended its parent's member.
    let mut objs: Vec<Value> = Vec::new();
    let mut stack: Vec<(Open, usize, bool)> = Vec::new();
    loop {
        let key = next(&mut at)?;
        let len = (key & DATA_MASK) as usize;
        let end = key & END != 0;
        let words = len.div_ceil(4) * 4;
        let mut bytes = |n: usize| -> Option<&[u8]> {
            let s = b.get(at..at + n)?;
            at += n;
            Some(s)
        };
        let (value, open) = match key & TYPE_MASK {
            DICTIONARY => (None, Some(Open::Dict(Vec::with_capacity(len), None))),
            ARRAY => (None, Some(Open::List(Vec::with_capacity(len), false))),
            SET => (None, Some(Open::List(Vec::with_capacity(len), true))),
            OBJECT => (Some(objs.get(len)?.clone()), None),
            NUMBER => {
                if ![8, 16, 32, 64].contains(&len) {
                    return None;
                }
                let raw = bytes(8)?;
                (Some(Value::Num(u64::from_le_bytes(raw.try_into().ok()?), len as u32)), None)
            }
            SYMBOL => {
                let raw = bytes(words)?;
                let s = raw.get(..len.checked_sub(1)?)?;
                (Some(Value::Str(String::from_utf8_lossy(s).into_owned())), None)
            }
            STRING => (Some(Value::Str(String::from_utf8_lossy(&bytes(words)?[..len]).into_owned())), None),
            DATA => (Some(Value::Data(bytes(words)?[..len].to_vec())), None),
            BOOLEAN => (Some(Value::Bool(len != 0)), None),
            _ => return None,
        };
        let index = objs.len();
        let done = match (value, open) {
            // An empty collection is whole as soon as it is seen.
            (None, Some(Open::Dict(d, _))) if len == 0 => Some(Value::Dict(d)),
            (None, Some(Open::List(l, set))) if len == 0 => Some(if set { Value::Set(l) } else { Value::Array(l) }),
            (None, Some(open)) => {
                objs.push(Value::Bool(false));
                stack.push((open, index, end));
                None
            }
            (Some(v), _) => Some(v),
            (None, None) => return None,
        };
        let Some(mut v) = done else { continue };
        if !matches!(key & TYPE_MASK, OBJECT) {
            objs.push(v.clone());
        }
        let mut end = end;
        loop {
            let Some((top, _, _)) = stack.last_mut() else { return Some(v) };
            match top {
                Open::Dict(d, k @ None) => {
                    *k = Some(v.as_str()?.to_string());
                    let _ = d;
                    break;
                }
                Open::Dict(d, k) => d.push((k.take()?, v)),
                Open::List(l, _) => l.push(v),
            }
            if !end {
                break;
            }
            // The collection is whole: it is the member that ends its own
            // parent's if it was marked so.
            let (open, slot, ends_parent) = stack.pop()?;
            v = match open {
                Open::Dict(d, None) => Value::Dict(d),
                Open::Dict(_, Some(_)) => return None,
                Open::List(l, true) => Value::Set(l),
                Open::List(l, false) => Value::Array(l),
            };
            objs[slot] = v.clone();
            end = ends_parent;
        }
    }
}

/// `v` as IOKit's XML (`OSSerialize`'s: a plist body with no header).
pub fn to_xml(v: &Value) -> String {
    let mut s = String::new();
    xml_put(&mut s, v);
    s
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn xml_put(s: &mut String, v: &Value) {
    match v {
        Value::Dict(d) => {
            s.push_str("<dict>");
            for (k, v) in d {
                s.push_str(&format!("<key>{}</key>", escape(k)));
                xml_put(s, v);
            }
            s.push_str("</dict>");
        }
        Value::Array(a) | Value::Set(a) => {
            let tag = if matches!(v, Value::Set(_)) { "set" } else { "array" };
            s.push_str(&format!("<{tag}>"));
            a.iter().for_each(|v| xml_put(s, v));
            s.push_str(&format!("</{tag}>"));
        }
        Value::Str(t) => s.push_str(&format!("<string>{}</string>", escape(t))),
        Value::Data(b) => s.push_str(&format!("<data>{}</data>", base64(b))),
        Value::Num(n, bits) => s.push_str(&format!("<integer size=\"{bits}\">0x{n:x}</integer>")),
        Value::Bool(b) => s.push_str(if *b { "<true/>" } else { "<false/>" }),
    }
}

const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(b: &[u8]) -> String {
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { B64[(n >> (18 - 6 * i)) as usize & 63] as char } else { '=' });
        }
    }
    s
}

fn unbase64(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let Some(v) = B64.iter().position(|&x| x == c) else { continue };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// IOKit's XML back into a value (`OSUnserializeXML`'s subset that IOKit's
/// user side writes: its collections, strings, integers, data, booleans,
/// and an object given an `ID` named again by `IDREF`).
pub fn from_xml(s: &str) -> Option<Value> {
    let mut p = Xml { s: s.as_bytes(), at: 0, ids: Vec::new() };
    p.value()
}

struct Xml<'a> {
    s: &'a [u8],
    at: usize,
    ids: Vec<(String, Value)>,
}

impl Xml<'_> {
    /// The next tag: its name, its attributes' text, whether it closes
    /// (`</x>`) and whether it is empty (`<x/>`).
    fn tag(&mut self) -> Option<(String, String, bool, bool)> {
        while self.at < self.s.len() && self.s[self.at] != b'<' {
            self.at += 1;
        }
        let close = *self.s.get(self.at + 1)? == b'/';
        let start = self.at + if close { 2 } else { 1 };
        let end = start + self.s[start..].iter().position(|&c| c == b'>')?;
        self.at = end + 1;
        let inner = std::str::from_utf8(&self.s[start..end]).ok()?;
        let empty = inner.ends_with('/');
        let inner = inner.trim_end_matches('/');
        let (name, attrs) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        Some((name.to_string(), attrs.to_string(), close, empty))
    }

    fn text(&mut self) -> String {
        let start = self.at;
        while self.at < self.s.len() && self.s[self.at] != b'<' {
            self.at += 1;
        }
        let t = String::from_utf8_lossy(&self.s[start..self.at]).into_owned();
        t.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
    }

    fn attr(attrs: &str, name: &str) -> Option<String> {
        let at = attrs.find(&format!("{name}=\""))? + name.len() + 2;
        Some(attrs[at..].split('"').next()?.to_string())
    }

    fn value(&mut self) -> Option<Value> {
        let (name, attrs, close, empty) = self.tag()?;
        if close {
            return None;
        }
        if let Some(r) = Self::attr(&attrs, "IDREF") {
            return self.ids.iter().find(|(id, _)| *id == r).map(|(_, v)| v.clone());
        }
        let v = match name.as_str() {
            "dict" if empty => Value::Dict(Vec::new()),
            "dict" => {
                let mut d = Vec::new();
                loop {
                    let save = self.at;
                    let (n, _, close, _) = self.tag()?;
                    if close && n == "dict" {
                        break;
                    }
                    if n != "key" {
                        self.at = save;
                        return None;
                    }
                    let k = self.text();
                    self.tag()?;
                    d.push((k, self.value()?));
                }
                Value::Dict(d)
            }
            "array" | "set" if empty => if name == "set" { Value::Set(Vec::new()) } else { Value::Array(Vec::new()) },
            "array" | "set" => {
                let mut a = Vec::new();
                loop {
                    let save = self.at;
                    let (n, _, close, _) = self.tag()?;
                    if close && n == name {
                        break;
                    }
                    self.at = save;
                    a.push(self.value()?);
                }
                if name == "set" { Value::Set(a) } else { Value::Array(a) }
            }
            "string" | "symbol" if empty => Value::Str(String::new()),
            "string" | "symbol" => {
                let t = self.text();
                self.tag()?;
                Value::Str(t)
            }
            "integer" => {
                let bits = Self::attr(&attrs, "size").and_then(|b| b.parse().ok()).unwrap_or(64);
                let t = if empty { String::new() } else { self.text() };
                if !empty {
                    self.tag()?;
                }
                let t = t.trim();
                let n = if let Some(h) = t.strip_prefix("0x") { u64::from_str_radix(h, 16).ok()? } else { t.parse::<i64>().map(|v| v as u64).or_else(|_| t.parse::<u64>()).ok()? };
                Value::Num(n, bits)
            }
            "data" => {
                let t = if empty { String::new() } else { self.text() };
                if !empty {
                    self.tag()?;
                }
                Value::Data(unbase64(&t))
            }
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return None,
        };
        if let Some(id) = Self::attr(&attrs, "ID") {
            self.ids.push((id, v.clone()));
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        Value::dict(vec![
            ("IOProviderClass", Value::str("IOHIDSystem")),
            ("Count", Value::n32(7)),
            ("Nested", Value::dict(vec![("a", Value::Bool(true)), ("b", Value::Array(vec![Value::Data(vec![1, 2, 3]), Value::n64(1 << 40)]))])),
            ("Empty", Value::Dict(Vec::new())),
            ("Last", Value::str("x")),
        ])
    }

    #[test]
    fn binary_comes_back_as_it_went() {
        let v = sample();
        assert_eq!(from_binary(&to_binary(&v)), Some(v));
    }

    #[test]
    fn xml_comes_back_as_it_went() {
        let v = sample();
        assert_eq!(from_xml(&to_xml(&v)), Some(v));
    }

    #[test]
    fn a_matching_dictionary_as_iokit_writes_it() {
        // IOServiceMatching("IOHIDSystem"), binary: a dictionary of one,
        // its key a symbol, its value the string that ends it.
        let mut b = SIGNATURE.to_vec();
        for w in [DICTIONARY | END | 1, SYMBOL | 16] {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b.extend_from_slice(b"IOProviderClass\0");
        b.extend_from_slice(&(STRING | END | 11).to_le_bytes());
        b.extend_from_slice(b"IOHIDSystem\0");
        assert_eq!(from_binary(&b), Some(Value::dict(vec![("IOProviderClass", Value::str("IOHIDSystem"))])));
        assert_eq!(from_xml("<dict><key>IOProviderClass</key><string>IOHIDSystem</string></dict>"), Some(Value::dict(vec![("IOProviderClass", Value::str("IOHIDSystem"))])));
    }
}
