//! `olp-hook install` / `olp-hook uninstall`.
//! What it does: (1) copies the two executables (olp-hook, olp-gui) to the fixed per-user location `<home>/olp/` (same place the install notes point to); (2) adds or removes one entry
//! each for PreToolUse and PostToolUse in `.claude/settings.json`. Malformed JSON is left untouched; other entries, key order and values are preserved.
//! Key order: serde_json's default Map reorders keys, and its `preserve_order` feature pulls in packages (foldhash, Zlib) whose licenses are not on the allow list,
//! so a small order-preserving value type (`OrderedJson`) is defined here instead, with no new dependencies.
//! The byte comparison after copying only checks copy integrity (it does not stop package substitution; supply-chain assurance is left to release SHA256SUMS and attestations).
//! The hook and counting paths never call this module.

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const EVENTS: [&str; 2] = ["PreToolUse", "PostToolUse"];
/// Same entry values as documented in the install notes.
const MATCHER: &str = "Bash|PowerShell|mcp__.*";
const TIMEOUT: u64 = 10;
const USAGE: &str = "usage: olp-hook install [--user] [--dry-run] [--replace] | olp-hook uninstall [--user] [--dry-run]";
/// Three-line "what it does" text printed at the end of install (matches the README: counting and the permission prompt, never auto-approves, on-device, no network).
const DOES: [&str; 3] = [
    "What OLP does now: before a shell command or MCP tool call runs, it counts what the call would remove or change and shows that in Claude Code's permission prompt.",
    "It never allows a call on its own: you still answer the prompt.",
    "It runs only on this device. The hook itself makes no network calls.",
];
/// Settings in the home folder's .claude/settings.json apply to every project.
const HOME_REFUSE: &str = "this folder is your home folder — a hook here applies to every project. Run install inside a project folder, or add --user if you want that.";
const LAYOUT_NOTE: &str = "note: the file's line layout changes (values and key order stay the same)";

/// Inputs from outside; tests inject a fake folder.
pub struct InstallEnv {
    pub project: PathBuf,
    pub home: PathBuf,
    /// The olp-hook executable currently running (olp-gui in the same folder is copied along with it).
    pub exe: PathBuf,
    pub now: u64,
}

impl InstallEnv {
    pub fn from_process(project: &Path) -> Result<InstallEnv, String> {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).ok_or("could not find your home folder")?;
        let exe = std::env::current_exe().map_err(|_| "could not find this program's file")?;
        Ok(InstallEnv { project: project.to_path_buf(), home, exe, now: crate::store::now() })
    }
}

/// The rewritten text and the lines to show the user.
pub struct Change {
    pub text: String,
    pub changed: bool,
    pub lines: Vec<String>,
}

/// JSON value that preserves order (an object is a Vec in insertion order).
#[derive(Clone, Debug, PartialEq)]
enum OrderedJson {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<OrderedJson>),
    Obj(Vec<(String, OrderedJson)>),
}

