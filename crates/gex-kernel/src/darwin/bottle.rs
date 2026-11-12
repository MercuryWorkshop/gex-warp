//! Bottles: Mac environments that share one system, as Wine prefixes share
//! one Wine.
//!
//! A Mac keeps its system on a sealed volume nothing writes and everything
//! else on a data volume, the two joined at a few directories. A bottle is
//! that split, with many data volumes over one system:
//!
//! - the **system layer** is a Mac root as pyrus builds it (`/System`,
//!   `/usr`, `/bin`, `/sbin`, `/private/etc`): one tree, read by every
//!   bottle and written by none ([`crate::vfs::readonly::ReadOnly`]);
//! - a **bottle** is a directory with the rest ([`PARTS`]): `Users`,
//!   `Applications`, `Library` and `private/var`, each bound over the
//!   system layer's directory of that name and written as any directory
//!   is. A new bottle's are copies of the system layer's own, so a bottle
//!   starts as the root it was made from.
//!
//! The joins are per-directory binds because that is what the kernel's
//! namespace already is, a table of mounts by path, and what a Mac's own
//! join is (its firmlinks); a union of two whole trees would need
//! whiteouts and copy-up for no directory a Mac writes outside these.
//!
//! A bottle's **configuration** is one of its own files, [`CONF`]
//! (`Library/Preferences/bottle.conf` in the bottle's directory), as a
//! Mac's machine-wide settings are files of `/Library/Preferences`: which
//! system layer it is over, which macOS and which Mac its programs are
//! told they are on, what of the system outside is shared in, and the same
//! again for single applications ([`Conf`]). The kernel reads it when a
//! program starts, so a change is seen by the programs started after it. A
//! root that is no bottle's is configured by the same file, where it has
//! one.
//!
//! Which bottle a Mac program runs in is its environment's word,
//! [`NAMED`], as `WINEPREFIX` is a Windows program's: on a host whose tree
//! is another system's (the page's), the kernel enters the bottle as the
//! process becomes the program ([`super::root`]); `gexc`, whose guest tree
//! is the Mac's own, lays the same binds out as mounts before it starts
//! ([`Layout`]).
use std::collections::BTreeSet;

/// Where a bottle's configuration is, in the bottle and (so) in its root.
pub const CONF: &str = "/Library/Preferences/bottle.conf";

/// The Macs a root's userland knows (pyrus's oss/mac-models, which its
/// build.sh puts here): one row a model, its fields apart by tabs, `-` for
/// what is not known.
pub const MODELS: &str = "/usr/share/gex/mac-models";

/// What of a root is a bottle's own, bound over the system layer's.
pub const PARTS: [&str; 4] = ["/Users", "/Applications", "/Library", "/private/var"];

/// The environment's word for the bottle a Mac program is to run in: the
/// bottle's directory, by its name in the system the program is started
/// from.
pub const NAMED: &str = "GEX_BOTTLE";

/// The environment's word for which Mac a process is to be told it is on,
/// whatever its bottle says: a model of [`MODELS`], `GEX_MAC_MODEL=Mac14,3`.
/// Inherited, as [`super::info::CHOSEN`] is.
pub const MODEL: &str = "GEX_MAC_MODEL";

/// The settings a configuration has, and what each is.
pub const KEYS: [(&str, &str); 9] = [
    ("name", "the bottle's name"),
    ("system", "the system layer: a Mac root's directory, by its name outside the bottle"),
    ("uuid", "the machine's identifier (gethostuuid), made with the bottle"),
    ("serial", "the machine's serial number, made with the bottle"),
    ("macos", "the macOS its programs are told they are on: a release of the system layer's table (14, 14.8.9, sonoma); the layer's own without it"),
    ("machine", "the Mac its programs are told they are on: a model of the system layer's table (Mac14,3); the release's default without it"),
    ("graphics", "the graphics profile: honest, machine (the chosen Mac's GPU family), or a family (mac2, apple7, apple8)"),
    ("share.system", "whether the system outside is the volume /Volumes/Linux: yes or no"),
    ("share.home", "a directory outside that is the user's home in place of the bottle's own"),
];

/// The settings an application's section may have.
pub const APP_KEYS: [&str; 3] = ["macos", "machine", "graphics"];

