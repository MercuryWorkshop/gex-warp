//! `gexc bottle`: bottles on this machine's disk, made, listed, configured
//! and deleted, and a program run in one.
//!
//! What a bottle is, how it is laid over its system layer and what its
//! configuration says are the kernel's (`gex_kernel::darwin::bottle`);
//! this is the files. The page's runtime has the same commands as an API
//! (`Bottles`, page/runtime/src/bottles.js), over the page's filesystem.
use anyhow::{bail, ensure, Context, Result};
use gex_kernel::darwin::bottle::{self, Conf, Layout, Model};
use gex_kernel::darwin::info::{Release, RELEASES};
use std::path::{Path, PathBuf};

pub fn usage() -> &'static str {
    "  gexc bottle create NAME [--system ROOT] [KEY=VALUE ...]
                                   a new bottle over the Mac root ROOT (by
                                   default GEX_DARWIN_ROOT's): its own
                                   /Users, /Applications, /Library and
                                   /private/var, copies of ROOT's, and a
                                   configuration with identifiers of its own
  gexc bottle list                 the bottles there are
  gexc bottle delete NAME          a bottle and everything in it, gone
  gexc bottle show NAME [--app APP] [KEY]
                                   its configuration, or one setting
  gexc bottle set NAME [--app APP] KEY=VALUE ...
                                   settings (KEY= forgets one): macos,
                                   machine, graphics, share.system,
                                   share.home; with --app, the application's
                                   own (macos, machine, graphics)
  gexc bottle app NAME add|remove APP
                                   an application's section, made or dropped
  gexc bottle machines NAME        the Macs its system layer's table has
  gexc bottle run NAME [run options] PROGRAM [args...]
                                   run PROGRAM, a path in the bottle
                                   (/bin/zsh, /Applications/A.app/Contents/MacOS/A)
                                   NAME is a bottle of GEX_BOTTLES (by default
                                   ~/.gex/bottles), or with a / in it a
                                   directory"
}

/// Where the bottles named by a name alone are.
fn home() -> PathBuf {
    match std::env::var_os("GEX_BOTTLES").filter(|v| !v.is_empty()) {
        Some(d) => d.into(),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".gex/bottles"),
    }
}

/// The directory of the bottle `name`.
pub fn dir_of(name: &str) -> PathBuf {
    if name.contains('/') { PathBuf::from(name) } else { home().join(name) }
}

fn conf_path(dir: &Path) -> PathBuf {
    dir.join(&bottle::CONF[1..])
}

pub fn read_conf(dir: &Path) -> Result<Conf> {
    let text = std::fs::read_to_string(conf_path(dir)).with_context(|| format!("{}: no bottle there (it has no {})", dir.display(), &bottle::CONF[1..]))?;
    Ok(Conf::parse(&text))
}

fn write_conf(dir: &Path, conf: &Conf) -> Result<()> {
    let path = conf_path(dir);
    std::fs::create_dir_all(path.parent().unwrap())?;
    // Whole or not at all: a program starting meanwhile reads one or the other.
    let tmp = path.with_extension("conf.new");
    std::fs::write(&tmp, conf.text()).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
}

/// A tree copied: directories, files with their modes, links as links.
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let md = std::fs::symlink_metadata(from).with_context(|| format!("{}", from.display()))?;
    if md.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(from)?, to).with_context(|| format!("{}", to.display()))?;
    } else if md.is_dir() {
        std::fs::create_dir_all(to).with_context(|| format!("{}", to.display()))?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        std::fs::set_permissions(to, md.permissions())?;
    } else if md.is_file() {
        std::fs::copy(from, to).with_context(|| format!("{}", to.display()))?;
    }
    Ok(())
}

/// The system layer's tables: its releases and its Macs.
fn tables(system: &Path) -> (Vec<Release>, Vec<Model>) {
    let read = |p: &str| std::fs::read_to_string(system.join(&p[1..])).unwrap_or_default();
    let models = Model::parse(&read(bottle::MODELS));
    (Release::parse(&read(RELEASES)), if models.is_empty() { Model::parse(bottle::BUILTIN) } else { models })
}