impl OrderedJson {
    fn get_mut(&mut self, key: &str) -> Option<&mut OrderedJson> {
        match self {
            OrderedJson::Obj(m) => m.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn get(&self, key: &str) -> Option<&OrderedJson> {
        match self {
            OrderedJson::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            OrderedJson::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for OrderedJson {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<OrderedJson,D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OrderedJson;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<OrderedJson,E> {
                Ok(OrderedJson::Null)
            }
            fn visit_bool<E>(self, b: bool) -> Result<OrderedJson,E> {
                Ok(OrderedJson::Bool(b))
            }
            fn visit_u64<E>(self, n: u64) -> Result<OrderedJson,E> {
                Ok(OrderedJson::Num(n.into()))
            }
            fn visit_i64<E>(self, n: i64) -> Result<OrderedJson,E> {
                Ok(OrderedJson::Num(n.into()))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> Result<OrderedJson,E> {
                serde_json::Number::from_f64(n).map(OrderedJson::Num).ok_or_else(|| E::custom("bad number"))
            }
            fn visit_str<E>(self, s: &str) -> Result<OrderedJson,E> {
                Ok(OrderedJson::Str(s.to_string()))
            }
            fn visit_string<E>(self, s: String) -> Result<OrderedJson,E> {
                Ok(OrderedJson::Str(s))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<OrderedJson,A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element()? {
                    v.push(x);
                }
                Ok(OrderedJson::Arr(v))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<OrderedJson,A::Error> {
                let mut m: Vec<(String, OrderedJson)> = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, OrderedJson>()? {
                    // On a duplicate key the later value wins (same as serde_json) but keeps the first position.
                    match m.iter_mut().find(|(x, _)| *x == k) {
                        Some(slot) => slot.1 = v,
                        None => m.push((k, v)),
                    }
                }
                Ok(OrderedJson::Obj(m))
            }
        }
        d.deserialize_any(V)
    }
}

impl Serialize for OrderedJson {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            OrderedJson::Null => s.serialize_unit(),
            OrderedJson::Bool(b) => s.serialize_bool(*b),
            OrderedJson::Num(n) => n.serialize(s),
            OrderedJson::Str(x) => s.serialize_str(x),
            OrderedJson::Arr(v) => {
                let mut q = s.serialize_seq(Some(v.len()))?;
                for x in v {
                    q.serialize_element(x)?;
                }
                q.end()
            }
            OrderedJson::Obj(m) => {
                let mut q = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    q.serialize_entry(k, v)?;
                }
                q.end()
            }
        }
    }
}

/// Original layout: indent width (first indented line, default 2) and trailing newline.
struct Layout {
    indent: usize,
    newline: bool,
}

fn layout_of(original: Option<&str>) -> Layout {
    let Some(t) = original else { return Layout { indent: 2, newline: true } };
    let indent = t.lines().map(|l| l.len() - l.trim_start_matches(' ').len()).find(|&n| n > 0).unwrap_or(2);
    Layout { indent, newline: t.ends_with('\n') }
}

fn render(v: &OrderedJson,l: &Layout) -> String {
    let ind = vec![b' '; l.indent];
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, serde_json::ser::PrettyFormatter::with_indent(&ind));
    // The value is our own OrderedJson, so serialization cannot fail (only strings, numbers, arrays and objects).
    let _ = v.serialize(&mut ser);
    let mut s = String::from_utf8(buf).unwrap_or_default();
    if l.newline {
        s.push('\n');
    }
    s
}

fn parse(original: &str) -> Result<OrderedJson,String> {
    let v: OrderedJson = serde_json::from_str(original).map_err(|_| "settings.json is not valid JSON; nothing changed".to_string())?;
    if !matches!(v, OrderedJson::Obj(_)) {
        return Err("settings.json is not a JSON object; nothing changed".to_string());
    }
    Ok(v)
}

/// Whether a command is our executable: the last path component, with quotes stripped, is `olp-hook` or `olp-hook.exe` (case-insensitive).
pub fn is_our_command(cmd: &str) -> bool {
    let c = cmd.trim().trim_matches('"').trim_matches('\'');
    let name = c.rsplit(['/', '\\']).next().unwrap_or(c).to_ascii_lowercase();
    name == "olp-hook" || name == "olp-hook.exe"
}

/// Only the pre-execution (PreToolUse) entry gets `onFailure: block`. If the hook fails or times out (TIMEOUT 10 s), Claude Code blocks that call
/// (Claude Code hooks docs, "Block the action when a hook fails", v2.1.295+). The post-execution entry has nothing to block, so it gets none.
/// Why 10 s: the hook itself finishes within 5 s (default hook_budget_ms); measured over 45 runs: median 59 ms, max 363 ms (9 sample commands).
fn pre_event(ev: &str) -> bool {
    ev == "PreToolUse"
}

fn our_hook(command: &str, ev: &str) -> OrderedJson {
    let mut h = vec![("type".into(), OrderedJson::Str("command".into())), ("command".into(), OrderedJson::Str(command.into())), ("timeout".into(), OrderedJson::Num(TIMEOUT.into()))];
    if pre_event(ev) {
        h.push(("onFailure".into(), OrderedJson::Str("block".into())));
    }
    OrderedJson::Obj(h)
}

fn our_entry(command: &str, ev: &str) -> OrderedJson {
    OrderedJson::Obj(vec![("matcher".into(), OrderedJson::Str(MATCHER.into())), ("hooks".into(), OrderedJson::Arr(vec![our_hook(command, ev)]))])
}

/// Adds onFailure: block to an existing PreToolUse entry of ours that lacks it (upgrading older installs). Returns true if it added one.
fn ensure_on_failure(list: &mut [OrderedJson], i: usize, j: usize) -> bool {
    let Some(OrderedJson::Arr(hs)) = list[i].get_mut("hooks") else { return false };
    let OrderedJson::Obj(h) = &mut hs[j] else { return false };
    match h.iter_mut().find(|(k, _)| k == "onFailure") {
        Some((_, v)) if v.str() == Some("block") => false,
        Some((_, v)) => {
            *v = OrderedJson::Str("block".into());
            true
        }
        None => {
            h.push(("onFailure".into(), OrderedJson::Str("block".into())));
            true
        }
    }
}

/// Commands of our hooks inside an entry (entry index, hook index, command).
fn ours_in(list: &[OrderedJson]) -> Vec<(usize, usize, String)> {
    matching_in(list, is_our_command)
}

fn matching_in(list: &[OrderedJson], ours: fn(&str) -> bool) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (i, e) in list.iter().enumerate() {
        if let Some(OrderedJson::Arr(hs)) = e.get("hooks") {
            for (j, h) in hs.iter().enumerate() {
                if let Some(c) = h.get("command").and_then(OrderedJson::str) {
                    if ours(c) {
                        out.push((i, j, c.to_string()));
                    }
                }
            }
        }
    }
    out
}