/// A configuration: `key = value` lines, then a section for each
/// application that has settings of its own (`[app NAME]`, NAME the
/// program's file name or its bundle's without `.app`). Lines that begin
/// with `#` are comments; a writer keeps none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conf {
    pub top: Vec<(String, String)>,
    pub apps: Vec<(String, Vec<(String, String)>)>,
}

fn put(list: &mut Vec<(String, String)>, key: &str, value: Option<&str>) {
    let at = list.iter().position(|(k, _)| k == key);
    match (at, value.map(str::trim).filter(|v| !v.is_empty())) {
        (Some(i), Some(v)) => list[i].1 = v.to_string(),
        (Some(i), None) => {
            list.remove(i);
        }
        (None, Some(v)) => list.push((key.to_string(), v.to_string())),
        (None, None) => {}
    }
}

impl Conf {
    pub fn parse(text: &str) -> Conf {
        let mut conf = Conf::default();
        let mut app: Option<usize> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = head.trim().strip_prefix("app").map(str::trim).unwrap_or("");
                app = if name.is_empty() {
                    // A section nobody knows: its lines are nobody's.
                    Some(usize::MAX)
                } else {
                    let at = conf.apps.iter().position(|(n, _)| n == name).unwrap_or_else(|| {
                        conf.apps.push((name.to_string(), Vec::new()));
                        conf.apps.len() - 1
                    });
                    Some(at)
                };
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match app {
                None => put(&mut conf.top, k, Some(v)),
                Some(usize::MAX) => {}
                Some(i) => put(&mut conf.apps[i].1, k, Some(v)),
            }
        }
        conf
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for (k, v) in &self.top {
            s.push_str(&format!("{k} = {v}\n"));
        }
        for (name, list) in &self.apps {
            s.push_str(&format!("\n[app {name}]\n"));
            for (k, v) in list {
                s.push_str(&format!("{k} = {v}\n"));
            }
        }
        s
    }

    /// The bottle's own setting `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.top.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// Set the bottle's `key`, or with no value (or an empty one) forget it.
    pub fn set(&mut self, key: &str, value: Option<&str>) {
        put(&mut self.top, key, value);
    }

    /// The settings of the application `name`, if it has a section.
    pub fn app(&self, name: &str) -> Option<&[(String, String)]> {
        self.apps.iter().find(|(n, _)| n == name).map(|(_, l)| l.as_slice())
    }

    /// Set `key` for the application `name`, its section made if it has
    /// none (with no key: the section alone); with no value, forget it.
    pub fn set_app(&mut self, name: &str, key: Option<&str>, value: Option<&str>) {
        let at = self.apps.iter().position(|(n, _)| n == name).unwrap_or_else(|| {
            self.apps.push((name.to_string(), Vec::new()));
            self.apps.len() - 1
        });
        if let Some(key) = key {
            put(&mut self.apps[at].1, key, value);
        }
    }

    pub fn remove_app(&mut self, name: &str) -> bool {
        let n = self.apps.len();
        self.apps.retain(|(a, _)| a != name);
        self.apps.len() != n
    }

    /// The section of the program at `exe`, if an application's is its:
    /// one named as the program's file, or as a bundle the program is in
    /// (`TextEdit` for `/Applications/TextEdit.app/Contents/MacOS/anything`).
    pub fn section_of(&self, exe: &str) -> Option<&[(String, String)]> {
        let parts: Vec<&str> = exe.split('/').filter(|p| !p.is_empty()).collect();
        let file = parts.last().copied().unwrap_or("");
        self.apps
            .iter()
            .find(|(n, _)| n == file || parts.iter().any(|p| p.strip_suffix(".app") == Some(n.as_str())))
            .map(|(_, l)| l.as_slice())
    }

    /// The setting `key` as the program at `exe` has it: its application's
    /// own, else the bottle's.
    pub fn setting(&self, exe: &str, key: &str) -> Option<&str> {
        self.section_of(exe).and_then(|l| l.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())).or_else(|| self.get(key))
    }

    /// Whether the system outside is shared in as a volume: unless the
    /// bottle says no.
    pub fn shares_system(&self) -> bool {
        !matches!(self.get("share.system"), Some("no" | "false" | "0" | "off"))
    }
}

