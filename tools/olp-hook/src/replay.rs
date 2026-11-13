//! `olp-hook replay` (0.1.3, DESIGN_OLP_REPLAY_013): re-judges the Bash/PowerShell commands in local Claude Code records with
//! the same `hook::decide` the hook uses. Runs none of the recorded commands (judging may start PowerShell's parser and
//! read-only git commands), sends nothing, writes nothing in any project. The only write is
//! `<home>/olp/replay-count.json` (`{"runs": n}`, no command text, never sent).

use crate::classify::Limits;
use crate::config;
use crate::hook::{decide, GuardFail, HookRequest, Route};
use crate::store;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DAY: u64 = 86_400;
const DEFAULT_DAYS: u64 = 30;
/// Recorded commands are never run. Judging may start PowerShell's parser and read-only git commands in the recorded folder
/// (second review C1), so the line says «none of these commands», not «nothing».
const LAST_LINE: &str = "OLP sent nothing and ran none of these commands.";
const IN_CLAUDE_CODE: &str = "  run this in your own terminal to see the commands";

/// What the replay needs from the outside (kept apart so tests can fix the clock and the home folder).
pub struct ReplayEnv {
    pub home: PathBuf,
    pub now: u64,
    /// `CLAUDECODE` is set: the output may land in a model's context, so command text is never printed.
    pub claudecode: bool,
    pub version: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub commands: u64,
    pub occurrences: u64,
    pub pass: u64,
    pub ask: u64,
    pub ask_unclassified: u64,
    pub block: u64,
    pub timeout: u64,
    pub errors: u64,
    pub skipped: u64,
    pub broken: u64,
    pub by_kind: BTreeMap<String, u64>,
    pub shown: Vec<String>,
    pub notes: Vec<String>,
}

struct Opts {
    dir: Option<PathBuf>,
    days: u64,
    today: bool,
    show: usize,
    json: bool,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Cmd {
    tool: String,
    command: String,
    cwd: Option<PathBuf>,
    mode: String,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { dir: None, days: DEFAULT_DAYS, today: false, show: 0, json: false };
    let mut i = 0;
    let num = |v: Option<&String>, name: &str| v.and_then(|s| s.parse::<u64>().ok()).ok_or(format!("{name} needs a number"));
    while i < args.len() {
        match args[i].as_str() {
            "--dir" => {
                o.dir = Some(PathBuf::from(args.get(i + 1).ok_or("--dir needs a folder")?));
                i += 1;
            }
            "--days" => {
                o.days = num(args.get(i + 1), "--days")?.max(1);
                i += 1;
            }
            "--show" => {
                o.show = num(args.get(i + 1), "--show")? as usize;
                i += 1;
            }
            "--today" => o.today = true,
            "--json" => o.json = true,
            other => return Err(format!("usage: olp-hook replay [--days N] [--today] [--dir PATH] [--show N] [--json] (unknown: {other})")),
        }
        i += 1;
    }
    Ok(o)
}

/// "2026-11-12T03:04:05.123Z" -> unix seconds (UTC). None if the shape is not that.
fn iso_secs(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    let (y, m, d, hh, mm, ss) = (n(0, 4)?, n(5, 7)?, n(8, 10)?, n(11, 13)?, n(14, 16)?, n(17, 19)?);
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            jsonl_files(&p, out);
        } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
}