/// Gets the `hooks` object and its event list (creating them if missing). Errors if the shape is wrong.
fn event_list<'a>(root: &'a mut OrderedJson,event: &str, create: bool) -> Result<Option<&'a mut Vec<OrderedJson>>, String> {
    let OrderedJson::Obj(m) = root else { return Err("settings.json is not a JSON object; nothing changed".into()) };
    if !m.iter().any(|(k, _)| k == "hooks") {
        if !create {
            return Ok(None);
        }
        m.push(("hooks".into(), OrderedJson::Obj(Vec::new())));
    }
    let Some(OrderedJson::Obj(h)) = root.get_mut("hooks") else { return Err("\"hooks\" in settings.json is not an object; nothing changed".into()) };
    if !h.iter().any(|(k, _)| k == event) {
        if !create {
            return Ok(None);
        }
        h.push((event.into(), OrderedJson::Arr(Vec::new())));
    }
    match h.iter_mut().find(|(k, _)| k == event).map(|(_, v)| v) {
        Some(OrderedJson::Arr(list)) => Ok(Some(list)),
        _ => Err(format!("\"hooks.{event}\" in settings.json is not a list; nothing changed")),
    }
}

fn finish(original: Option<&str>, root: &OrderedJson,changed: bool, mut lines: Vec<String>) -> Change {
    let l = layout_of(original);
    if !changed {
        return Change { text: original.unwrap_or_default().to_string(), changed, lines };
    }
    if let Some(o) = original {
        // If re-serializing the original gives different text, report that the line layout will change.
        if parse(o).map(|v| render(&v, &l) != o).unwrap_or(true) {
            lines.push(LAYOUT_NOTE.into());
        }
    }
    Change { text: render(root, &l), changed, lines }
}

/// Adds one entry of ours per event. An existing entry with the same command is kept; one of ours with a different path is only reported (replaced when `replace` is set).
pub fn add_entries(original: Option<&str>, command: &str, replace: bool) -> Result<Change, String> {
    let mut root = match original {
        Some(o) => parse(o)?,
        None => OrderedJson::Obj(Vec::new()),
    };
    let mut changed = false;
    let mut lines = Vec::new();
    for ev in EVENTS {
        let list = event_list(&mut root, ev, true)?.expect("만듦");
        let found = ours_in(list);
        if let Some((i, j, _)) = found.iter().find(|(_, _, c)| c == command).cloned() {
            if pre_event(ev) && ensure_on_failure(list, i, j) {
                changed = true;
                lines.push(format!("{ev}: added onFailure: block (block the call if the hook fails or times out)"));
            } else {
                lines.push(format!("{ev}: already there"));
            }
        } else if let Some((i, j, old)) = found.first().cloned() {
            if replace {
                let only_ours = matches!(list[i].get("hooks"), Some(OrderedJson::Arr(hs)) if hs.len() == 1);
                if only_ours {
                    list[i] = our_entry(command, ev);
                } else if let Some(OrderedJson::Arr(hs)) = list[i].get_mut("hooks") {
                    hs[j] = our_hook(command, ev);
                }
                changed = true;
                lines.push(format!("{ev}: replaced {old} with {command}"));
            } else {
                lines.push(format!("{ev}: olp-hook is already set at {old}; left as is (use --replace to point it to {command})"));
            }
        } else {
            list.push(our_entry(command, ev));
            changed = true;
            lines.push(format!("{ev}: added {command}"));
        }
    }
    Ok(finish(original, &root, changed, lines))
}

