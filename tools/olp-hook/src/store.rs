//! Two device files (under the project's `.olp/` folder; on Unix the folder is 0700 and files 0600):
//! - `hook-map.json` mapping: key (`tool:<tool name>` or `bash:<first word subcommand>`) → effect, reversibility, recovery declaration, declaration time, DB row count given by the user. No argument values, amounts or paths.
//! - `hook-log.jsonl` log: only what was shown on the card (kind, count, reversibility status, decision, time, human choice). No paths, command text or DB names.

use crate::classify::{Effect, Finding};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DbCount {
    ///     /// Treated as unknown if not an integer (including hand-edited values).
    pub n: serde_json::Value,
    pub declared_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub effect: Effect,
    ///     /// Whether the person answered "reversible". Without a recovery location declaration this is always false in the decision.
    pub reversible: bool,
    /// "declared" | "unknown"
    pub recovery: String,
    pub declared_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub db_count: Option<DbCount>,
}

impl Entry {
    pub fn recovery_declared(&self) -> bool {
        self.recovery == "declared"
    }
    ///     /// Row count the user gave for this entry (only if an integer).
    pub fn user_db_count(&self) -> Option<(u64, u64)> {
        let d = self.db_count.as_ref()?;
        d.n.as_u64().map(|n| (n, d.declared_at))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceMap {
    pub version: u32,
    pub entries: BTreeMap<String, Entry>,
}

pub fn dir(project: &Path) -> PathBuf {
    project.join(".olp")
}
fn map_path(project: &Path) -> PathBuf {
    dir(project).join("hook-map.json")
}
fn log_path(project: &Path) -> PathBuf {
    dir(project).join("hook-log.jsonl")
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn load_map(project: &Path) -> Result<DeviceMap, &'static str> {
    match std::fs::read(map_path(project)) {
        Ok(b) => serde_json::from_slice(&b).map_err(|_| "hook-map.json could not be read"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeviceMap { version: 1, entries: BTreeMap::new() }),
        Err(_) => Err("hook-map.json could not be read"),
    }
}

fn ensure_dir(project: &Path) -> std::io::Result<()> {
    let d = dir(project);
    if !d.exists() {
        std::fs::create_dir_all(&d)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn open_private(p: &Path, append: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true);
    if append {
        o.append(true);
    } else {
        o.write(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let f = o.open(p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(f)
}

pub fn save_map(project: &Path, m: &DeviceMap) -> std::io::Result<()> {
    ensure_dir(project)?;
    let s = serde_json::to_string_pretty(m).map_err(std::io::Error::other)?;
    let mut f = open_private(&map_path(project), false)?;
    f.write_all(s.as_bytes())?;
    f.write_all(b"\n")
}

/// /// One log line. Fields = only what was shown on the card.
#[derive(Debug, Clone, Serialize, Default)]
pub struct LogLine {
    pub ts: u64,
    /// "decision" | "human_choice"
    pub event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reversible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<&'static str>,
    ///     /// True if any count could not be determined or the decision itself could not be made
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unchecked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classified: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<&'static str>,
    ///     /// Blocked record = "not_chosen" (nothing chosen by a person at that time), classified command = "classified"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub human_choice: Option<&'static str>,
    ///     /// Fixed string on error or timeout (no path or command)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<&'static str>,
    ///     /// Permission-prompt pair: "card_shown" (PreToolUse ask) · "executed_after_card" (PostToolUse with the same id). A card_shown without a pair = not_run (counted by stats).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<&'static str>,
    ///     /// Claude Code's tool_use_id (device log only, never in server or stats output). Accepts only letters, digits and `_-`, up to 128 chars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// /// tool_use_id is not trusted as-is; accepted only if well-formed (no way to put arbitrary text in the log).
pub fn clean_tool_use_id(v: Option<&str>) -> Option<String> {
    let s = v?;
    (!s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')).then(|| s.to_string())
}

/// /// Whole device log (one JSON per line). Empty list if missing; corrupt lines are skipped.
pub fn read_log(project: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log_path(project))
        .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
        .unwrap_or_default()
}

pub fn today() -> u64 {
    now() / 86_400
}

fn first_run_path(project: &Path) -> PathBuf {
    dir(project).join("first-run")
}
fn used_day_path(project: &Path) -> PathBuf {
    dir(project).join("used-day")
}

/// /// On every hook run: first-run timestamp (once, when absent) + "used today" marker (written only when the day changes). Failure never affects the decision.
pub fn mark_run(project: &Path) {
    if ensure_dir(project).is_err() {
        return;
    }
    let fr = first_run_path(project);
    if !fr.exists() {
        if let Ok(mut f) = open_private(&fr, false) {
            let _ = f.write_all(now().to_string().as_bytes());
        }
    }
    let d = today().to_string();
    let ud = used_day_path(project);
    if std::fs::read_to_string(&ud).map(|s| s.trim() != d).unwrap_or(true) {
        if let Ok(mut f) = open_private(&ud, false) {
            let _ = f.write_all(d.as_bytes());
        }
    }
}

pub fn first_run(project: &Path) -> Option<u64> {
    std::fs::read_to_string(first_run_path(project)).ok()?.trim().parse().ok()
}

pub fn used_today(project: &Path) -> bool {
    std::fs::read_to_string(used_day_path(project)).map(|s| s.trim() == today().to_string()).unwrap_or(false)
}

/// /// Day the hook last ran (UTC day number, `used-day`). None if absent.
pub fn last_used_day(project: &Path) -> Option<u64> {
    std::fs::read_to_string(used_day_path(project)).ok()?.trim().parse().ok()
}

/// /// Read/write of a device file outside the repository (usage-consent record, `<home>/olp/usage/…`); called only by the signal module. None if corrupt or missing.
pub fn read_json_file(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(p).ok()?).ok()
}
pub fn write_private_json(p: &Path, v: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let mut f = open_private(p, false)?;
    f.write_all(v.to_string().as_bytes())
}

pub fn append_log(project: &Path, l: &LogLine) -> std::io::Result<()> {
    ensure_dir(project)?;
    let mut s = serde_json::to_string(l).map_err(std::io::Error::other)?;
    s.push('\n');
    let mut f = open_private(&log_path(project), true)?;
    // One write per record: with append mode, concurrent hook calls then never glue two records onto one line.
    f.write_all(s.as_bytes())
}