/// Every Bash/PowerShell tool call in the window, one entry per recorded call. Broken lines are only counted.
/// Only `type: "assistant"` lines are read: records also carry request copies (`api-request-blob`) that repeat earlier
/// tool_use blocks, and resumed or copied records can repeat a line, so each tool_use id is counted once (review B C1).
fn collect(dir: &Path, since: u64, broken: &mut u64, no_mode: &mut u64) -> Vec<Cmd> {
    let mut files = vec![];
    jsonl_files(dir, &mut files);
    files.sort();
    let mut out = vec![];
    let mut seen_ids = std::collections::HashSet::new();
    for f in files {
        let mtime = fs::metadata(&f).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
        let Ok(text) = fs::read_to_string(&f) else {
            *broken += 1; // not UTF-8 (or unreadable): counted, never silently dropped (second review memo)
            continue;
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                *broken += 1;
                continue;
            };
            if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                continue;
            }
            let Some(content) = v.pointer("/message/content").and_then(|c| c.as_array()) else { continue };
            let ts = v.get("timestamp").and_then(|t| t.as_str()).and_then(iso_secs).or(mtime);
            if ts.is_none_or(|t| t < since) {
                continue;
            }
            for c in content {
                let tool = c.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if c.get("type").and_then(|t| t.as_str()) != Some("tool_use") || !matches!(tool, "Bash" | "PowerShell") {
                    continue;
                }
                let Some(command) = c.pointer("/input/command").and_then(|x| x.as_str()) else { continue };
                if let Some(id) = c.get("id").and_then(|x| x.as_str()) {
                    if !seen_ids.insert(id.to_string()) {
                        continue;
                    }
                }
                let mode = match v.get("permissionMode").and_then(|m| m.as_str()) {
                    Some(m) => m.to_string(),
                    None => {
                        *no_mode += 1;
                        "auto".to_string()
                    }
                };
                let cwd = v.get("cwd").and_then(|x| x.as_str()).map(PathBuf::from);
                out.push(Cmd { tool: tool.to_string(), command: command.to_string(), cwd, mode });
            }
        }
    }
    out
}

enum Outcome {
    Pass,
    Ask { unclassified: bool, kinds: Vec<String> },
    Block { kinds: Vec<String> },
    Timeout,
    Error,
    Skipped,
}

/// Same verdict as the hook: config and answers of the recorded working folder, the hook's own limits.
fn judge(c: &Cmd) -> Outcome {
    let Some(project) = c.cwd.as_deref().filter(|p| p.is_dir()) else { return Outcome::Skipped };
    let (Ok(cfg), Ok(map)) = (config::load(project), store::load_map(project)) else { return Outcome::Skipped };
    let limits = Limits { counter: cfg.counter_limit(), ps_parser: true };
    let req = HookRequest { tool: c.tool.clone(), input: json!({ "command": c.command }), cwd: project.to_path_buf(), mode: Some(c.mode.clone()) };
    match decide(req, project, &cfg, Arc::new(map), limits, cfg.hook_budget()) {
        Ok(d) => {
            let kinds: Vec<String> = d.a.findings.iter().map(|f| serde_json::to_value(f.kind).ok().and_then(|k| k.as_str().map(String::from)).unwrap_or_default()).collect();
            match d.route {
                Route::Read | Route::QuietPass => Outcome::Pass,
                Route::Ask => Outcome::Ask { unclassified: !d.a.to_classify.is_empty() || d.a.unnamed, kinds },
                Route::CwdDeny | Route::Block { .. } => Outcome::Block { kinds },
            }
        }
        Err(GuardFail::Timeout) => Outcome::Timeout,
        Err(GuardFail::Panic) => Outcome::Error,
    }
}