/// Why `key = value` is not a setting, if it is not one.
pub fn refuse(key: &str, value: &str, app: bool) -> Option<String> {
    let known = if app { APP_KEYS.contains(&key) } else { KEYS.iter().any(|(k, _)| *k == key) };
    if !known {
        let all: Vec<&str> = if app { APP_KEYS.to_vec() } else { KEYS.iter().map(|(k, _)| *k).collect() };
        return Some(format!("no setting {key} (there are: {})", all.join(", ")));
    }
    if value.contains(['\n', '\r']) || value.trim_start().starts_with('[') {
        return Some(format!("{key}: not a value that fits a line"));
    }
    match key {
        "share.system" if !["", "yes", "no"].contains(&value) => Some("share.system is yes or no".into()),
        "graphics" if !value.is_empty() && !value.chars().all(|c| c.is_ascii_alphanumeric()) => Some("graphics is a profile's name: honest, machine, mac2, apple7, apple8".into()),
        _ => None,
    }
}

/// Why `name` cannot name an application's section, if it cannot.
pub fn refuse_app(name: &str) -> Option<String> {
    (name.is_empty() || name.contains(['/', '[', ']', '\n', '\r']) || name != name.trim()).then(|| format!("{name:?} is not an application's name (its program's file name, or its bundle's without .app)"))
}

/// How a bottle is laid over its system layer: what is bound where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The system layer's directory, outside.
    pub system: String,
    /// (where in the root, which directory outside), each written through.
    pub binds: Vec<(String, String)>,
    /// Whether the system outside is the volume [`super::root::SHARED`]
    /// names.
    pub shares_system: bool,
}

impl Layout {
    /// The layout of the bottle at `bottle` (its directory outside, no
    /// trailing slash) with configuration `conf`: each of [`PARTS`] the
    /// bottle has (`there` says whether a directory outside is one), and
    /// the home it shares in, at the home `passwd` (the system layer's
    /// `/private/etc/passwd`) gives its user. `None` without a system
    /// layer: `fallback`, where the configuration names none.
    pub fn of(conf: &Conf, bottle: &str, fallback: Option<&str>, passwd: &str, there: &dyn Fn(&str) -> bool) -> Option<Layout> {
        let system = conf.get("system").or(fallback)?.trim_end_matches('/').to_string();
        let system = if system.is_empty() { "/".to_string() } else { system };
        let mut binds: Vec<(String, String)> = PARTS.iter().map(|p| (p.to_string(), format!("{bottle}{p}"))).filter(|(_, out)| there(out)).collect();
        if let Some(home) = conf.get("share.home").map(|h| h.trim_end_matches('/')).filter(|h| !h.is_empty() && there(h)) {
            binds.push((home_of(passwd), home.to_string()));
        }
        Some(Layout { system, binds, shares_system: conf.shares_system() })
    }

    /// The name outside of `inner`, a name in the bottle's root: through
    /// the bind that holds it most closely, else in the system layer.
    pub fn outer(&self, inner: &str) -> String {
        let holds = |at: &str| inner == at || inner.strip_prefix(at).is_some_and(|r| r.starts_with('/'));
        match self.binds.iter().filter(|(at, _)| holds(at)).max_by_key(|(at, _)| at.len()) {
            Some((at, out)) => format!("{out}{}", &inner[at.len()..]),
            None if self.system == "/" => inner.to_string(),
            None => format!("{}{inner}", self.system),
        }
    }
}

/// The first person's home, as a Mac root's `passwd` names it: the user
/// 501's (whom the kernel says a program is run by), `/Users/user` where
/// the file has none.
pub fn home_of(passwd: &str) -> String {
    passwd
        .lines()
        .filter(|l| !l.starts_with('#'))
        .map(|l| l.split(':').collect::<Vec<_>>())
        .find(|f| f.get(2) == Some(&"501"))
        .and_then(|f| f.get(5).map(|h| h.to_string()))
        .filter(|h| h.starts_with('/'))
        .unwrap_or_else(|| "/Users/user".to_string())
}

