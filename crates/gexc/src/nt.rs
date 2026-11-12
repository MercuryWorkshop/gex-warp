//! `gexc run app.exe`: a Windows program is run by Wine, the way a Mach-O
//! program is run by the Darwin personality (it is not told apart by a
//! flag, but by what the file is).
//!
//! Wine's loader (`lib/wine/x86_64-unix/wine` of a Wine dist) is the guest
//! program, and the .exe is its argument, with the environment the demo and
//! the tests give Wine. A 32-bit program runs in the same Wine (WoW64).
//!
//! Where the pieces are, by option, then environment, then the cache that
//! `wine/mac-export.sh DIR wow64` fills:
//!   dist      --wine-dist DIR    GEX_WINE_DIST    ~/.cache/gex/wine-on-mac/export-wow64/dist
//!   prefix    --wine-prefix DIR  GEX_WINE_PREFIX  ~/.gex/wine/default
//! A prefix that does not exist yet is made as a copy of the template
//! (GEX_WINE_TEMPLATE, by default the `pfx` beside the dist), so the
//! exported one stays as it was.
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// The Wine dist's loader.
pub const LOADER: &str = "lib/wine/x86_64-unix/wine";

#[derive(Default)]
pub struct Wine {
    pub dist: Option<String>,
    pub prefix: Option<String>,
}

/// Whether `head` (a file's first bytes, or all of it) is a PE image: MZ,
/// and the PE signature where e_lfanew says.
pub fn is_pe(head: &[u8]) -> bool {
    if head.len() < 0x40 || &head[..2] != b"MZ" {
        return false;
    }
    let at = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as usize;
    head.get(at..at + 4) == Some(b"PE\0\0")
}

/// Whether the file at `path` is a PE image.
pub fn file_is_pe(path: &str) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = Vec::new();
    // (e_lfanew is nearly always within the first 4 KiB)
    f.by_ref().take(4096).read_to_end(&mut head).is_ok() && is_pe(&head)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

fn env_dir(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// What to run for the PE program `exe`: the Wine loader, its arguments
/// (the program first) and the environment it needs.
pub fn command(w: &Wine, exe: &str, env: &mut Vec<String>) -> Result<(String, Vec<String>)> {
    let cache = home().join(".cache/gex/wine-on-mac/export-wow64");
    let dist = match w.dist.clone().or_else(|| env_dir("GEX_WINE_DIST")) {
        Some(d) => PathBuf::from(d),
        None => cache.join("dist"),
    };
    let loader = dist.join(LOADER);
    if !loader.is_file() {
        bail!(
            "{exe} is a Windows program, which runs in Wine, and {} is not there: \
             --wine-dist DIR or GEX_WINE_DIST (a Wine dist, wine/mac-export.sh DIR wow64)",
            loader.display()
        );
    }
    let prefix = match w.prefix.clone().or_else(|| env_dir("GEX_WINE_PREFIX")) {
        Some(p) => PathBuf::from(p),
        None => home().join(".gex/wine/default"),
    };
    if !prefix.exists() {
        let template = match env_dir("GEX_WINE_TEMPLATE") {
            Some(t) => PathBuf::from(t),
            None => dist.parent().map(|p| p.join("pfx")).unwrap_or_else(|| cache.join("pfx")),
        };
        make_prefix(&prefix, &template)?;
    }
    let exe = std::fs::canonicalize(exe).with_context(|| format!("reading {exe}"))?;
    let mut set = |k: &str, v: String| {
        if !env.iter().any(|e| e.split_once('=').is_some_and(|(n, _)| n == k)) {
            env.push(format!("{k}={v}"));
        }
    };
    set("WINEPREFIX", prefix.to_string_lossy().into_owned());
    set("WINEDEBUG", "err+all,fixme-all".into());
    set("WINEGEX_DOOR", "1".into());
    set("WINEGEX_WOW64", "1".into());
    Ok((loader.to_string_lossy().into_owned(), vec![exe.to_string_lossy().into_owned()]))
}

/// A new prefix: a copy of the template (a Wine prefix is only files).
fn make_prefix(prefix: &Path, template: &Path) -> Result<()> {
    if !template.is_dir() {
        bail!(
            "the Wine prefix {} does not exist, and neither does the template to make it from ({}): \
             GEX_WINE_TEMPLATE, or an existing --wine-prefix",
            prefix.display(),
            template.display()
        );
    }
    if let Some(parent) = prefix.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ok = std::process::Command::new("cp").arg("-Rp").arg(template).arg(prefix).status()?.success();
    anyhow::ensure!(ok, "copying {} to {}", template.display(), prefix.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pe_is_told_by_its_signature() {
        let mut v = vec![0u8; 0x80];
        v[..2].copy_from_slice(b"MZ");
        v[0x3c] = 0x40;
        assert!(!super::is_pe(&v));
        v[0x40..0x44].copy_from_slice(b"PE\0\0");
        assert!(super::is_pe(&v));
        assert!(!super::is_pe(b"\x7fELF"));
    }
}
