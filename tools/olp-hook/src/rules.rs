//! Minimal first version: counts the broad entries among Claude Code's "always allow" (permissions.allow) rules that can reach
//! deletes/overwrites and shows one line at the end of setup. Read-only - settings files are never written (removal is left to a later `olp rules remove`).
//! Broad = a wildcard rule (or a bare tool name) such that, with a delete / force-push style suffix appended to its prefix, the hook classifier (classify::assess) finds a risky pattern.
//! Exact-command rules (what "don't ask again" stores) only match the same text, so they are not counted (no scare tactics).
//! Managed (enterprise) settings are not read (out of scope).

use crate::classify::{self, Limits};
use crate::store::DeviceMap;
use serde_json::{json, Value};
use std::path::Path;

/// Max rules shown by name on screen (the rest are summarized as "and N more").
const SHOW: usize = 5;

/// Suffix appended after the prefix: does any command the rule accepts contain a risky pattern?
const BASH_TAILS: [&str; 4] = ["x", "--force x", "push --force origin x", "reset --hard"];
const PS_TAILS: [&str; 2] = ["x", "-Recurse -Force x"];

/// Is this a broad "always allow" rule (see module docs)?
pub fn broad(rule: &str) -> bool {
    let r = rule.trim();
    if r == "Bash" || r == "PowerShell" {
        return true;
    }
    if r.starts_with("mcp__") {
        return r.contains('*');
    }
    let (tool, tails): (&str, &[&str]) = if let Some(inner) = r.strip_prefix("Bash(").and_then(|s| s.strip_suffix(')')) {
        (inner, &BASH_TAILS)
    } else if let Some(inner) = r.strip_prefix("PowerShell(").and_then(|s| s.strip_suffix(')')) {
        (inner, &PS_TAILS)
    } else {
        return false;
    };
    let inner = tool.trim();
    if !inner.ends_with('*') {
        return false;
    }
    let prefix = inner.trim_end_matches('*').trim_end_matches(':').trim();
    if prefix.is_empty() {
        return true;
    }
    let shell = if r.starts_with("Bash(") { "Bash" } else { "PowerShell" };
    let map = DeviceMap::default();
    tails.iter().any(|t| {
        let a = classify::assess(shell, &json!({ "command": format!("{prefix} {t}") }), &map, Path::new("."), Limits { counter: None, ps_parser: shell == "PowerShell" && expression(prefix) });
        !a.findings.is_empty()
    })
}

/// Is a PowerShell rule prefix an expression (`[IO.File]::Delete(…)`, `$f.Delete()`, `(…)`, `{…}`, `;`) rather than a command name with arguments?
/// Only for these can the PowerShell parser find a delete the built-in list misses (it only adds findings), so only these keep the parser in the rule check
/// (review C1 of DESIGN_INSTALL_PROGRESS_012). Command-name rules take the same path as on macOS/Linux: no powershell.exe per rule.
fn expression(prefix: &str) -> bool {
    prefix.contains("::") || prefix.contains(['$', '(', ')', '[', ']', '{', '}', ';', '`'])
}

/// Prefix of a wildcard PowerShell rule, if it is one (`PowerShell(<prefix>*)`).
fn ps_prefix(rule: &str) -> Option<&str> {
    let inner = rule.trim().strip_prefix("PowerShell(")?.strip_suffix(')')?.trim();
    inner.ends_with('*').then(|| inner.trim_end_matches('*').trim_end_matches(':').trim())
}

/// Max characters of a rule name shown on screen.
const NAME_MAX: usize = 80;

/// Result: broad rules (in first-seen order, duplicates once) + number of settings files that exist but could not be read (malformed).
#[derive(Debug, Default, PartialEq)]
pub struct Scan {
    pub rules: Vec<String>,
    pub unreadable: usize,
}

/// None = file exists but could not be read (I/O error or not JSON). Missing file = empty list.
fn allow_rules(file: &Path) -> Option<Vec<String>> {
    let t = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    let v = serde_json::from_str::<Value>(&t).ok()?;
    Some(v["permissions"]["allow"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_string)).collect())
}

fn settings_files(project: &Path, home: &Path) -> [std::path::PathBuf; 3] {
    [home.join(".claude").join("settings.json"), project.join(".claude").join("settings.json"), project.join(".claude").join("settings.local.json")]
}

/// How many distinct rules the check will run through the PowerShell parser (expression prefixes, parser available). Install prints the long progress line only when this is above 0 (review C2), so "this can take a minute" is said only when a powershell.exe start per rule really follows.
pub fn parser_rules(project: &Path, home: &Path) -> usize {
    if !crate::ps_ast::enabled() {
        return 0;
    }
    let mut seen: Vec<String> = Vec::new();
    for f in &settings_files(project, home) {
        for r in allow_rules(f).unwrap_or_default() {
            if ps_prefix(&r).is_some_and(|p| !p.is_empty() && expression(p)) && !seen.contains(&r) {
                seen.push(r);
            }
        }
    }
    seen.len()
}

/// Reads the user, project and local settings files (read-only). Missing files are skipped; malformed ones are counted in `unreadable` (never masked as "no rules").
pub fn scan(project: &Path, home: &Path) -> Scan {
    let files = settings_files(project, home);
    let mut out = Scan::default();
    for f in &files {
        let Some(list) = allow_rules(f) else {
            out.unreadable += 1;
            continue;
        };
        for r in list {
            if broad(&r) && !out.rules.contains(&r) {
                out.rules.push(r);
            }
        }
    }
    out
}

/// Is the name safe to show? Only printable ASCII and spaces, at most 80 chars (so a repo's settings text cannot forge the notice line - no newline, ESC or backtick).
fn showable(r: &str) -> bool {
    r.len() <= NAME_MAX && r.chars().all(|c| (c.is_ascii_graphic() || c == ' ') && c != '`')
}

/// One line at the end of setup. Facts only: rule count, names (filtered ones only), what the rules mean (Claude Code may run them without asking). It does not claim whether OLP stops before them (not measured).
pub fn summary(rules: &[String], unreadable: usize) -> String {
    let unread = match unreadable {
        0 => String::new(),
        1 => "1 settings file could not be read".to_string(),
        n => format!("{n} settings files could not be read"),
    };
    if rules.is_empty() {
        return if unreadable == 0 {
            "Permission rules: no 'always allow' rule covers commands that delete or overwrite files.".to_string()
        } else {
            format!("Permission rules: no 'always allow' rule found that covers commands that delete or overwrite files ({unread}).")
        };
    }
    let shown: Vec<&String> = rules.iter().filter(|r| showable(r)).collect();
    let hidden = rules.len() - shown.len();
    let mut names: Vec<String> = shown.iter().take(SHOW).map(|r| format!("`{r}`")).collect();
    if shown.len() > SHOW {
        names.push(format!("and {} more", shown.len() - SHOW));
    }
    if hidden > 0 {
        names.push(format!("{hidden} not shown (unusual characters or too long)"));
    }
    let (n, verb) = if rules.len() == 1 { ("1 'always allow' rule".to_string(), "covers") } else { (format!("{} 'always allow' rules", rules.len()), "cover") };
    let tail = if unreadable == 0 { String::new() } else { format!(" ({unread}.)") };
    format!("Permission rules: {n} {verb} commands that can delete or overwrite: {}. They tell Claude Code it may run matching commands without asking you. Nothing was changed.{tail}", names.join(", "))
}
