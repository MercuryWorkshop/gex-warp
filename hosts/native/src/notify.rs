//! The notifications service of `/dev/gex-services` (`crate::services`),
//! natively: a stand-in for a shell that presents notifications, for tests.
//!
//! Offered when gexc's own environment says the shell presents
//! notifications (`GEX_NOTIFY=1`, or a script in `GEX_NOTIFY_INPUT`), as
//! `GEX_WL_MENUS=1` is for menus; without it a program's framework is told
//! the service is not there, and behaves as a Mac's with notifications
//! denied. Its messages are the service's (page/runtime/README.md,
//! Notifications, is their reference). It prints what a shell would be
//! given, one `[notify]` line (and indented lines under it) each, on gexc's
//! standard error; checks what a program sends as a shell must (lengths
//! bounded, text cut, images size-checked and their type sniffed); and
//! answers from a script ([`Script`]): the authorization decision, and what
//! the user does with each notification shown. Its thread carries out the
//! answers that wait.

use crate::services::{image, lock, quote, text, Json, Link, Service, Session};
use std::fmt::Write as _;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

/// Whether this host presents notifications: `GEX_NOTIFY=1`, or a script.
pub fn presented() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("GEX_NOTIFY").is_ok_and(|v| !v.is_empty() && v != "0") || std::env::var_os("GEX_NOTIFY_INPUT").is_some_and(|v| !v.is_empty())
    })
}

// What a shell takes of what a program sends; the page runtime's are the
// same (page/runtime/src/notifications.js).
const ICON_MAX: usize = 256 << 10;
const IMAGE_MAX: usize = 2 << 20;
const ID_MAX: usize = 256;
const TITLE_MAX: usize = 1024;
const BODY_MAX: usize = 8192;
const ACTIONS_MAX: usize = 10;
const OUTSTANDING_MAX: usize = 100;
const BADGE_LABEL_MAX: usize = 16;

/// The service.
pub struct Notifications;

impl Service for Notifications {
    fn name(&self) -> &'static str {
        "notifications"
    }
    fn version(&self) -> u32 {
        1
    }
    fn open(&self, link: Link, _version: u32) -> Box<dyn Session> {
        let (m, _) = shell();
        let mut g = lock(m);
        let id = g.next;
        g.next += 1;
        let app = Arc::new(Mutex::new(App { link: Some(link), ..App::default() }));
        g.apps.retain(|(_, w)| w.strong_count() > 0);
        g.apps.push((id, Arc::downgrade(&app)));
        Box::new(Connection { id, app })
    }
}

// ---------------------------------------------------------------------------
// The script: the shell's answers.