/// `key = value` checked: a setting there is, and a release or a Mac the
/// system layer's tables have.
fn check(system: &Path, key: &str, value: &str, app: bool) -> Result<()> {
    if let Some(why) = bottle::refuse(key, value, app) {
        bail!("{why}");
    }
    let (releases, models) = tables(system);
    match key {
        "macos" if !value.is_empty() && Release::find(&releases, value).is_none() => bail!("macos = {value}: no such release in the system layer's table (it has {})", releases.iter().map(|r| r.product.as_str()).collect::<Vec<_>>().join(" ")),
        "machine" if !value.is_empty() && Model::find(&models, value).is_none() => bail!("machine = {value}: no such Mac in the system layer's table (it has {})", models.iter().map(|m| m.model.as_str()).collect::<Vec<_>>().join(" ")),
        "system" => ensure!(Path::new(value).join("usr/lib/dyld").exists(), "system = {value}: not a Mac root (no usr/lib/dyld there)"),
        "share.home" if !value.is_empty() => ensure!(Path::new(value).is_dir(), "share.home = {value}: not a directory"),
        _ => {}
    }
    Ok(())
}

fn system_of(dir: &Path, conf: &Conf) -> Result<PathBuf> {
    let system = conf.get("system").with_context(|| format!("{}: its configuration names no system layer (system = ROOT)", dir.display()))?;
    Ok(PathBuf::from(system))
}

fn sets<'a>(args: &'a [String]) -> Result<Vec<(&'a str, &'a str)>> {
    args.iter().map(|a| a.split_once('=').with_context(|| format!("{a}: want KEY=VALUE"))).collect()
}

fn create(name: &str, args: &[String]) -> Result<()> {
    let mut system = std::env::var("GEX_DARWIN_ROOT").ok().filter(|s| !s.is_empty());
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--system" {
            i += 1;
            system = Some(args.get(i).context("--system needs a Mac root")?.clone());
        } else {
            rest.push(args[i].clone());
        }
        i += 1;
    }
    let system = system.context("which Mac root is the bottle over? (--system ROOT, or GEX_DARWIN_ROOT)")?;
    let system = std::fs::canonicalize(&system).with_context(|| format!("--system {system}"))?;
    ensure!(system.join("usr/lib/dyld").exists(), "{}: not a Mac root (no usr/lib/dyld there)", system.display());
    let dir = dir_of(name);
    ensure!(!dir.exists(), "{}: there already", dir.display());
    std::fs::create_dir_all(&dir).with_context(|| format!("{}", dir.display()))?;
    let dir = std::fs::canonicalize(&dir)?;
    for part in bottle::PARTS {
        let (from, to) = (system.join(&part[1..]), dir.join(&part[1..]));
        if from.is_dir() {
            if let Some(up) = to.parent() {
                std::fs::create_dir_all(up)?;
            }
            copy_tree(&from, &to)?;
        } else {
            std::fs::create_dir_all(&to)?;
        }
    }
    // The first person's home, which a root as built has not.
    let passwd = std::fs::read_to_string(system.join("private/etc/passwd")).unwrap_or_default();
    let home = bottle::home_of(&passwd);
    if home.starts_with("/Users/") {
        std::fs::create_dir_all(dir.join(&home[1..]))?;
    }
    // The root's own configuration, where it had one, with what is this
    // bottle's alone.
    let mut conf = read_conf(&dir).unwrap_or_default();
    let mut random = [0u8; 26];
    std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut random)?;
    let (uuid, serial) = bottle::identifiers(&random);
    let named = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for (k, v) in [("name", named.as_str()), ("system", &system.to_string_lossy()), ("uuid", &uuid), ("serial", &serial)] {
        conf.set(k, Some(v));
    }
    for (k, v) in sets(&rest)? {
        if let Err(e) = check(&system, k, v, false) {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        conf.set(k, Some(v));
    }
    write_conf(&dir, &conf)?;
    println!("{}", dir.display());
    Ok(())
}

