//! Hook-side "picked files only" support for the companion GUI.
//! What it does: (1) sweep stale pending/choice files at hook start; (2) treat `olp-gui` as alive if `.olp/gui.alive` was modified within 15 s;
//! (3) write the delete card's file list to `.olp/pending/<id>.json` (project-relative names); (4) compare the server-written choice with this delete's file set -> one card line.
//! The verdict is never loosened (no allow): choice and alive are low-trust inputs that a same-machine agent can also write, so they only inform card text.
//! Exception: if a file the user chose to keep is among the next delete's targets, that command is denied with a "re-run without it" reason (blocking only).
//! The permanent hook log never records names; names live only in the short-lived pending file (<= 10 min).

use crate::counter;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

/// Lifetime (seconds) of pending/choice files; older ones are swept and ignored when matching.
pub const TTL_S: u64 = 600;
/// Liveness window (seconds) for the GUI server.
pub const ALIVE_S: u64 = 15;
/// Max bytes read from one choice/pending file; larger counts as absent.
pub const READ_MAX: u64 = 1024 * 1024;
/// Cap (milliseconds) on the file-kind lookup (never longer than the hook's remaining budget).
pub const NATURE_MS: u64 = 3000;
/// Max bytes for one file name (same as the server's filter).
const NAME_MAX: usize = 1024;

/// Choice vocabulary: the same nouns as the GUI, Keep / Delete.
pub const PICK_LINE: &str = "Mark Keep or Delete in the GUI window";
/// Line after the pick hint: the first card is decided before any pick, so if the user chose Keep they should answer No in this dialog (the next attempt will then omit the kept file).
pub const ANSWER_NO: &str = "If any file is marked Keep, answer No here; the agent's next try must leave them out.";
/// Shown when picking is enabled but the GUI is not running: only the command that starts it, without address or key (the key stays in that terminal).
pub const GUI_HINT: &str = "To mark files Keep or Delete, run npx @feedpia/olp gui serve in another terminal";

/// Up to 3 names of files to delete (sorted) plus "and N more" (N = total - shown). None if any name is outside the project.
pub fn names_line(project: &Path, files: &[PathBuf], total: u64) -> Option<String> {
    let mut names = rel_names(project, files)?;
    if names.is_empty() {
        return None;
    }
    names.sort();
    let shown: Vec<String> = names.into_iter().take(3).collect();
    let more = total.saturating_sub(shown.len() as u64);
    let tail = if more > 0 { format!(" and {more} more") } else { String::new() };
    Some(format!("Files: {}{tail}", shown.join(", ")))
}

fn dir(project: &Path) -> PathBuf {
    project.join(".olp").join("pending")
}

fn now_s() -> u64 {
    crate::store::now()
}

/// Same character rules as `store::clean_tool_use_id` (1-128 chars, `[A-Za-z0-9_-]`).
fn id_ok(s: &str) -> bool {
    crate::store::clean_tool_use_id(Some(s)).is_some()
}

/// Returns the id for `<id>.json` or `<id>.choice.json` (anything else, including `.tmp`, is None).
fn pending_id(file: &str) -> Option<(&str, bool)> {
    if let Some(id) = file.strip_suffix(".choice.json") {
        return id_ok(id).then_some((id, true));
    }
    let id = file.strip_suffix(".json")?;
    id_ok(id).then_some((id, false))
}

