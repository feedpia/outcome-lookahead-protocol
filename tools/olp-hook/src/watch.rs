//! `olp-hook watch` - companion view. Reads `.olp/hook-log.jsonl` read-only.
//! - Text view: `render` (pure: log lines + current time -> string). Looping and file writes live in the thin outer layer (`run`).
//! - HTML view: fills the same values into the `data/watch_template.html` template and writes one `.olp/watch.html` via temp file + rename.
//! Fields rendered: ts, findings kind/count (n, unit, source), reversible, recovery, unchecked, verdict, card/tool_use_id (pairing only).
//! Other fields (why, reasons, unknown fields) are neither read nor rendered. No network, no spawning other programs, no opening a browser.

use crate::stats::{self, Week};
use crate::store;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Template written by the frontend (verbatim, embedded at build time). Slot contract = `SLOTS`.
pub const TEMPLATE: &str = include_str!("../data/watch_template.html");

/// The 20 `{{...}}` slots of the template (slot contract).
pub const SLOTS: [&str; 20] = [
    "lang",
    "updated_ago_s",
    "card.state",
    "card.kind",
    "card.count_n",
    "card.count_unit",
    "card.count_state",
    "card.undo",
    "card.waiting_s",
    "files[].name",
    "files[].nature",
    "files_more_n",
    "week.cards",
    "week.ran",
    "week.not_run",
    "week.blocked",
    "week.files_ran",
    "week.files_not_run",
    "week.files_blocked",
    "week.not_counted",
];

// Closed list of code values. Anything else cannot reach the page (values are used as class names).
const KINDS: [&str; 6] = ["file_delete", "file_overwrite", "git_force_push", "git_reset_hard", "db_delete", "db_update"];
const UNITS: [&str; 4] = ["file", "commit", "change", "row"];
const STATES: [&str; 5] = ["waiting", "ran", "not_run", "blocked", "none"];
const COUNT_STATES: [&str; 3] = ["counted", "not_checked", "user_declared"];
const UNDOS: [&str; 2] = ["not_confirmed", "declared"];
const NATURES: [&str; 5] = ["tracked", "ignored", "recent", "untracked", "unreadable"];
const LANGS: [&str; 2] = ["en", "ko"];
/// Max file rows rendered (template contract).
const MAX_ROWS: usize = 6;
const TICK_MS: u64 = 500;
const REFRESH_LINE: &str = "<meta http-equiv=\"refresh\" content=\"1\">\n";

fn allowed(slot: &str) -> Option<&'static [&'static str]> {
    Some(match slot {
        "lang" => &LANGS,
        "card.state" => &STATES,
        "card.kind" => &KINDS,
        "card.count_unit" => &UNITS,
        "card.count_state" => &COUNT_STATES,
        "card.undo" => &UNDOS,
        "files[].nature" => &NATURES,
        _ => return None,
    })
}