/// Removes only our hooks (PreToolUse, PostToolUse and the usage-count SessionStart entries). An item containing only our hooks is removed whole; empty lists and empty hooks objects are kept.
pub fn remove_entries(original: &str) -> Result<Change, String> {
    remove_where(original, &[("PreToolUse", is_our_command), ("PostToolUse", is_our_command), (START_EVENT, is_our_start_command)])
}

/// Removes only the usage-count SessionStart entry (`telemetry off`); hook entries are untouched.
pub fn remove_start_entry(original: &str) -> Result<Change, String> {
    remove_where(original, &[(START_EVENT, is_our_start_command)])
}

/// (event name, predicate telling whether a command is ours for that event)
type EventMatch<'a> = (&'a str, fn(&str) -> bool);

fn remove_where(original: &str, events: &[EventMatch]) -> Result<Change, String> {
    let mut root = parse(original)?;
    let mut changed = false;
    let mut lines = Vec::new();
    for &(ev, ours) in events {
        let Some(list) = event_list(&mut root, ev, false)? else { continue };
        let before = matching_in(list, ours).len();
        if before == 0 {
            continue;
        }
        list.retain_mut(|e| {
            let Some(OrderedJson::Arr(hs)) = e.get_mut("hooks") else { return true };
            let had = hs.len();
            hs.retain(|h| !h.get("command").and_then(OrderedJson::str).is_some_and(ours));
            !(had > 0 && hs.is_empty())
        });
        changed = true;
        lines.push(format!("{ev}: removed olp-hook ({before})"));
    }
    if !changed {
        lines.push(if events.len() == 1 { format!("{}: nothing to remove", events[0].0) } else { "olp-hook is not in this settings file; nothing to remove".into() });
    }
    Ok(finish(Some(original), &root, changed, lines))
}

fn exe_name(base: &str) -> String {
    format!("{base}{}", std::env::consts::EXE_SUFFIX)
}

/// Command string to write into settings.json: forward-slash path, wrapped in double quotes if it contains spaces.
fn command_for(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Copy to a temporary name, compare bytes, then rename. Stops if the rename fails (e.g. locked while running).
fn copy_program(src: &Path, dst: &Path) -> Result<(), String> {
    let tmp = dst.with_extension("olp-new");
    let fail = |m: String| {
        let _ = fs::remove_file(&tmp);
        m
    };
    fs::copy(src, &tmp).map_err(|_| fail(format!("could not copy {}", src.display())))?;
    let same = matches!((fs::read(src), fs::read(&tmp)), (Ok(a), Ok(b)) if a == b);
    if !same {
        return Err(fail(format!("the copy of {} does not match the original; nothing changed", src.display())));
    }
    fs::rename(&tmp, dst).map_err(|_| fail(format!("could not replace {} — if Claude Code is running, close it and run install again", dst.display())))
}

/// Before writing, saves the original under `<home>/olp/backup/` (outside the repository). If a backup exists, a number is appended (never overwrites).
fn backup(env: &InstallEnv, label: &str, text: &str) -> Result<PathBuf, String> {
    let dir = env.home.join("olp").join("backup");
    fs::create_dir_all(&dir).map_err(|_| format!("could not make {}", dir.display()))?;
    for n in 0..1000 {
        let name = if n == 0 { format!("{label}-settings-{}.json", env.now) } else { format!("{label}-settings-{}-{n}.json", env.now) };
        let p = dir.join(name);
        match fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(mut f) => return f.write_all(text.as_bytes()).map(|_| p).map_err(|_| "could not write the backup; nothing changed".into()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    Err("could not write the backup; nothing changed".into())
}

/// Writes to a temporary file in the same folder, then renames.
fn write_settings(path: &Path, text: &str) -> Result<(), String> {
    let dir = path.parent().ok_or("bad settings path")?;
    fs::create_dir_all(dir).map_err(|_| format!("could not make {}", dir.display()))?;
    let tmp = dir.join("settings.json.olp-tmp");
    let r = fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, path));
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
        return Err(format!("could not write {}", path.display()));
    }
    Ok(())
}