/// On every hook start: remove only pending/choice files with well-formed names whose mtime is older than 10 minutes. Failures are silent (no effect on the verdict).
pub fn sweep(project: &Path) {
    let Ok(rd) = std::fs::read_dir(dir(project)) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if pending_id(name).is_none() {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        if m.is_dir() {
            continue;
        }
        let old = m.modified().ok().and_then(|t| t.elapsed().ok()).map(|age| age > Duration::from_secs(TTL_S)).unwrap_or(false);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Server is alive if the mtime of `.olp/gui.alive` is within 15 s either way (content is not read).
pub fn gui_alive(project: &Path) -> bool {
    let Ok(t) = std::fs::metadata(project.join(".olp").join("gui.alive")).and_then(|m| m.modified()) else { return false };
    let lim = Duration::from_secs(ALIVE_S);
    match t.elapsed() {
        Ok(age) => age <= lim,
        Err(e) => e.duration() <= lim, // file with a slightly future mtime (clock skew)
    }
}

/// Normalizes `.` and `..` textually (no filesystem access). None if it climbs above the root.
fn lexical(p: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    return None;
                }
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

/// Project-relative name (`/`-separated). None if any path is outside the project or malformed (= no pending file).
pub fn rel_names(project: &Path, files: &[PathBuf]) -> Option<Vec<String>> {
    let base = lexical(project)?;
    let bc: Vec<Component> = base.components().collect();
    // Windows paths are case-insensitive, so the prefix comparison ignores case too
    let same = |a: &Component, b: &Component| {
        if cfg!(windows) {
            a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
        } else {
            a == b
        }
    };
    files
        .iter()
        .map(|f| {
            let full = lexical(f)?;
            let fc: Vec<Component> = full.components().collect();
            if fc.len() <= bc.len() || !bc.iter().zip(&fc).all(|(a, b)| same(a, b)) {
                return None;
            }
            let parts: Vec<&str> = fc[bc.len()..].iter().map(|c| match c {
                Component::Normal(s) => s.to_str(),
                _ => None,
            }).collect::<Option<_>>()?;
            let name = parts.join("/");
            let ok = !name.is_empty() && name.len() <= NAME_MAX && !name.contains(':') && !name.chars().any(|ch| ch.is_control()) && !name.starts_with('~');
            ok.then_some(name)
        })
        .collect()
}

/// Write pending: temp file then rename; Unix mode 0600 (on Windows the folder ACL is inherited).
fn write_pending(project: &Path, id: &str, names: &[String], natures: &[&str], more_n: u64) -> std::io::Result<()> {
    let files: Vec<Value> = names.iter().zip(natures).enumerate().map(|(i, (n, k))| json!({"index": i, "name": n, "nature": k})).collect();
    let body = json!({"v": 1, "id": id, "created": now_s(), "files": files, "more_n": more_n}).to_string();
    let d = dir(project);
    std::fs::create_dir_all(&d)?;
    let tmp = d.join(format!("{id}.json.tmp"));
    {
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        use std::io::Write;
        o.open(&tmp)?.write_all(body.as_bytes())?;
    }
    std::fs::rename(&tmp, d.join(format!("{id}.json")))
}

/// Read a low-trust file: over 1 MiB or malformed = None.
fn read_small(p: &Path) -> Option<Value> {
    let mut s = String::new();
    std::fs::File::open(p).ok()?.take(READ_MAX + 1).read_to_string(&mut s).ok()?;
    if s.len() as u64 > READ_MAX {
        return None;
    }
    serde_json::from_str(&s).ok()
}

fn fresh(t: u64, now: u64) -> bool {
    t <= now + 60 && now.saturating_sub(t) <= TTL_S
}

/// One live choice: names picked for deletion, names picked to keep (in pending order), and that pending's more_n.
struct Choice {
    gone: HashSet<String>,
    kept: Vec<String>,
    more: u64,
}

/// The most recent live choice by `at`.
fn latest_choice(project: &Path) -> Option<Choice> {
    let now = now_s();
    let mut best: Option<(u64, Choice)> = None;
    for e in std::fs::read_dir(dir(project)).ok()?.flatten() {
        let name = e.file_name();
        let Some((id, true)) = name.to_str().and_then(pending_id) else { continue };
        let Some(c) = read_small(&e.path()) else { continue };
        let at = c["at"].as_u64().unwrap_or(0);
        if c["v"] != 1 || c["id"] != id || !fresh(at, now) {
            continue;
        }
        let Some(p) = read_small(&dir(project).join(format!("{id}.json"))) else { continue };
        if p["v"] != 1 || p["id"] != id || !fresh(p["created"].as_u64().unwrap_or(0), now) {
            continue;
        }
        let Some(files) = p["files"].as_array() else { continue };
        let Some(keep) = c["keep"].as_array() else { continue };
        let keep: Option<HashSet<u64>> = keep.iter().map(|k| k.as_u64().filter(|k| (*k as usize) < files.len())).collect();
        let Some(keep) = keep else { continue };
        let gone: Option<HashSet<String>> = files
            .iter()
            .filter(|f| !f["index"].as_u64().map(|i| keep.contains(&i)).unwrap_or(false))
            .map(|f| f["name"].as_str().map(String::from))
            .collect();
        let Some(gone) = gone else { continue };
        let kept: Option<Vec<String>> = files
            .iter()
            .filter(|f| f["index"].as_u64().map(|i| keep.contains(&i)).unwrap_or(false))
            .map(|f| f["name"].as_str().map(String::from))
            .collect();
        let Some(kept) = kept else { continue };
        let more = p["more_n"].as_u64().unwrap_or(u64::MAX);
        if best.as_ref().map(|b| at > b.0).unwrap_or(true) {
            best = Some((at, Choice { gone, kept, more }));
        }
    }
    best.map(|(_, c)| c)
}

/// Honor the keep choice: kept files of the live selection that are among this delete's targets (pending order). None if there are none.
/// The choice file is low-trust input, so this path is used only to block (deny); it never produces an allow.
pub fn kept_in_target(project: &Path, files: &[PathBuf]) -> Option<Vec<String>> {
    let names: HashSet<String> = rel_names(project, files)?.into_iter().collect();
    let hit: Vec<String> = latest_choice(project)?.kept.into_iter().filter(|k| names.contains(k)).collect();
    (!hit.is_empty()).then_some(hit)
}

/// Reason returned to the agent: up to 3 names plus "and N more".
pub fn keep_reason(kept: &[String]) -> String {
    let shown = kept.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    let more = if kept.len() > 3 { format!(" and {} more", kept.len() - 3) } else { String::new() };
    format!("The person chose to keep these files in the OLP picker: {shown}{more}. This command would delete them, so it did not run. Run it again without them.")
}

fn n_files(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

/// One pick line for a delete card. `files` = first 200 counted by the counter, `total` = full count.
/// With an id, always writes this card's pending file (so the GUI can pick for this card too).
/// Card line: if a live choice exists, compare ("matches your pick (n files)" / "differs from your pick"); otherwise show the hint.
/// Files outside the project = None (plain text, no pending). The hint line is shown only when pending was actually written.
pub fn card_line(project: &Path, id: Option<&str>, files: &[PathBuf], total: u64, deadline: Instant) -> Option<String> {
    let names = rel_names(project, files)?;
    let compared = latest_choice(project).map(|Choice { gone, more, .. }| {
        let cur: HashSet<String> = names.iter().cloned().collect();
        let complete = more == 0 && total == names.len() as u64;
        if complete && !gone.is_empty() && cur == gone { format!("matches your pick ({})", n_files(gone.len())) } else { "differs from your pick".to_string() }
    });
    let written = id.is_some_and(|id| {
        let natures = counter::natures(project, &names, files, deadline);
        write_pending(project, id, &names, &natures, total.saturating_sub(names.len() as u64)).is_ok()
    });
    compared.or_else(|| written.then(|| if total > names.len() as u64 { format!("{PICK_LINE} (first {} of {total} files listed). {ANSWER_NO}", names.len()) } else { format!("{PICK_LINE}. {ANSWER_NO}") }))
}

/// Read cap = min(3 s, remaining hook budget).
pub fn nature_deadline(start: Instant, budget: Option<Duration>) -> Instant {
    let cap = Duration::from_millis(NATURE_MS);
    let left = budget.map(|b| b.saturating_sub(start.elapsed())).unwrap_or(cap);
    Instant::now() + cap.min(left)
}