/// The answers of the stand-in shell (`GEX_NOTIFY_INPUT`), one per line, `#`
/// comments:
///
/// ```text
/// authorize grant                  # the decision for every application (the default)
/// authorize deny com.example.App   # or for one: grant, deny or provisional
/// on hello activate                # when the notification with identifier `hello`
///                                  # is shown: its default action
/// on hello after 1.5 activate reply text Thanks, see you   # a named action, with typed text
/// on * dismiss                     # any notification: the user closed it
/// on bye clear                     # the shell took it away (told as cleared)
/// on bye settings                  # the user asked for the application's settings
/// at 3 clear hello                 # seconds after the first connection: verb, identifier
/// at 4 settings                    # (settings for no notification in particular)
/// ```
///
/// Each `on` line answers once, the first notification shown that it names;
/// lines naming the same identifier answer its showings in turn, a
/// `settings` line with the line after it (asking for the settings leaves
/// the notification shown).
#[derive(Debug, Default)]
struct Script {
    /// The default decision, and each application's.
    authorize: Option<Decision>,
    apps: Vec<(String, Decision)>,
    /// `on` lines: identifier (`*` any), delay, answer; and whether used.
    on: Vec<(String, f64, Answer, bool)>,
    /// `at` lines: seconds, answer, identifier (empty for none).
    at: Vec<(f64, Answer, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Decision {
    Grant,
    Deny,
    Provisional,
}

#[derive(Debug, Clone)]
enum Answer {
    Activate { action: Option<String>, text: Option<String> },
    Dismiss,
    Clear,
    Settings,
}

impl Script {
    fn load(path: &str) -> Result<Script, String> {
        let body = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut s = Script::default();
        for (n, line) in body.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let bad = || format!("{path}:{}: cannot read `{line}`", n + 1);
            let w: Vec<&str> = line.split_whitespace().collect();
            let decision = |v: &str| match v {
                "grant" | "yes" => Some(Decision::Grant),
                "deny" | "no" => Some(Decision::Deny),
                "provisional" => Some(Decision::Provisional),
                _ => None,
            };
            let answer = |w: &[&str]| -> Option<Answer> {
                match *w.first()? {
                    "activate" => {
                        let mut action = None;
                        let mut rest = &w[1..];
                        if let Some(a) = rest.first().filter(|a| **a != "text") {
                            action = Some(a.to_string());
                            rest = &rest[1..];
                        }
                        let text = match rest.first() {
                            Some(&"text") => Some(rest[1..].join(" ")),
                            None => None,
                            Some(_) => return None,
                        };
                        Some(Answer::Activate { action, text })
                    }
                    "dismiss" if w.len() == 1 => Some(Answer::Dismiss),
                    "clear" if w.len() == 1 => Some(Answer::Clear),
                    "settings" if w.len() == 1 => Some(Answer::Settings),
                    _ => None,
                }
            };
            match w[0] {
                "authorize" => {
                    let d = w.get(1).and_then(|v| decision(v)).ok_or_else(bad)?;
                    match w.get(2) {
                        Some(app) => s.apps.push((app.to_string(), d)),
                        None => s.authorize = Some(d),
                    }
                }
                "on" => {
                    let id = w.get(1).ok_or_else(bad)?.to_string();
                    let (delay, rest) = if w.get(2) == Some(&"after") {
                        (w.get(3).and_then(|v| v.parse().ok()).ok_or_else(bad)?, &w[4..])
                    } else {
                        (0.0, &w[2.min(w.len())..])
                    };
                    s.on.push((id, delay, answer(rest).ok_or_else(bad)?, false));
                }
                "at" => {
                    let t: f64 = w.get(1).and_then(|v| v.parse().ok()).ok_or_else(bad)?;
                    // The verb's words, then (for all but activate's own) an identifier.
                    let (verb, id) = match w.get(2) {
                        Some(&("dismiss" | "clear")) => (&w[2..3], w.get(3).ok_or_else(bad)?.to_string()),
                        Some(&"settings") => (&w[2..3], w.get(3).map(|v| v.to_string()).unwrap_or_default()),
                        Some(&"activate") => {
                            let id = w.get(3).ok_or_else(bad)?.to_string();
                            let mut v = vec!["activate"];
                            v.extend_from_slice(&w[4..]);
                            s.at.push((t, answer(&v).ok_or_else(bad)?, id));
                            continue;
                        }
                        _ => return Err(bad()),
                    };
                    s.at.push((t, answer(verb).ok_or_else(bad)?, id));
                }
                _ => return Err(bad()),
            }
        }
        Ok(s)
    }

    fn decision(&self, app: &str) -> Decision {
        self.apps.iter().rev().find(|(a, _)| a == app).map(|(_, d)| *d).or(self.authorize).unwrap_or(Decision::Grant)
    }
}

// ---------------------------------------------------------------------------
// The shell: every application connected, what is outstanding, the script,
// and a thread for the answers that wait.

#[derive(Default)]
struct App {
    link: Option<Link>,
    /// The application, from its `hello`.
    app: String,
    hello: bool,
    /// Identifiers of the notifications outstanding, oldest first.
    outstanding: Vec<String>,
}

impl App {
    fn send(&self, json: &str) {
        if let Some(l) = &self.link {
            l.send(json, &[]);
        }
    }
}

/// One program's connection to the service.
struct Connection {
    id: u64,
    app: Arc<Mutex<App>>,
}

struct Shell {
    script: Script,
    apps: Vec<(u64, Weak<Mutex<App>>)>,
    next: u64,
    /// When the first program said hello: `at` lines count from it.
    start: Option<Instant>,
    /// Answers due: when, which connection (0: whichever has the
    /// identifier), the identifier, what.
    due: Vec<(Instant, u64, String, Answer)>,
    /// Decisions made, by application (`authorize` asks once).
    decided: Vec<(String, Decision)>,
}

fn shell() -> &'static (Mutex<Shell>, Condvar) {
    static SHELL: OnceLock<(Mutex<Shell>, Condvar)> = OnceLock::new();
    SHELL.get_or_init(|| {
        let script = match std::env::var("GEX_NOTIFY_INPUT") {
            Ok(p) if !p.is_empty() => Script::load(&p).unwrap_or_else(|e| {
                eprintln!("[gex] GEX_NOTIFY_INPUT: {e}");
                Script::default()
            }),
            _ => Script::default(),
        };
        let _ = std::thread::Builder::new().name("gex-notify".into()).spawn(answer_loop);
        (Mutex::new(Shell { script, apps: Vec::new(), next: 1, start: None, due: Vec::new(), decided: Vec::new() }), Condvar::new())
    })
}

