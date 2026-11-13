//! The registry: a tree of entries in the service plane, each an instance
//! of a class (whose superclasses it conforms to as well), with a name, a
//! location, an id and properties; and what IOKit's matching dictionaries
//! ask of an entry.

use super::plist::Value;

/// Each class an entry may be, with its superclass: what
/// `IOObjectConformsTo` and `IOProviderClass` go by.
const CLASSES: &[(&str, &str)] = &[
    ("IORegistryEntry", "OSObject"),
    ("IOService", "IORegistryEntry"),
    ("IOPlatformExpertDevice", "IOService"),
    ("IOResources", "IOService"),
    ("IOHIDSystem", "IOService"),
    ("IOUserClient", "IOService"),
    ("IOHIDUserClient", "IOUserClient"),
    ("IOHIDParamUserClient", "IOUserClient"),
    ("IOHIDEventSystemUserClient", "IOUserClient"),
    ("IOPCIDevice", "IOService"),
    ("IOGraphicsDevice", "IOService"),
    ("IOFramebuffer", "IOGraphicsDevice"),
    ("IONDRVFramebuffer", "IOFramebuffer"),
    ("IODisplayConnect", "IOService"),
    ("IODisplay", "IOService"),
    ("AppleDisplay", "IODisplay"),
    ("IOFramebufferUserClient", "IOUserClient"),
    ("IOFramebufferSharedUserClient", "IOUserClient"),
    ("IOAccelerator", "IOService"),
    ("IOAccelerationUserClient", "IOUserClient"),
];

/// `class`'s superclass, if it has one (OSObject has none).
pub fn superclass(class: &str) -> Option<&'static str> {
    CLASSES.iter().find(|(c, _)| *c == class).map(|(_, s)| *s)
}

/// Whether an instance of `class` is a `want`.
pub fn conforms(class: &str, want: &str) -> bool {
    let mut c = Some(class);
    while let Some(now) = c {
        if now == want {
            return true;
        }
        c = superclass(now);
    }
    false
}

/// What opening an entry (`IOServiceOpen`) makes: the user client a
/// driver of that class gives a task ([`super::Client`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opens {
    Nothing,
    HidSystem,
    Framebuffer,
}

pub struct Entry {
    pub id: u64,
    pub class: &'static str,
    pub name: String,
    pub location: String,
    pub props: Vec<(String, Value)>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Whether it is a service (an `IOService`, registered and matched),
    /// which only services are found by matching.
    pub service: bool,
    pub opens: Opens,
}

impl Entry {
    pub fn prop(&self, key: &str) -> Option<&Value> {
        self.props.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn set_prop(&mut self, key: &str, v: Value) {
        match self.props.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = v,
            None => self.props.push((key.to_string(), v)),
        }
    }

    /// The properties as `IORegistryEntryCreateCFProperties` has them.
    pub fn properties(&self) -> Value {
        Value::Dict(self.props.clone())
    }
}

/// The display the framebuffer drives: its size and depth.
#[derive(Clone, Copy, Debug)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
}

/// The display mode asked for with `GEX_DISPLAY=WIDTHxHEIGHT`, or 1024 by
/// 768.
pub fn mode() -> Mode {
    let asked = std::env::var("GEX_DISPLAY").ok().and_then(|s| {
        let (w, h) = s.split_once('x')?;
        Some(Mode { width: w.trim().parse().ok()?, height: h.trim().parse().ok()? })
    });
    asked.filter(|m| (64..=8192).contains(&m.width) && (64..=8192).contains(&m.height)).unwrap_or(Mode { width: 1024, height: 768 })
}

pub struct Registry {
    pub entries: Vec<Entry>,
}

/// Index of the root entry.
pub const ROOT: usize = 0;