/// The single escape function every slot value passes through.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// Replaces `{{slot}}` with its value. Value = `get(slot)`; code slots are blank if outside the closed list; then `esc`. Only template text is scanned (a `{{` inside a value is not rescanned).
fn subst(text: &str, get: &dyn Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len() + 512);
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find("}}") {
            Some(j) => {
                let slot = &after[..j];
                let mut v = get(slot);
                if allowed(slot).is_some_and(|l| !l.contains(&v.as_str())) {
                    v.clear();
                }
                out.push_str(&esc(&v));
                rest = &after[j + 2..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Fill the template. `files` = (name, kind), at most 6 rows: the `<!--FILE_ROW_BEGIN-->...<!--FILE_ROW_END-->` sample is copied per row (with 0 rows the sample is dropped entirely).
pub fn fill(tpl: &str, vals: &BTreeMap<String, String>, files: &[(String, String)]) -> String {
    const B: &str = "<!--FILE_ROW_BEGIN-->";
    const E: &str = "<!--FILE_ROW_END-->";
    let main = |slot: &str| vals.get(slot).cloned().unwrap_or_default();
    match (tpl.find(B), tpl.find(E)) {
        (Some(b), Some(e)) if b + B.len() <= e => {
            let sample = &tpl[b + B.len()..e];
            let rows: String = files
                .iter()
                .take(MAX_ROWS)
                .map(|(name, nature)| {
                    subst(sample, &|s| match s {
                        "files[].name" => name.clone(),
                        "files[].nature" => nature.clone(),
                        _ => String::new(),
                    })
                })
                .collect();
            format!("{}{}{}", subst(&tpl[..b], &main), rows, subst(&tpl[e + E.len()..], &main))
        }
        _ => subst(tpl, &main),
    }
}

// ---- log -> display values (pure) ----

struct View {
    has_card: bool,
    state: &'static str,
    kind: &'static str,
    n: Option<u64>,
    unit: &'static str,
    count_state: &'static str,
    undo: &'static str,
    waiting_s: Option<u64>,
    verdict: &'static str,
    ago_s: u64,
    updated_ago_s: u64,
    week: Week,
}

fn pick(list: &'static [&'static str], v: &Value) -> &'static str {
    v.as_str().and_then(|s| list.iter().copied().find(|x| *x == s)).unwrap_or("")
}

/// A card = a decision shown in the permission dialog (`card_shown`) or a block (same criterion as stats::week).
fn is_card(l: &Value) -> bool {
    l["event"] == "decision" && (l["card"] == "card_shown" || l["verdict"] == "blocked" || l["verdict"] == "error_blocked")
}

fn view(log: &[Value], now: u64) -> View {
    let week = stats::week(log, now);
    let updated_ago_s = log.iter().filter_map(|l| l["ts"].as_u64()).max().map_or(0, |t| now.saturating_sub(t));
    let mut v = View { has_card: false, state: "none", kind: "", n: None, unit: "", count_state: "", undo: "", waiting_s: None, verdict: "", ago_s: 0, updated_ago_s, week };
    let Some(i) = log.iter().rposition(is_card) else { return v };
    let d = &log[i];
    v.has_card = true;
    let blocked = d["verdict"] == "blocked" || d["verdict"] == "error_blocked";
    v.verdict = if blocked { "blocked" } else { "asked" };
    let ts = d["ts"].as_u64().unwrap_or(now);
    v.ago_s = now.saturating_sub(ts);
    let paired = d["tool_use_id"].as_str().is_some_and(|id| log.iter().any(|l| l["card"] == "executed_after_card" && l["tool_use_id"].as_str() == Some(id)));
    // waiting is decided only by "no later decision followed", not by time (staying long in the permission dialog does not turn it into not run)
    v.state = if blocked {
        "blocked"
    } else if paired {
        "ran"
    } else if log[i + 1..].iter().any(|l| l["event"] == "decision") {
        "not_run"
    } else {
        "waiting"
    };
    v.waiting_s = (v.state == "waiting").then_some(v.ago_s);
    let f = &d["findings"][0];
    v.kind = pick(&KINDS, &f["kind"]);
    let c = &f["count"];
    let unit = pick(&UNITS, &c["unit"]);
    let has_finding = d["findings"].as_array().is_some_and(|a| !a.is_empty());
    (v.count_state, v.n, v.unit) = match (c["n"].as_u64(), c["source"].as_str()) {
        (Some(n), Some("counted")) if !unit.is_empty() => ("counted", Some(n), unit),
        (Some(n), Some("user_declared")) if !unit.is_empty() => ("user_declared", Some(n), unit),
        _ if has_finding || d["unchecked"] == true => ("not_checked", None, ""),
        _ => ("", None, ""),
    };
    v.undo = if d["reversible"] == true && d["recovery"] == "declared" { "declared" } else { "not_confirmed" };
    v
}

fn kind_text(kind: &str) -> &'static str {
    match kind {
        "file_delete" => "delete files",
        "file_overwrite" => "overwrite files",
        "git_force_push" => "git force push",
        "git_reset_hard" => "git reset --hard",
        "db_delete" => "delete database rows",
        "db_update" => "change database rows",
        _ => "write action",
    }
}