fn say(line: &str) {
    eprintln!("[notify] {line}");
}

/// Carry out the answers as they fall due.
fn answer_loop() {
    let (m, cv) = shell();
    let mut g = lock(m);
    loop {
        let now = Instant::now();
        g.due.sort_by_key(|d| d.0);
        if let Some(first) = g.due.first() {
            if first.0 <= now {
                let (_, cid, id, answer) = g.due.remove(0);
                let apps: Vec<_> = g.apps.iter().filter(|(n, _)| cid == 0 || *n == cid).filter_map(|(_, w)| w.upgrade()).collect();
                drop(g);
                answer_now(&apps, &id, &answer);
                g = lock(m);
                continue;
            }
            let wait = first.0 - now;
            g = cv.wait_timeout(g, wait).unwrap_or_else(|e| e.into_inner()).0;
        } else {
            g = cv.wait(g).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// The user's answer to the notification `id` (of the first of `apps` that
/// has it outstanding), or the settings asked for with none.
fn answer_now(apps: &[Arc<Mutex<App>>], id: &str, answer: &Answer) {
    for a in apps {
        let mut c = lock(a);
        let had = c.outstanding.iter().position(|o| o == id);
        if had.is_none() && !(matches!(answer, Answer::Settings) && id.is_empty()) {
            continue;
        }
        let msg = match answer {
            Answer::Activate { action, text: typed } => {
                say(&format!(
                    "activate app={} id={:?}{}{}",
                    c.app,
                    id,
                    action.as_ref().map(|a| format!(" action={a:?}")).unwrap_or_default(),
                    typed.as_ref().map(|t| format!(" text={t:?}")).unwrap_or_default()
                ));
                format!(
                    "{{\"op\":\"response\",\"id\":{},\"action\":{}{}}}",
                    quote(id),
                    quote(action.as_deref().unwrap_or("default")),
                    typed.as_ref().map(|t| format!(",\"text\":{}", quote(&text(t, BODY_MAX)))).unwrap_or_default()
                )
            }
            Answer::Dismiss => {
                say(&format!("dismiss app={} id={id:?}", c.app));
                format!("{{\"op\":\"response\",\"id\":{},\"action\":\"dismiss\"}}", quote(id))
            }
            Answer::Clear => {
                say(&format!("clear app={} id={id:?}", c.app));
                format!("{{\"op\":\"cleared\",\"id\":{}}}", quote(id))
            }
            Answer::Settings => {
                say(&format!("settings app={}{}", c.app, if id.is_empty() { String::new() } else { format!(" id={id:?}") }));
                format!("{{\"op\":\"settings\",\"id\":{}}}", if id.is_empty() { "null".into() } else { quote(id) })
            }
        };
        // Answered, it is no longer outstanding (a banner closes when it is
        // clicked); asking for the settings leaves it.
        if let (Some(i), false) = (had, matches!(answer, Answer::Settings)) {
            c.outstanding.remove(i);
        }
        c.send(&msg);
        return;
    }
    say(&format!("({answer:?} for {id:?}: nothing outstanding has that identifier)"));
}

impl Session for Connection {
    fn message(&mut self, m: &Json, data: &[u8]) {
        let op = m.str("op").unwrap_or("");
        let (sm, cv) = shell();
        let mut c = lock(&self.app);
        if !c.hello && op != "hello" {
            say(&format!("refuse {op:?} before hello"));
            return;
        }
        match op {
            "hello" => {
                let app = m.get("app").cloned().unwrap_or(Json::Null);
                c.app = text(app.str("id").unwrap_or(""), ID_MAX);
                c.hello = true;
                let name = text(app.str("name").unwrap_or(""), TITLE_MAX);
                let icon = match image(&app, "icon", data, ICON_MAX) {
                    Ok(Some((t, n))) => format!(" icon={t}:{n}"),
                    Ok(None) => String::new(),
                    Err(e) => format!(" (icon left out: {e})"),
                };
                say(&format!("hello app={} name={name:?}{icon}", c.app));
                c.send(&format!(
                    "{{\"op\":\"welcome\",\"limits\":{{\"icon\":{ICON_MAX},\"image\":{IMAGE_MAX},\"title\":{TITLE_MAX},\"body\":{BODY_MAX},\"actions\":{ACTIONS_MAX},\"outstanding\":{OUTSTANDING_MAX}}}}}"
                ));
                let mut g = lock(sm);
                if g.start.is_none() {
                    let now = Instant::now();
                    g.start = Some(now);
                    let at: Vec<_> = g.script.at.iter().map(|(t, a, id)| (now + Duration::from_secs_f64(t.max(0.0)), 0, id.clone(), a.clone())).collect();
                    g.due.extend(at);
                    cv.notify_all();
                }
            }
            "authorize" => {
                let seq = m.num("seq").unwrap_or(0.0) as i64;
                let opts: Vec<String> = match m.get("options") {
                    Some(Json::Arr(v)) => v.iter().filter_map(|o| if let Json::Str(s) = o { Some(text(s, 32)) } else { None }).take(16).collect(),
                    _ => Vec::new(),
                };
                let ask = m.flag("ask");
                let mut g = lock(sm);
                let known = g.decided.iter().find(|(a, _)| *a == c.app).map(|(_, d)| *d);
                let decision = match (known, ask) {
                    (Some(d), _) => Some(d),
                    (None, true) => {
                        let d = g.script.decision(&c.app);
                        g.decided.push((c.app.clone(), d));
                        Some(d)
                    }
                    (None, false) => None,
                };
                drop(g);
                let status = match decision {
                    None => "not-determined",
                    Some(Decision::Grant) => "authorized",
                    Some(Decision::Deny) => "denied",
                    Some(Decision::Provisional) => "provisional",
                };
                if ask {
                    say(&format!("authorize app={} options={} -> {status}", c.app, opts.join(",")));
                }
                let on = matches!(decision, Some(Decision::Grant | Decision::Provisional));
                c.send(&format!("{{\"op\":\"authorization\",\"seq\":{seq},\"status\":\"{status}\",\"alert\":{on},\"sound\":{on},\"badge\":{on}}}"));
            }
            "post" => {
                let id = text(m.str("id").unwrap_or(""), ID_MAX);
                let g = lock(sm);
                let allowed = matches!(g.decided.iter().find(|(a, _)| *a == c.app).map(|(_, d)| *d), Some(Decision::Grant | Decision::Provisional));
                drop(g);
                if !allowed {
                    say(&format!("refuse app={} id={id:?}: not authorized", c.app));
                    c.send(&format!("{{\"op\":\"refused\",\"id\":{},\"reason\":\"not authorized\"}}", quote(&id)));
                    return;
                }
                let s = |k: &str, max: usize| text(m.str(k).unwrap_or(""), max);
                let mut line = format!(
                    "post app={} id={id:?} title={:?} subtitle={:?} body={:?}",
                    c.app,
                    s("title", TITLE_MAX),
                    s("subtitle", TITLE_MAX),
                    s("body", BODY_MAX)
                );
                match m.get("sound") {
                    Some(Json::Str(n)) if n == "default" => line.push_str(" sound=default"),
                    Some(Json::Str(n)) => {
                        let _ = write!(line, " sound={:?}", text(n, ID_MAX));
                    }
                    _ => {}
                }
                if let Some(b) = m.num("badge") {
                    let _ = write!(line, " badge={}", b.clamp(0.0, 1e9) as u64);
                }
                for k in ["thread", "category", "interruption"] {
                    let v = s(k, ID_MAX);
                    if !v.is_empty() {
                        let _ = write!(line, " {k}={v:?}");
                    }
                }
                for k in ["banner", "list"] {
                    if m.flag(k) {
                        let _ = write!(line, " {k}");
                    }
                }
                let replaced = c.outstanding.iter().position(|o| *o == id);
                let replace = !matches!(m.get("replace"), Some(Json::Bool(false)));
                if let Some(i) = replaced {
                    c.outstanding.remove(i);
                    line.push_str(if replace { " replaces" } else { " (the earlier one withdrawn)" });
                }
                say(&line);
                if let Some(Json::Arr(acts)) = m.get("actions") {
                    for a in acts.iter().take(ACTIONS_MAX) {
                        let mut l = format!("  action {:?} {:?}", text(a.str("id").unwrap_or(""), ID_MAX), text(a.str("title").unwrap_or(""), ID_MAX));
                        for f in ["destructive", "foreground", "authenticate"] {
                            if a.flag(f) {
                                let _ = write!(l, " {f}");
                            }
                        }
                        if let Some(t) = a.get("text").filter(|t| **t != Json::Null) {
                            let _ = write!(l, " text button={:?} placeholder={:?}", text(t.str("button").unwrap_or(""), ID_MAX), text(t.str("placeholder").unwrap_or(""), TITLE_MAX));
                        }
                        say(&l);
                    }
                    if acts.len() > ACTIONS_MAX {
                        say(&format!("  ({} more actions left out)", acts.len() - ACTIONS_MAX));
                    }
                }
                match image(m, "image", data, IMAGE_MAX) {
                    Ok(Some((t, n))) => say(&format!("  image {t} {n} bytes")),
                    Ok(None) => {}
                    Err(e) => say(&format!("  (image left out: {e})")),
                }
                if c.outstanding.len() >= OUTSTANDING_MAX {
                    let old = c.outstanding.remove(0);
                    say(&format!("  (the oldest, {old:?}, is cleared: {OUTSTANDING_MAX} are outstanding)"));
                    c.send(&format!("{{\"op\":\"cleared\",\"id\":{}}}", quote(&old)));
                }
                c.outstanding.push(id.clone());
                c.send(&format!("{{\"op\":\"shown\",\"id\":{},\"presented\":true}}", quote(&id)));
                // The script's answer to it, if it has one.
                let mut g = lock(sm);
                // The lines for it, in turn: a `settings` leaves it shown, so
                // the line after one answers the same showing.
                let now = Instant::now();
                let mut due = Vec::new();
                for rule in g.script.on.iter_mut().filter(|(rid, _, _, used)| !*used && (*rid == id || rid == "*")) {
                    rule.3 = true;
                    due.push((now + Duration::from_secs_f64(rule.1.max(0.0)), self.id, id.clone(), rule.2.clone()));
                    if !matches!(rule.2, Answer::Settings) {
                        break;
                    }
                }
                if !due.is_empty() {
                    g.due.extend(due);
                    cv.notify_all();
                }
            }
            "withdraw" => {
                if m.flag("all") {
                    say(&format!("withdraw app={} all ({})", c.app, c.outstanding.len()));
                    c.outstanding.clear();
                } else if let Some(Json::Arr(ids)) = m.get("ids") {
                    for i in ids {
                        if let Json::Str(id) = i {
                            if let Some(p) = c.outstanding.iter().position(|o| o == id) {
                                c.outstanding.remove(p);
                                say(&format!("withdraw app={} id={id:?}", c.app));
                            }
                        }
                    }
                }
            }
            "badge" => {
                if let Some(l) = m.str("label") {
                    say(&format!("badge app={} label={:?}", c.app, text(l, BADGE_LABEL_MAX)));
                } else {
                    let n = m.num("count").map(|n| n.clamp(0.0, 1e9) as u64).unwrap_or(0);
                    say(&format!("badge app={} count={n}", c.app));
                }
            }
            other => say(&format!("refuse app={} a message {other:?} no shell knows", c.app)),
        }
    }

    fn closed(&mut self) {
        let mut c = lock(&self.app);
        if c.hello {
            say(&format!("bye app={} ({} outstanding withdrawn)", c.app, c.outstanding.len()));
        }
        c.link = None;
    }
}