impl Registry {
    fn add(&mut self, parent: Option<usize>, class: &'static str, name: &str, location: &str, props: Vec<(&str, Value)>, opens: Opens) -> usize {
        let i = self.entries.len();
        let service = conforms(class, "IOService");
        let mut props: Vec<(String, Value)> = props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        if service && !props.iter().any(|(k, _)| k == "IOClass") {
            props.insert(0, ("IOClass".to_string(), Value::str(class)));
        }
        self.entries.push(Entry { id: 0x1_0000_0100 + i as u64, class, name: name.to_string(), location: location.to_string(), props, parent, children: Vec::new(), service, opens });
        if let Some(p) = parent {
            self.entries[p].children.push(i);
        }
        i
    }

    /// The registry of a machine with one display and the HID system:
    ///
    /// ```text
    /// Root
    ///   IOResources
    ///     IOHIDSystem
    ///   gex (IOPlatformExpertDevice)
    ///     display@2 (IOPCIDevice)
    ///       IONDRVFramebuffer
    ///         display0 (IODisplayConnect)
    ///           AppleDisplay
    /// ```
    pub fn new() -> Registry {
        let mut r = Registry { entries: Vec::new() };
        let root = r.add(None, "IORegistryEntry", "Root", "", vec![("IOKitBuildVersion", Value::str("Darwin Kernel Version 18.7.0 (gex)")), ("IOKitDiagnostics", Value::Dict(Vec::new()))], Opens::Nothing);
        r.entries[root].service = false;
        let platform = r.add(
            Some(root),
            "IOPlatformExpertDevice",
            "gex",
            "",
            vec![
                ("model", Value::Data(b"Gex1,1\0".to_vec())),
                ("compatible", Value::Data(b"Gex1,1\0".to_vec())),
                ("IOPlatformSerialNumber", Value::str("GEX00000001")),
                ("IOPlatformUUID", Value::str("00000000-0000-1000-8000-000000000000")),
                ("manufacturer", Value::Data(b"gex\0".to_vec())),
                ("IOBusyInterest", Value::str("IOCommand is not serializable")),
            ],
            Opens::Nothing,
        );
        let resources = r.add(Some(root), "IOResources", "IOResources", "", vec![("IOKit", Value::str("IOService")), ("IOConsoleUsers", Value::Array(Vec::new()))], Opens::Nothing);
        r.add(
            Some(resources),
            "IOHIDSystem",
            "IOHIDSystem",
            "",
            vec![
                ("IOProviderClass", Value::str("IOResources")),
                ("IOResourceMatch", Value::str("IOKit")),
                ("IOMatchCategory", Value::str("IOHID")),
                ("CFBundleIdentifier", Value::str("com.apple.iokit.IOHIDSystem")),
                ("HIDParameters", Value::Dict(Vec::new())),
            ],
            Opens::HidSystem,
        );
        let mode = mode();
        let pci = r.add(
            Some(platform),
            "IOPCIDevice",
            "display",
            "2",
            vec![
                ("name", Value::Data(b"display\0".to_vec())),
                ("vendor-id", Value::Data(0x1234u32.to_le_bytes().to_vec())),
                ("device-id", Value::Data(0x1111u32.to_le_bytes().to_vec())),
                ("class-code", Value::Data(0x30000u32.to_le_bytes().to_vec())),
                ("model", Value::Data(b"gex display\0".to_vec())),
            ],
            Opens::Nothing,
        );
        let fb = r.add(
            Some(pci),
            "IONDRVFramebuffer",
            "IONDRVFramebuffer",
            "",
            vec![
                ("IOProviderClass", Value::str("IOPCIDevice")),
                ("CFBundleIdentifier", Value::str("com.apple.iokit.IONDRVSupport")),
                ("IOFBDependentID", Value::n64(0)),
                ("IOFBDependentIndex", Value::n32(0)),
                ("IOFramebufferOpenGLIndex", Value::n32(0)),
                ("IOFBCurrentPixelCount", Value::n64(mode.width as u64 * mode.height as u64)),
                ("IOFBWidth", Value::n32(mode.width as u64)),
                ("IOFBHeight", Value::n32(mode.height as u64)),
            ],
            Opens::Framebuffer,
        );
        let connect = r.add(Some(fb), "IODisplayConnect", "display0", "", vec![("IOProviderClass", Value::str("IOFramebuffer"))], Opens::Nothing);
        r.add(
            Some(connect),
            "AppleDisplay",
            "AppleDisplay",
            "",
            vec![
                ("IOProviderClass", Value::str("IODisplayConnect")),
                ("DisplayVendorID", Value::n32(0x756e6b6e)),
                ("DisplayProductID", Value::n32(0x717)),
                ("DisplaySerialNumber", Value::n32(0)),
                ("IODisplayEDID", Value::Data(Vec::new())),
            ],
            Opens::Nothing,
        );
        r
    }