/// A row of [`MODELS`]: a Mac Apple made, and what it says of itself where
/// that is known.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Model {
    /// `hw.model`: "Mac14,3".
    pub model: String,
    /// "arm64" or "x86_64".
    pub arch: String,
    /// `hw.target`: the board, "J473AP"; an Intel Mac's board id.
    pub target: Option<String>,
    /// `hw.targettype`: "J473".
    pub targettype: Option<String>,
    /// What `kern.version` says the kernel was built for: "T8112".
    pub soc: Option<String>,
    /// `hw.cpufamily` and `hw.cpusubfamily`.
    pub cpufamily: Option<u32>,
    pub cpusubfamily: Option<u32>,
    /// Performance and efficiency cores, and logical processors.
    pub pcores: Option<u32>,
    pub ecores: Option<u32>,
    pub threads: Option<u32>,
    /// `machdep.cpu.brand_string`: "Apple M2".
    pub brand: Option<String>,
    /// The first macOS it runs, and the last (none: the latest still does).
    pub first: Option<String>,
    pub last: Option<String>,
    /// Its GPU's Metal family: "Apple8".
    pub gpu: Option<String>,
    /// What Apple calls it: "Mac mini (2023)".
    pub name: Option<String>,
}

/// The Macs a root without a table is told it is one of: the two the
/// kernel named before there was one.
pub const BUILTIN: &str = "Mac14,3\tarm64\tJ473AP\tJ473\tT8112\t0xda33d83d\t2\t-\t-\t-\t-\t-\t-\t-\t-\nMacBookPro16,1\tx86_64\tMac-E1008331FDC96864\t-\t-\t0x37fc219f\t0\t-\t-\t-\t-\t-\t-\t-\t-\n";

fn number(f: &str) -> Option<u32> {
    match f.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok(),
        None => f.parse().ok(),
    }
}

/// (major, minor) of a version, what it leaves out zero.
fn version(v: &str) -> (u32, u32) {
    let mut p = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (p.next().unwrap_or(0), p.next().unwrap_or(0))
}

impl Model {
    /// The rows of the table's `text`.
    pub fn parse(text: &str) -> Vec<Model> {
        let known = |f: Option<&&str>| f.map(|f| f.trim()).filter(|f| !f.is_empty() && *f != "-").map(String::from);
        text.lines()
            .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\t').collect();
                let num = |i: usize| known(f.get(i)).and_then(|v| number(&v));
                Some(Model {
                    model: known(f.first())?,
                    arch: known(f.get(1))?,
                    target: known(f.get(2)),
                    targettype: known(f.get(3)),
                    soc: known(f.get(4)),
                    cpufamily: num(5),
                    cpusubfamily: num(6),
                    pcores: num(7),
                    ecores: num(8),
                    threads: num(9),
                    brand: known(f.get(10)),
                    first: known(f.get(11)),
                    last: known(f.get(12)),
                    gpu: known(f.get(13)),
                    name: known(f.get(14)),
                })
            })
            .collect()
    }

    /// Whether this Mac runs macOS `product`: not one from before it
    /// shipped, nor one after its last. (What the row does not know does
    /// not rule it out.)
    pub fn runs(&self, product: &str) -> bool {
        let at = version(product);
        self.first.as_deref().is_none_or(|f| at >= version(f)) && self.last.as_deref().is_none_or(|l| at.0 <= version(l).0)
    }

    /// The row a setting names: by its model, whatever the case.
    pub fn find<'a>(rows: &'a [Model], spec: &str) -> Option<&'a Model> {
        rows.iter().find(|m| m.model.eq_ignore_ascii_case(spec.trim()))
    }

    /// The Mac a process on macOS `product` is told it is on when nothing
    /// chose one: the first row of its processor's that runs that macOS
    /// (the table's order is the preference), so that none is told of a
    /// Mac older or newer than its system ran on; the first of its
    /// processor's when none does.
    pub fn default_for<'a>(rows: &'a [Model], arch: &str, product: &str) -> Option<&'a Model> {
        let mut of = rows.iter().filter(|m| m.arch == arch);
        let first = of.clone().next();
        of.find(|m| m.runs(product)).or(first)
    }

    /// The graphics profile `setting` comes to on this Mac: `machine` is
    /// its GPU's family, anything else itself, nothing `honest` (what the
    /// graphics layer can really do).
    pub fn graphics(&self, setting: Option<&str>) -> String {
        match setting.map(|s| s.trim().to_ascii_lowercase()) {
            Some(s) if s == "machine" => self.gpu.as_deref().map_or("honest".into(), str::to_ascii_lowercase),
            Some(s) if !s.is_empty() => s,
            _ => "honest".into(),
        }
    }
}