/// `olp-hook install …` / `olp-hook uninstall …`.
pub fn run(cmd: &str, args: &[String], env: &InstallEnv) -> Result<String, String> {
    let (mut user, mut dry, mut replace) = (false, false, false);
    for a in args {
        match a.as_str() {
            "--user" => user = true,
            "--dry-run" => dry = true,
            "--replace" if cmd == "install" => replace = true,
            _ => return Err(USAGE.into()),
        }
    }
    // Running from the home folder without --user would edit the user-global settings, so refuse before doing anything (--dry-run too). uninstall is unaffected.
    if cmd == "install" && !user && same_file(&env.project, &env.home) {
        return Err(HOME_REFUSE.into());
    }
    let base = if user { &env.home } else { &env.project };
    let settings = base.join(".claude").join("settings.json");
    let label = if user { "user".to_string() } else { env.project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into()) };
    let original = match fs::read_to_string(&settings) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(format!("could not read {}; nothing changed", settings.display())),
    };
    let dir = env.home.join("olp");
    let mut out = Vec::new();
    let change = if cmd == "uninstall" {
        match &original {
            Some(o) => remove_entries(o)?,
            None => return Ok(format!("{} does not exist; nothing to remove", settings.display())),
        }
    } else {
        add_entries(original.as_deref(), &command_for(&dir.join(exe_name("olp-hook"))), replace)?
    };
    if dry {
        out.push(format!("dry run — nothing was changed. {}:", settings.display()));
        out.extend(change.lines);
        return Ok(out.join("\n"));
    }
    if cmd == "install" {
        let src_dir = env.exe.parent().ok_or("could not find this program's folder")?;
        // If the executable was downloaded under a different name (olp-hook-windows-x64.exe), no olp-hook(.exe) sits next to it, nothing is copied, and settings would point at a missing file, blocking every shell command.
        // If the olp-hook to register exists neither at the source nor at the install location, fail without changing anything (no settings change, no copy).
        let hook = exe_name("olp-hook");
        if !src_dir.join(&hook).exists() && !dir.join(&hook).exists() {
            return Err(format!("{hook} was not found next to this program in {}; keep the name {hook} when you unpack the release and run install again. nothing changed", src_dir.display()));
        }
        fs::create_dir_all(&dir).map_err(|_| format!("could not make {}", dir.display()))?;
        for base in ["olp-hook", "olp-gui"] {
            let (src, dst) = (src_dir.join(exe_name(base)), dir.join(exe_name(base)));
            if !src.exists() {
                // The current release bundle has no olp-gui, so skip silently when it is absent (code unchanged; copied when the bundle includes it).
                continue;
            } else if same_file(&src, &dst) {
                out.push(format!("{} is already in {}", exe_name(base), dir.display()));
            } else {
                copy_program(&src, &dst)?;
                out.push(format!("copied {} to {}", exe_name(base), dir.display()));
            }
        }
    }
    if change.changed {
        if let Some(o) = &original {
            out.push(format!("backup: {}", backup(env, &label, o)?.display()));
        }
        // Install already copied the files, so say plainly: copied, but hook entry not added.
        write_settings(&settings, &change.text).map_err(|e| if cmd == "install" { format!("{e}: the programs were copied to {} but the hook was not added", dir.display()) } else { e })?;
    }
    out.push(format!("{}:", settings.display()));
    out.extend(change.lines);
    if cmd == "install" {
        // Final install notes in one block: entries added (above), the three "what it does" lines, one permission-rule line, then the removal line.
        out.extend(DOES.iter().map(|s| s.to_string()));
        let s = crate::rules::scan(&env.project, &env.home);
        out.push(crate::rules::summary(&s.rules, s.unreadable));
        out.push(format!("undo: run the same command with uninstall instead of install (the backup stays in {})", dir.join("backup").display()));
    } else {
        out.push(format!("the programs in {} were left in place; delete that folder if you no longer need them", dir.display()));
    }
    Ok(out.join("\n"))
}

// ---- Usage-count SessionStart entry (only for users who consented) ----
// This module only adds and removes the entry. Asking for consent, recording it and sending belong to the executable (main.rs) and the signal module.

const START_EVENT: &str = "SessionStart";
/// Tail of the SessionStart command. `--session-start` = no output and exit 0 (SessionStart output enters Claude's context).
const START_ARGS: &str = "signal --session-start";
/// New sessions only (not resume, clear or compact).
const START_MATCHER: &str = "startup";
/// Total limit 3 s (same as the sender's time cap). No onFailure, so a failure never affects the session.
const START_TIMEOUT: u64 = 3;

/// Whether a SessionStart command is ours: `<our executable> signal --session-start`.
pub fn is_our_start_command(cmd: &str) -> bool {
    cmd.trim().strip_suffix(START_ARGS).is_some_and(|exe| exe.ends_with(' ') && is_our_command(exe))
}