fn paint(on: bool, code: &str, s: &str) -> String {
    if on {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

fn text(v: &View, color: bool) -> String {
    if !v.has_card {
        return "waiting for the first card".to_string();
    }
    const RED: &str = "31";
    const YELLOW: &str = "33";
    const GRAY: &str = "90";
    let destructive = matches!(v.kind, "file_delete" | "file_overwrite" | "git_force_push" | "git_reset_hard" | "db_delete");
    let kind = paint(color && destructive, RED, kind_text(v.kind));
    let plural = |n: u64| if n == 1 { "" } else { "s" };
    let count = match (v.count_state, v.n) {
        ("counted", Some(n)) => format!("{n} {}{}", v.unit, plural(n)),
        ("user_declared", Some(n)) => paint(color, YELLOW, &format!("{n} {}{} (declared, not checked)", v.unit, plural(n))),
        ("not_checked", _) => paint(color, YELLOW, "count not checked"),
        _ => String::new(),
    };
    let now_line = if count.is_empty() { format!("Now: {kind}") } else { format!("Now: {kind} - {count}") };
    let undo = if v.undo == "declared" { "Undo: declared (recovery location declared, not checked)".to_string() } else { paint(color, RED, "Undo: not confirmed") };
    let result = match v.state {
        "ran" => "Result: ran".to_string(),
        "not_run" => paint(color, GRAY, "Result: not run"),
        "blocked" => paint(color, RED, "Result: blocked before it ran"),
        _ => format!("Result: waiting for your answer ({}s)", v.waiting_s.unwrap_or(0)),
    };
    let w = &v.week;
    [
        now_line,
        undo,
        result,
        format!("Card: {} - {}s ago", v.verdict, v.ago_s),
        format!("This week: cards {} · ran {} · not run {} · blocked {}", w.cards, w.ran, w.not_run, w.blocked),
        format!("Files on cards: ran {} · not run {} · blocked {} · not counted {}", w.files_ran, w.files_not_run, w.files_blocked, w.not_counted),
        "Counted on this device only. Nothing is sent. Read-only.".to_string(),
    ]
    .join("\n")
}

/// Text view, plain (no color).
pub fn render(log: &[Value], now: u64) -> String {
    text(&view(log, now), false)
}

/// Text view, colored (identical to `render` once color is stripped).
pub fn render_color(log: &[Value], now: u64) -> String {
    text(&view(log, now), true)
}

/// Template slot values (18 scalar slots; `files[]` is always 0 rows at this level). Code slots only take list members.
pub fn slots(log: &[Value], now: u64, lang: &str) -> BTreeMap<String, String> {
    let v = view(log, now);
    let num = |x: Option<u64>| x.map(|n| n.to_string()).unwrap_or_default();
    let w = &v.week;
    let mut m: BTreeMap<String, String> = BTreeMap::new();
    let mut put = |k: &str, val: String| {
        m.insert(k.to_string(), val);
    };
    put("lang", if lang == "ko" { "ko" } else { "en" }.to_string());
    put("updated_ago_s", v.updated_ago_s.to_string());
    put("card.state", v.state.to_string());
    put("card.kind", v.kind.to_string());
    put("card.count_n", num(v.n));
    put("card.count_unit", v.unit.to_string());
    put("card.count_state", v.count_state.to_string());
    put("card.undo", v.undo.to_string());
    put("card.waiting_s", num(v.waiting_s));
    put("files_more_n", String::new());
    for (k, n) in [("cards", w.cards), ("ran", w.ran), ("not_run", w.not_run), ("blocked", w.blocked), ("files_ran", w.files_ran), ("files_not_run", w.files_not_run), ("files_blocked", w.files_blocked), ("not_counted", w.not_counted)] {
        put(&format!("week.{k}"), n.to_string());
    }
    m
}

/// HTML view. If `refresh` is false (`--once`, for recording) the refresh line is omitted.
pub fn render_html(log: &[Value], now: u64, lang: &str, refresh: bool) -> String {
    let vals = slots(log, now, lang);
    if refresh {
        fill(TEMPLATE, &vals, &[])
    } else {
        fill(&TEMPLATE.replacen(REFRESH_LINE, "", 1), &vals, &[])
    }
}

// ---- tail reading ----

/// Tail-read the log: read only the growth; if it shrinks, restart from the beginning. Read-only (never creates or modifies files).
pub struct Tail {
    path: PathBuf,
    pos: u64,
    pending: Vec<u8>,
    pub records: Vec<Value>,
    pub bytes_read: u64,
}

/// Saves memory by keeping only allowed fields (the renderer reads only those, so the result is the same).
fn slim(v: &Value) -> Value {
    let mut o = Map::new();
    for k in ["ts", "event", "verdict", "effect", "reversible", "recovery", "unchecked", "card", "tool_use_id", "human_choice"] {
        if let Some(x) = v.get(k) {
            o.insert(k.to_string(), x.clone());
        }
    }
    if let Some(a) = v["findings"].as_array() {
        let f: Vec<Value> = a.iter().map(|f| json!({"kind": f["kind"], "count": {"n": f["count"]["n"], "unit": f["count"]["unit"], "source": f["count"]["source"]}})).collect();
        o.insert("findings".to_string(), Value::Array(f));
    }
    Value::Object(o)
}

impl Tail {
    pub fn new(path: &Path) -> Tail {
        Tail { path: path.to_path_buf(), pos: 0, pending: Vec::new(), records: Vec::new(), bytes_read: 0 }
    }

    /// Reads new bytes if any and appends them line by line (a partial line waits until it is complete). Returns true if changed.
    pub fn poll(&mut self) -> bool {
        let len = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let mut changed = false;
        if len < self.pos {
            self.pos = 0;
            self.pending.clear();
            changed = !self.records.is_empty();
            self.records.clear();
        }
        if len == self.pos {
            return changed;
        }
        let Ok(mut f) = std::fs::File::open(&self.path) else { return changed };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return changed;
        }
        let mut buf = Vec::new();
        let Ok(n) = f.take(len - self.pos).read_to_end(&mut buf) else { return changed };
        self.pos += n as u64;
        self.bytes_read += n as u64;
        self.pending.extend_from_slice(&buf);
        if let Some(nl) = self.pending.iter().rposition(|b| *b == b'\n') {
            let rest = self.pending.split_off(nl + 1);
            for line in self.pending.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
                if let Ok(v) = serde_json::from_slice::<Value>(line) {
                    self.records.push(slim(&v));
                }
            }
            self.pending = rest;
        }
        true
    }
}