/// The 16 bytes of a UUID written as text (`gethostuuid`), if it is one.
pub fn uuid_bytes(text: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = text.bytes().filter(|b| *b != b'-').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in hex.chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(out)
}

/// A version 4 UUID and a serial number of `random` bytes (26 of them): a
/// new bottle's own, so that no two say they are one machine and none says
/// it is the machine it runs on.
pub fn identifiers(random: &[u8; 26]) -> (String, String) {
    let mut b = [0u8; 16];
    b.copy_from_slice(&random[..16]);
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02X}")).collect();
    let uuid = format!("{}-{}-{}-{}-{}", h[..4].concat(), h[4..6].concat(), h[6..8].concat(), h[8..10].concat(), h[10..].concat());
    // Ten of the letters and digits a serial number is written in.
    const LETTERS: &[u8] = b"0123456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let serial: String = random[16..].iter().map(|r| LETTERS[*r as usize % LETTERS.len()] as char).collect();
    (uuid, serial)
}

/// The applications a configuration names and a list of installed ones,
/// together, each once, in order.
pub fn applications(conf: &Conf, installed: &[String]) -> Vec<String> {
    let all: BTreeSet<String> = conf.apps.iter().map(|(n, _)| n.clone()).chain(installed.iter().cloned()).collect();
    all.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "# a bottle\nname = Work\nsystem = /opt/mac\nmacos = 14\nmachine = Mac14,3\nshare.system = no\n\n[app TextEdit]\nmacos = 11\nmachine = Macmini9,1\n\n[app sw_vers]\nmacos = 26.5\n";

    #[test]
    fn a_configuration_is_read_and_written_as_it_was() {
        let c = Conf::parse(TEXT);
        assert_eq!(c.get("name"), Some("Work"));
        assert_eq!(c.get("graphics"), None);
        assert!(!c.shares_system());
        assert_eq!(c.apps.len(), 2);
        assert_eq!(Conf::parse(&c.text()), c);
        let mut d = c.clone();
        d.set("macos", None);
        d.set("graphics", Some("apple8"));
        d.set_app("Demo", Some("graphics"), Some("honest"));
        d.set_app("TextEdit", Some("macos"), Some(""));
        assert!(d.remove_app("sw_vers") && !d.remove_app("sw_vers"));
        assert_eq!(d.text(), "name = Work\nsystem = /opt/mac\nmachine = Mac14,3\nshare.system = no\ngraphics = apple8\n\n[app TextEdit]\nmachine = Macmini9,1\n\n[app Demo]\ngraphics = honest\n");
    }

    #[test]
    fn an_application_has_its_own_settings_over_the_bottles() {
        let c = Conf::parse(TEXT);
        assert_eq!(c.setting("/bin/ls", "macos"), Some("14"));
        assert_eq!(c.setting("/usr/bin/sw_vers", "macos"), Some("26.5"));
        assert_eq!(c.setting("/usr/bin/sw_vers", "machine"), Some("Mac14,3"));
        assert_eq!(c.setting("/Applications/TextEdit.app/Contents/MacOS/TextEdit", "macos"), Some("11"));
        assert_eq!(c.setting("/Applications/TextEdit.app/Contents/Helpers/tool", "machine"), Some("Macmini9,1"));
        assert_eq!(c.setting("/Applications/TextEdit", "graphics"), None);
    }

    #[test]
    fn a_setting_nobody_knows_is_refused() {
        assert!(refuse("macos", "14", false).is_none() && refuse("macos", "14", true).is_none());
        assert!(refuse("name", "x", true).is_some());
        assert!(refuse("colour", "red", false).is_some());
        assert!(refuse("share.system", "maybe", false).is_some());
        assert!(refuse("name", "a\nb", false).is_some());
        assert!(refuse_app("a/b").is_some() && refuse_app("TextEdit").is_none());
    }

    #[test]
    fn a_bottle_is_laid_over_its_system_by_its_parts() {
        let c = Conf::parse("system = /opt/mac\nshare.home = /home/me\n");
        let there = |p: &str| p != "/b/one/Applications";
        let l = Layout::of(&c, "/b/one", None, "nobody:*:-2:-2::0:0:Unprivileged User:/var/empty:/usr/bin/false\nme:*:501:20:Me:/Users/me:/bin/zsh\n", &there).unwrap();
        assert_eq!(l.system, "/opt/mac");
        assert_eq!(l.binds, [("/Users", "/b/one/Users"), ("/Library", "/b/one/Library"), ("/private/var", "/b/one/private/var"), ("/Users/me", "/home/me")].map(|(a, b)| (a.to_string(), b.to_string())));
        assert!(l.shares_system);
        assert_eq!(l.outer("/Library/Preferences/x.plist"), "/b/one/Library/Preferences/x.plist");
        assert_eq!(l.outer("/Users/me/Desktop"), "/home/me/Desktop");
        assert_eq!(l.outer("/Users/other"), "/b/one/Users/other");
        assert_eq!(l.outer("/Applications/A.app"), "/opt/mac/Applications/A.app");
        assert_eq!(l.outer("/Librarian"), "/opt/mac/Librarian");
        assert_eq!(Layout::of(&Conf::default(), "/b", None, "", &there), None);
        assert_eq!(Layout::of(&Conf::default(), "/b", Some("/"), "", &|_| false).unwrap().outer("/bin/ls"), "/bin/ls");
        assert_eq!(home_of(""), "/Users/user");
    }

    const TABLE: &str = "#model\tarch\ttarget\ttargettype\tsoc\tcpufamily\tcpusubfamily\tpcores\tecores\tthreads\tbrand\tfirst\tlast\tgpufamily\tname\tsource\n\
Mac14,3\tarm64\tJ473AP\tJ473\tT8112\t0xda33d83d\t2\t4\t4\t8\tApple M2\t13.2\t-\tApple8\tMac mini (2023)\ta\n\
Macmini9,1\tarm64\tJ274AP\tJ274\tT8103\t0x1b588bb3\t2\t4\t4\t8\tApple M1\t11.0\t-\tApple7\tMac mini (M1, 2020)\tb\n\
MacBookPro16,1\tx86_64\tMac-E1008331FDC96864\t-\t-\t0x37fc219f\t0\t6\t0\t12\t-\t10.15.1\t26\t-\tMacBook Pro (16-inch, 2019)\tc\n";

    #[test]
    fn the_default_mac_is_one_its_macos_ran_on() {
        let rows = Model::parse(TABLE);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].cpufamily, Some(0xda33_d83d));
        assert_eq!(rows[2].targettype, None);
        let d = |arch: &str, v: &str| Model::default_for(&rows, arch, v).map(|m| m.model.as_str());
        assert_eq!(d("arm64", "27.0.1"), Some("Mac14,3"));
        assert_eq!(d("arm64", "13.2"), Some("Mac14,3"));
        assert_eq!(d("arm64", "13.1"), Some("Macmini9,1"));
        assert_eq!(d("arm64", "11.7.11"), Some("Macmini9,1"));
        // (None ran 10.15: the first, then.)
        assert_eq!(d("arm64", "10.15.7"), Some("Mac14,3"));
        assert_eq!(d("x86_64", "14.8.9"), Some("MacBookPro16,1"));
        assert_eq!(d("riscv", "14"), None);
        assert!(!rows[2].runs("27.0") && rows[2].runs("26.7.3") && rows[2].runs("10.15.7") && !rows[2].runs("10.14.6"));
        assert_eq!(Model::find(&rows, "mac14,3").map(|m| m.model.as_str()), Some("Mac14,3"));
        assert_eq!(rows[0].graphics(Some("machine")), "apple8");
        assert_eq!(rows[2].graphics(Some("machine")), "honest");
        assert_eq!(rows[0].graphics(None), "honest");
        assert_eq!(rows[0].graphics(Some("Apple7")), "apple7");
        assert_eq!(Model::parse(BUILTIN).len(), 2);
    }

    #[test]
    fn a_bottle_has_identifiers_of_its_own() {
        let (uuid, serial) = identifiers(&[0xab; 26]);
        assert_eq!(uuid, "ABABABAB-ABAB-4BAB-ABAB-ABABABABABAB");
        assert_eq!(serial.len(), 10);
        assert_eq!(uuid_bytes(&uuid).unwrap()[6], 0x4b);
        assert_eq!(uuid_bytes("gex"), None);
    }
}