fn start_entry(command: &str) -> OrderedJson {
    let hook = OrderedJson::Obj(vec![("type".into(), OrderedJson::Str("command".into())), ("command".into(), OrderedJson::Str(command.into())), ("timeout".into(), OrderedJson::Num(START_TIMEOUT.into()))]);
    OrderedJson::Obj(vec![("matcher".into(), OrderedJson::Str(START_MATCHER.into())), ("hooks".into(), OrderedJson::Arr(vec![hook]))])
}

/// Adds one SessionStart entry of ours (`exe_command` is the same executable string as the hook entries). An identical entry is kept; one with a different path is replaced.
pub fn add_start_entry(original: Option<&str>, exe_command: &str) -> Result<Change, String> {
    let mut root = match original {
        Some(o) => parse(o)?,
        None => OrderedJson::Obj(Vec::new()),
    };
    let command = format!("{exe_command} {START_ARGS}");
    let list = event_list(&mut root, START_EVENT, true)?.expect("만듦");
    let found = matching_in(list, is_our_start_command);
    let (changed, line) = match found.first() {
        Some((_, _, c)) if *c == command => (false, format!("{START_EVENT}: already there")),
        Some(&(i, j, _)) => {
            // If the entry holds another tool's hook, replace only our single hook (same rule as add_entries).
            let only_ours = matches!(list[i].get("hooks"), Some(OrderedJson::Arr(hs)) if hs.len() == 1);
            if only_ours {
                list[i] = start_entry(&command);
            } else if let Some(OrderedJson::Arr(hs)) = list[i].get_mut("hooks") {
                if let Some(OrderedJson::Obj(h)) = hs.get_mut(j) {
                    if let Some((_, v)) = h.iter_mut().find(|(k, _)| k == "command") {
                        *v = OrderedJson::Str(command.clone());
                    }
                }
            }
            (true, format!("{START_EVENT}: replaced with {command}"))
        }
        None => {
            list.push(start_entry(&command));
            (true, format!("{START_EVENT}: added {command} (daily usage numbers, you said yes)"))
        }
    };
    Ok(finish(original, &root, changed, vec![line]))
}

/// Install route: if the executable runs from node_modules it is npx (npm package), otherwise manual. Only the route name is kept, never the path.
pub fn install_path(exe: &Path) -> &'static str {
    if exe.components().any(|c| c.as_os_str() == "node_modules") {
        "npx"
    } else {
        "manual"
    }
}

/// Whether this settings file has our hook under PreToolUse (read-only; a broken file counts as none).
fn has_our_hook(settings: &Path) -> bool {
    let Ok(t) = fs::read_to_string(settings) else { return false };
    let Ok(mut root) = parse(&t) else { return false };
    matches!(event_list(&mut root, "PreToolUse", false), Ok(Some(list)) if !ours_in(list).is_empty())
}

/// True only if the hook is in user settings (so that on setup consent the SessionStart entry goes to the install location). False if it is in a project or nowhere = project.
pub fn hook_in_user_settings(env: &InstallEnv) -> bool {
    let at = |base: &Path| base.join(".claude").join("settings.json");
    !has_our_hook(&at(&env.project)) && has_our_hook(&at(&env.home))
}

/// Adds (`add`) or removes the SessionStart entry in settings.json. If anything changes, the original is saved under `<home>/olp/backup/` before writing.
pub fn edit_start(env: &InstallEnv, user: bool, add: bool) -> Result<String, String> {
    let base = if user { &env.home } else { &env.project };
    let settings = base.join(".claude").join("settings.json");
    let original = match fs::read_to_string(&settings) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(format!("could not read {}; nothing changed", settings.display())),
    };
    let change = if add {
        add_start_entry(original.as_deref(), &command_for(&env.home.join("olp").join(exe_name("olp-hook"))))?
    } else {
        match &original {
            Some(o) => remove_start_entry(o)?,
            None => return Ok(format!("{}: no SessionStart entry", settings.display())),
        }
    };
    if change.changed {
        if let Some(o) = &original {
            let label = if user { "user".to_string() } else { env.project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "project".into()) };
            backup(env, &label, o)?;
        }
        write_settings(&settings, &change.text)?;
    }
    let mut out = vec![format!("{}:", settings.display())];
    out.extend(change.lines);
    Ok(out.join("\n"))
}