/// `gexc bottle ...`: `Some(args)` when what is left to do is `gexc run`
/// with those arguments.
pub fn cmd(args: &[String]) -> Result<Option<Vec<String>>> {
    let name = |i: usize| args.get(i).map(String::as_str).with_context(|| format!("gexc bottle {}: which bottle?", args[0]));
    match args.first().map(String::as_str) {
        Some("create") => create(name(1)?, &args[2..])?,
        Some("list") => {
            let mut names: Vec<_> = std::fs::read_dir(home()).map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| conf_path(p).exists()).collect()).unwrap_or_default();
            names.sort();
            for dir in names {
                let conf = read_conf(&dir)?;
                let get = |k: &str| conf.get(k).unwrap_or("-").to_string();
                println!("{}\t{}\t{}\t{}", dir.file_name().unwrap_or_default().to_string_lossy(), get("macos"), get("machine"), get("system"));
            }
        }
        Some("delete") => {
            let dir = dir_of(name(1)?);
            read_conf(&dir)?;
            std::fs::remove_dir_all(&dir).with_context(|| format!("{}", dir.display()))?;
        }
        Some(what @ ("show" | "set")) => {
            let dir = dir_of(name(1)?);
            let mut conf = read_conf(&dir)?;
            let (app, rest) = match args.get(2).map(String::as_str) {
                Some("--app") => (Some(args.get(3).context("--app needs an application's name")?.as_str()), &args[4.min(args.len())..]),
                _ => (None, &args[2..]),
            };
            if let Some(why) = app.and_then(bottle::refuse_app) {
                bail!("{why}");
            }
            if what == "show" {
                match (app, rest.first()) {
                    (None, None) => print!("{}", conf.text()),
                    (Some(a), None) => conf.app(a).with_context(|| format!("no application {a} in the configuration"))?.iter().for_each(|(k, v)| println!("{k} = {v}")),
                    (None, Some(k)) => println!("{}", conf.get(k).unwrap_or("")),
                    (Some(a), Some(k)) => println!("{}", conf.app(a).and_then(|l| l.iter().find(|(x, _)| x == k)).map_or("", |(_, v)| v.as_str())),
                }
            } else {
                ensure!(!rest.is_empty(), "gexc bottle set: KEY=VALUE ...");
                let system = system_of(&dir, &conf)?;
                for (k, v) in sets(rest)? {
                    check(&system, k, v, app.is_some())?;
                    match app {
                        Some(a) => conf.set_app(a, Some(k), Some(v)),
                        None if matches!(k, "system") && v.is_empty() => bail!("a bottle has a system layer"),
                        None => conf.set(k, Some(v)),
                    }
                }
                write_conf(&dir, &conf)?;
            }
        }
        Some("app") => {
            let dir = dir_of(name(1)?);
            let mut conf = read_conf(&dir)?;
            let (what, app) = (args.get(2).map(String::as_str), args.get(3).context("gexc bottle app NAME add|remove APP")?);
            if let Some(why) = bottle::refuse_app(app) {
                bail!("{why}");
            }
            match what {
                Some("add") => conf.set_app(app, None, None),
                Some("remove") => ensure!(conf.remove_app(app), "no application {app} in the configuration"),
                _ => bail!("gexc bottle app NAME add|remove APP"),
            }
            write_conf(&dir, &conf)?;
        }
        Some("machines") => {
            let dir = dir_of(name(1)?);
            let conf = read_conf(&dir)?;
            let (_, models) = tables(&system_of(&dir, &conf)?);
            let known = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".into());
            for m in models {
                println!("{}\t{}\t{}\t{}\t{}\t{}", m.model, m.arch, known(&m.soc), known(&m.first), known(&m.last), known(&m.name));
            }
        }
        Some("run") => {
            let dir = dir_of(name(1)?);
            read_conf(&dir)?;
            let mut run = vec!["--bottle".to_string(), dir.to_string_lossy().into_owned(), "--guest".to_string()];
            run.extend_from_slice(&args[2..]);
            return Ok(Some(run));
        }
        _ => bail!("usage:\n{}", usage()),
    }
    Ok(None)
}

/// `gexc run --bottle DIR`: the run's root is the bottle's system layer,
/// not to be written, with the bottle's directories bound over it and the
/// user's home the bottle's.
pub fn apply(dir: &str, opts: &mut gex_native::RunOptions) -> Result<()> {
    let dir = std::fs::canonicalize(dir).with_context(|| format!("--bottle {dir}"))?;
    let conf = read_conf(&dir)?;
    let fallback = opts.sysroot.as_ref().map(|p| p.to_string_lossy().into_owned());
    let system = PathBuf::from(conf.get("system").map(String::from).or(fallback.clone()).with_context(|| format!("{}: its configuration names no system layer, and no --sysroot does", dir.display()))?);
    let system = std::fs::canonicalize(&system).with_context(|| format!("{}: its system layer {}", dir.display(), system.display()))?;
    let passwd = std::fs::read_to_string(system.join("private/etc/passwd")).unwrap_or_default();
    let layout = Layout::of(&conf, &dir.to_string_lossy(), Some(&system.to_string_lossy()), &passwd, &|p| Path::new(p).is_dir()).context("no system layer")?;
    opts.sysroot = Some(system);
    opts.read_only.push("/".to_string());
    for (inside, outside) in &layout.binds {
        opts.binds.push((PathBuf::from(outside), inside.clone()));
    }
    if layout.shares_system {
        let (inside, outside) = gex_kernel::darwin::root::SHARED[2];
        opts.binds.push((PathBuf::from(outside), inside.to_string()));
    }
    for (k, v) in [("HOME", bottle::home_of(&passwd)), ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin".to_string())] {
        if !opts.env.iter().any(|e| e.starts_with(&format!("{k}="))) {
            opts.env.push(format!("{k}={v}"));
        }
    }
    Ok(())
}