fn report(cmds: &[Cmd], show: usize, claudecode: bool) -> Report {
    let mut r = Report { occurrences: cmds.len() as u64, ..Default::default() };
    let mut groups: BTreeMap<&Cmd, u64> = BTreeMap::new();
    for c in cmds {
        *groups.entry(c).or_default() += 1;
    }
    let mut texts = std::collections::BTreeSet::new();
    for (c, n) in groups {
        texts.insert(c.command.as_str());
        let mut flagged = false;
        match judge(c) {
            Outcome::Pass => r.pass += n,
            Outcome::Ask { unclassified, kinds } => {
                r.ask += n;
                if unclassified {
                    r.ask_unclassified += n;
                }
                for k in kinds.into_iter().filter(|k| !k.is_empty()) {
                    *r.by_kind.entry(k).or_default() += n;
                }
                flagged = true;
            }
            Outcome::Block { kinds } => {
                r.block += n;
                for k in kinds.into_iter().filter(|k| !k.is_empty()) {
                    *r.by_kind.entry(k).or_default() += n;
                }
                flagged = true;
            }
            Outcome::Timeout => r.timeout += n,
            Outcome::Error => r.errors += n,
            Outcome::Skipped => r.skipped += n,
        }
        if flagged && !claudecode && r.shown.len() < show {
            r.shown.push(c.command.clone());
        }
    }
    r.commands = texts.len() as u64;
    r
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn card(r: &Report, span: &str, version: &str, show: usize, claudecode: bool) -> String {
    let mut out = vec![
        format!("OLP {version} replay — {span}, {} commands ({} runs)", thousands(r.commands), thousands(r.occurrences)),
        format!("  passed silently                    {}", thousands(r.pass)),
        if r.ask_unclassified > 0 {
            format!("  would have asked                   {}   (not classified yet {})", thousands(r.ask), thousands(r.ask_unclassified))
        } else {
            format!("  would have asked                   {}", thousands(r.ask))
        },
        format!("  would have been stopped by a rule  {}", thousands(r.block)),
    ];
    let unjudged = r.timeout + r.errors + r.skipped;
    if unjudged > 0 {
        out.push(format!("  could not judge                    {}", thousands(unjudged)));
    }
    if show > 0 {
        if claudecode {
            out.push(IN_CLAUDE_CODE.to_string());
        } else {
            out.extend(r.shown.iter().map(|c| format!("  · {}", c.replace(['\n', '\r'], " "))));
        }
    }
    out.push(LAST_LINE.to_string());
    out.join("\n")
}

/// Design §3 differences (where the same `decide` can still differ from what the hook did), one fact line each, only when present.
fn difference_notes(cmds: &[Cmd]) -> Vec<String> {
    let mut notes = vec![];
    if cmds.is_empty() {
        return notes;
    }
    notes.push("each command was judged in its recorded working folder; the hook uses CLAUDE_PROJECT_DIR first, so commands run in a subfolder may show more «would have asked» than the hook gave".to_string());
    let projects: std::collections::BTreeSet<&Path> = cmds.iter().filter_map(|c| c.cwd.as_deref()).filter(|p| p.is_dir()).collect();
    let (mut pick, mut decision) = (0, 0);
    for p in &projects {
        if let Ok(cfg) = config::load(p) {
            pick += u64::from(cfg.pick);
            decision += u64::from(cfg.decision);
        }
    }
    let unread = cmds.iter().filter(|c| c.cwd.as_deref().is_some_and(|p| p.is_dir() && (config::load(p).is_err() || store::load_map(p).is_err()))).count();
    let n = |k: u64, one: &str, many: &str| if k == 1 { format!("1 {one}") } else { format!("{k} {many}") };
    if pick > 0 {
        notes.push(format!("{} pick on; a file a person chose to keep is denied by the hook but counted here as «would have asked»", n(pick, "project has", "projects have")));
    }
    if decision > 0 {
        notes.push(format!("{} decision on; the hook stops once for a decision there, which is not counted here", n(decision, "project has", "projects have")));
    }
    if unread > 0 {
        notes.push(format!("{} in a project whose OLP settings could not be read; the hook blocks these, here they count as «could not judge»", n(unread as u64, "command was", "commands were")));
    }
    notes
}

/// Adds 1 to `<home>/olp/replay-count.json`. A failure here never changes the card or the exit code.
fn count_run(home: &Path) {
    let dir = home.join("olp");
    let path = dir.join("replay-count.json");
    let runs = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.get("runs").and_then(|n| n.as_u64())).unwrap_or(0);
    if fs::create_dir_all(&dir).is_ok() {
        let _ = fs::write(&path, json!({ "runs": runs + 1 }).to_string());
    }
}

pub fn default_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}

pub fn replay_cli(args: &[String], env: &ReplayEnv) -> Result<String, String> {
    let o = parse(args)?;
    let dir = o.dir.clone().unwrap_or_else(|| default_dir(&env.home));
    let window = if o.today { DAY } else { o.days * DAY };
    let span = if o.today { "last 24 hours".to_string() } else { format!("last {} days", o.days) };
    let (mut broken, mut no_mode) = (0, 0);
    let cmds = collect(&dir, env.now.saturating_sub(window), &mut broken, &mut no_mode);
    let mut r = report(&cmds, o.show, env.claudecode);
    r.broken = broken;
    if no_mode > 0 {
        r.notes.push(format!("{no_mode} records had no permission mode; they were judged as auto"));
    }
    r.notes.extend(difference_notes(&cmds));
    count_run(&env.home);
    if o.json {
        return serde_json::to_string(&r).map_err(|e| e.to_string());
    }
    if r.occurrences == 0 {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Ok(format!("No Claude Code records in the {span} (looked in {name})."));
    }
    Ok(card(&r, &span, &env.version, o.show, env.claudecode))
}