    /// The entry's path in the service plane (`IOService:/gex/...`), each
    /// component its name and `@location` if it has one.
    pub fn path(&self, i: usize) -> String {
        let mut parts = Vec::new();
        let mut at = Some(i);
        while let Some(e) = at {
            if e != ROOT {
                let en = &self.entries[e];
                parts.push(if en.location.is_empty() { en.name.clone() } else { format!("{}@{}", en.name, en.location) });
            }
            at = self.entries[e].parent;
        }
        parts.reverse();
        format!("IOService:/{}", parts.join("/"))
    }

    /// The entry a path names: in the service plane (the plane's name and
    /// colon may be left out), each component a name with or without its
    /// location. `IODeviceTree:/` and other planes have nothing here but
    /// their root.
    pub fn lookup(&self, path: &str) -> Option<usize> {
        let rest = match path.split_once(':') {
            Some(("IOService", rest)) => rest,
            Some((_, rest)) => return (rest.trim_matches('/').is_empty()).then_some(ROOT),
            None => path,
        };
        let mut at = ROOT;
        for part in rest.split('/').filter(|p| !p.is_empty()) {
            let (name, location) = part.split_once('@').map_or((part, None), |(n, l)| (n, Some(l)));
            at = *self.entries[at].children.iter().find(|&&c| {
                let e = &self.entries[c];
                (e.name == name || e.class == name) && location.is_none_or(|l| e.location == l)
            })?;
        }
        Some(at)
    }

    /// Every entry below `i`, depth first, in the order an iterator of the
    /// plane gives them.
    pub fn below(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        for &c in &self.entries[i].children {
            out.push(c);
            out.extend(self.below(c));
        }
        out
    }