// ---- file writing, URL, environment ----

fn open_private(p: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

/// Replace via temp file + rename only when content changed (so a browser never reads a half-written file). Returns true if written.
/// If rename fails (e.g. a browser holds the file on Windows) returns Err; `last` is unchanged so the next tick retries.
pub fn write_if_changed(path: &Path, content: &str, last: &mut Option<String>) -> std::io::Result<bool> {
    if last.as_deref() == Some(content) && path.exists() {
        return Ok(false);
    }
    let tmp = path.with_extension("html.tmp");
    {
        let mut f = open_private(&tmp)?;
        f.write_all(content.as_bytes())?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    *last = Some(content.to_string());
    Ok(true)
}

fn pct(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// Path -> file URL (pure). Backslashes become slashes only for Windows forms (drive/UNC); spaces, non-ASCII, `#`, `%` are percent-encoded.
pub fn file_url(path: &str) -> String {
    let p = path.strip_prefix("\\\\?\\").unwrap_or(path);
    let b = p.as_bytes();
    let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
    let unc = p.starts_with("\\\\");
    let s = if drive || unc { p.replace('\\', "/") } else { p.to_string() };
    let (host, rest) = if unc {
        let t = &s[2..];
        match t.find('/') {
            Some(i) => (&t[..i], &t[i..]),
            None => (t, ""),
        }
    } else {
        ("", s.as_str())
    };
    let body = if drive { format!("/{}:{}", &rest[..1], pct(&rest[2..])) } else if rest.starts_with('/') { pct(rest) } else { format!("/{}", pct(rest)) };
    format!("file://{}{}", pct(host), body)
}

/// Headless environment: inside SSH, or on Linux with neither DISPLAY nor WAYLAND_DISPLAY.
pub fn headless(ssh: bool, display: bool, wayland: bool, linux: bool) -> bool {
    ssh || (linux && !display && !wayland)
}

pub fn start_message(url: &str, headless: bool) -> String {
    if headless {
        "No screen here — open .olp/watch.html on the machine that has the browser, or use `olp-hook watch` (text).".to_string()
    } else {
        format!("Open this file in your browser: {url}")
    }
}

/// For `setup`: adds a `.olp/` line to the repo's `.git/info/exclude` (unchanged if present; skipped if not a git repo). Returns a one-line notice.
pub fn ensure_git_exclude(project: &Path) -> String {
    let g = project.join(".git");
    let gitdir = if g.is_dir() {
        Some(g)
    } else {
        // worktree (.git is a file): the commondir under the gitdir points to the shared .git
        std::fs::read_to_string(&g).ok().and_then(|s| s.trim().strip_prefix("gitdir:").map(|t| project.join(t.trim()))).map(|gd| match std::fs::read_to_string(gd.join("commondir")) {
            Ok(c) => gd.join(c.trim()),
            Err(_) => gd,
        })
    };
    let Some(gitdir) = gitdir else {
        return "skipped: not a git repository here, so `.olp/` was not added to the git exclude list.".to_string();
    };
    let ex = gitdir.join("info").join("exclude");
    let mut s = std::fs::read_to_string(&ex).unwrap_or_default();
    if s.lines().any(|l| matches!(l.trim(), ".olp" | ".olp/" | "/.olp" | "/.olp/")) {
        return "`.olp/` is already in .git/info/exclude.".to_string();
    }
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(".olp/\n");
    let ok = std::fs::create_dir_all(gitdir.join("info")).and_then(|_| std::fs::write(&ex, s)).is_ok();
    if ok {
        "Added `.olp/` to .git/info/exclude so a bulk `git add` skips the device log.".to_string()
    } else {
        "could not update .git/info/exclude (nothing changed).".to_string()
    }
}

// ---- thin outer layer: subcommands ----

pub struct Env {
    pub is_terminal: bool,
    pub no_color: bool,
    pub term: Option<String>,
    pub ssh: bool,
    pub display: bool,
    pub wayland: bool,
}

impl Env {
    pub fn from_process() -> Env {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
        Env { is_terminal: std::io::stdout().is_terminal(), no_color: std::env::var_os("NO_COLOR").is_some(), term: std::env::var("TERM").ok(), ssh: set("SSH_CONNECTION"), display: set("DISPLAY"), wayland: set("WAYLAND_DISPLAY") }
    }
}

struct Opts {
    html: bool,
    once: bool,
    plain: bool,
    lang: String,
}

const USAGE: &str = "usage: olp-hook watch [--html] [--once] [--plain] [--lang en|ko]";

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { html: false, once: false, plain: false, lang: "en".to_string() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--html" => o.html = true,
            "--once" => o.once = true,
            "--plain" => o.plain = true,
            "--lang" => match it.next().map(|s| s.as_str()) {
                Some(l) if LANGS.contains(&l) => o.lang = l.to_string(),
                _ => return Err(USAGE.to_string()),
            },
            _ => return Err(USAGE.to_string()),
        }
    }
    Ok(o)
}

fn tick() {
    std::thread::sleep(std::time::Duration::from_millis(TICK_MS));
}

/// `--once` returns the rendered text; otherwise runs until Ctrl-C (ends when output is closed).
pub fn run(project: &Path, args: &[String], env: &Env) -> Result<String, String> {
    let o = parse(args)?;
    let mut tail = Tail::new(&store::dir(project).join("hook-log.jsonl"));
    tail.poll();
    if o.html {
        return run_html(project, &o, env, tail);
    }
    let color = stats::animate_ok(env.is_terminal, env.no_color, env.term.as_deref(), o.plain);
    if o.once {
        return Ok(text(&view(&tail.records, store::now()), color));
    }
    let mut out = std::io::stdout();
    let mut shown: Option<String> = None;
    let mut changed = true;
    loop {
        changed |= tail.poll();
        let frame = text(&view(&tail.records, store::now()), color);
        // The colored (terminal) view redraws even when only the seconds counter changes; the plain view prints another block only when the log changes.
        if (color && shown.as_deref() != Some(frame.as_str())) || (!color && changed) {
            let head = if color { "\x1b[H\x1b[J" } else { "\n" };
            if writeln!(out, "{head}{frame}").and_then(|_| out.flush()).is_err() {
                return Ok(String::new());
            }
            shown = Some(frame);
            changed = false;
        }
        tick();
    }
}

fn run_html(project: &Path, o: &Opts, env: &Env, mut tail: Tail) -> Result<String, String> {
    let dir = store::dir(project);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|_| "could not create .olp".to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let path = dir.join("watch.html");
    let abs = if path.is_absolute() { path.clone() } else { std::env::current_dir().map(|c| c.join(&path)).unwrap_or_else(|_| path.clone()) };
    let msg = start_message(&file_url(&abs.to_string_lossy()), headless(env.ssh, env.display, env.wayland, cfg!(target_os = "linux")));
    let mut last: Option<String> = None;
    if o.once {
        let html = render_html(&tail.records, store::now(), &o.lang, false);
        write_if_changed(&path, &html, &mut last).map_err(|_| "could not write .olp/watch.html".to_string())?;
        return Ok(msg);
    }
    let mut out = std::io::stdout();
    if writeln!(out, "{msg}").and_then(|_| out.flush()).is_err() {
        return Ok(String::new());
    }
    loop {
        tail.poll();
        let html = render_html(&tail.records, store::now(), &o.lang, true);
        // A write failure (e.g. browser holds the file) is retried next tick; the page's "updated N s ago" shows it is stale.
        let _ = write_if_changed(&path, &html, &mut last);
        tick();
    }
}