    pub fn by_id(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// The services a matching dictionary matches, in the registry's order.
    pub fn matching(&self, dict: &Value) -> Vec<usize> {
        (0..self.entries.len()).filter(|&i| self.entries[i].service && self.matches(i, dict)).collect()
    }

    /// Whether entry `i` is what `dict` asks for, as `IOService`'s
    /// matching reads its keys: the class it is, its name or any of the
    /// names it is compatible with, properties it has (with their values,
    /// or only that it has them), its id, a resource that is published, its
    /// parent's matching, its path. Keys a family would compare itself are
    /// not asked of here.
    pub fn matches(&self, i: usize, dict: &Value) -> bool {
        let e = &self.entries[i];
        let Value::Dict(keys) = dict else { return false };
        for (k, want) in keys {
            let ok = match k.as_str() {
                "IOProviderClass" => want.as_str().is_some_and(|c| conforms(e.class, c)),
                "IONameMatch" => {
                    let names = self.names(i);
                    match want {
                        Value::Str(s) => names.iter().any(|n| n == s),
                        Value::Array(a) | Value::Set(a) => a.iter().any(|w| w.as_str().is_some_and(|s| names.iter().any(|n| n == s))),
                        _ => false,
                    }
                }
                "IOPropertyMatch" => match want {
                    Value::Dict(_) => self.has_props(i, want),
                    Value::Array(a) => a.iter().any(|w| self.has_props(i, w)),
                    _ => false,
                },
                "IOPropertyExistsMatch" => match want {
                    Value::Str(s) => e.prop(s).is_some(),
                    Value::Array(a) | Value::Set(a) => a.iter().all(|w| w.as_str().is_some_and(|s| e.prop(s).is_some())),
                    _ => false,
                },
                "IORegistryEntryID" => want.as_u64() == Some(e.id),
                "IOResourceMatch" => want.as_str().is_some_and(|r| self.entries.iter().any(|x| x.class == "IOResources" && x.prop(r).is_some())),
                "IOParentMatch" => e.parent.is_some_and(|p| self.matches(p, want)),
                "IOPathMatch" => want.as_str().is_some_and(|p| self.lookup(p) == Some(i)),
                "IOLocationMatch" => match want {
                    Value::Dict(_) => self.has_props(i, want),
                    _ => false,
                },
                "IONameMatched" | "IOMatchCategory" | "IOProbeScore" | "IOKitDebug" | "IOMatchedPersonality" => true,
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// The names an entry answers to: its own, and the names its
    /// `compatible` and `name` properties list.
    fn names(&self, i: usize) -> Vec<String> {
        let e = &self.entries[i];
        let mut names = vec![e.name.clone()];
        for key in ["compatible", "name"] {
            if let Some(Value::Data(d)) = e.prop(key) {
                names.extend(d.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()));
            }
        }
        names
    }

    /// Whether the entry has every property `want` names, with its value.
    fn has_props(&self, i: usize, want: &Value) -> bool {
        let Value::Dict(w) = want else { return false };
        w.iter().all(|(k, v)| self.entries[i].prop(k).is_some_and(|have| have.same(v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_service_is_found_by_its_class_and_its_superclasses() {
        let r = Registry::new();
        let hid = r.matching(&Value::dict(vec![("IOProviderClass", Value::str("IOHIDSystem"))]));
        assert_eq!(hid.len(), 1);
        assert_eq!(r.entries[hid[0]].class, "IOHIDSystem");
        let fbs = r.matching(&Value::dict(vec![("IOProviderClass", Value::str("IOFramebuffer"))]));
        assert_eq!(fbs.iter().map(|&i| r.entries[i].class).collect::<Vec<_>>(), ["IONDRVFramebuffer"]);
        assert!(r.matching(&Value::dict(vec![("IOProviderClass", Value::str("IOUSBDevice"))])).is_empty());
        // (the root is no service)
        assert!(!r.matching(&Value::dict(vec![("IOProviderClass", Value::str("IOService"))])).contains(&ROOT));
    }

    #[test]
    fn names_properties_and_paths() {
        let r = Registry::new();
        let by_name = r.matching(&Value::dict(vec![("IONameMatch", Value::str("display"))]));
        assert_eq!(by_name.iter().map(|&i| r.entries[i].class).collect::<Vec<_>>(), ["IOPCIDevice"]);
        let by_prop = r.matching(&Value::dict(vec![("IOPropertyMatch", Value::dict(vec![("IOFBDependentIndex", Value::Num(0, 64))]))]));
        assert_eq!(by_prop.len(), 1);
        let fb = by_prop[0];
        assert_eq!(r.path(fb), "IOService:/gex/display@2/IONDRVFramebuffer");
        assert_eq!(r.lookup("IOService:/gex/display@2/IONDRVFramebuffer"), Some(fb));
        assert_eq!(r.lookup("IOService:/gex/display/IONDRVFramebuffer"), Some(fb));
        assert_eq!(r.lookup("IOService:/"), Some(ROOT));
        assert_eq!(r.lookup("IOService:/nothing"), None);
        let child = r.matching(&Value::dict(vec![("IOParentMatch", Value::dict(vec![("IOProviderClass", Value::str("IOFramebuffer"))]))]));
        assert_eq!(child.iter().map(|&i| r.entries[i].class).collect::<Vec<_>>(), ["IODisplayConnect"]);
    }
}
