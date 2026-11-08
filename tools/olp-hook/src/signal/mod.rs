//! Opt-in usage numbers. Disabled by default.
//! - Consent: asked at the end of `npx … install` (interactive terminal only) or by `olp-hook setup` (default = no).
//!   The answer is stored only in a per-device file outside the repository, `<home>/olp/usage/<project path hash>.json`; a file placed in or committed to the repository is not consent.
//! - `olp-hook signal`: once a day, sends only the closed field list v2 (`FIELDS`, same as the README table). Numeric fields count device records whose 10-minute window closed since the last send.
//!   The endpoint comes from the bundled data file (or an `endpoint` in the home record, written by a person outside the repository). Project settings can only change it to an empty string (off). `OLP_HOOK_SIGNAL=off` always wins.
//! - Sending goes only through the system curl via this module's spawn.rs (closed argument list, user curl config ignored, at most 3 s, no redirects). The day is recorded before the attempt, so there are no retries.
//! Hook, counting and PostToolUse paths never call this module (`hook_paths_never_touch_signal_wrapper`).

pub mod spawn;

use crate::{config, stats, store};
use serde_json::{json, Value};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

/// /// Closed list of fields sent, v2 (README "Optional daily usage numbers" table = server = this list; test t8 cross-checks them).
pub const FIELDS: [&str; 11] = ["v", "install_id", "hook_version", "install_path", "used_today", "age_days", "cards_shown", "blocked", "decision_on", "decision_asked", "after_decision"];
/// /// Upper bound for integer fields (the server validates 0..=10000 and an overflow would zero the whole day, so values are clamped before sending).
const CAP: u64 = 10_000;
/// /// Window (seconds) in which a "next delete" after a decision card is observed = the same 10 minutes as the block limit in decision.rs.
const AFTER_S: u64 = crate::decision::ASKED_S;

pub enum Plan {
    ///     /// Not sent (fixed reason string)
    Off(&'static str),
    ///     /// Ready to send: curl args + body + end time of the records counted this time (written to the home record on send)
    Ready { argv: Vec<String>, body: String, counted_to: u64 },
}

fn random_id() -> String {
    let mut parts = String::new();
    for i in 0..2u64 {
        //         // RandomState seeds its key from OS randomness per process (no new crate). Not a secret; this is an anonymous bundle id.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u32(std::process::id());
        parts.push_str(&format!("{:016x}", h.finish()));
    }
    //     // UUIDv4 shape (36 lowercase chars): 13th char = 4, 17th char = 8..b (install_id shape).
    let mut c: Vec<char> = parts.chars().collect();
    c[12] = '4';
    c[16] = ['8', '9', 'a', 'b'][(c[16].to_digit(16).unwrap_or(0) % 4) as usize];
    let s: String = c.into_iter().collect();
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

/// /// install_id shape: sent only when it is a 36-char lowercase UUIDv4, so hand-edited text can never reach the body.
pub fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { *c == b'-' } else { c.is_ascii_digit() || (b'a'..=b'f').contains(c) })
        && b[14] == b'4'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b')
}

/// /// True if `OLP_HOOK_SIGNAL=off` (case-insensitive); always wins over consent and settings.
pub fn env_off(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// /// Days since install: integer 0..31 from the first-run timestamp (31 = 31 days or more). No record or clock going backwards = 0.
pub fn age_days(first_run: Option<u64>, now: u64) -> u64 {
    first_run.map_or(0, |f| (now.saturating_sub(f) / 86_400).min(31))
}

/// /// Consent record location: `<home>/olp/usage/<FNV-1a 64 hash of the canonical project path>.json`. No path characters in the file name.
pub fn record_path(project: &Path, home: &Path) -> PathBuf {
    let key = std::fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf()).to_string_lossy().replace('\\', "/");
    let key = if cfg!(windows) { key.to_lowercase() } else { key };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    home.join("olp").join("usage").join(format!("{h:016x}.json"))
}

fn read_record(project: &Path, home: &Path) -> Option<Value> {
    store::read_json_file(&record_path(project, home))
}

fn write_record(project: &Path, home: &Path, v: &Value) -> Result<(), String> {
    store::write_private_json(&record_path(project, home), v).map_err(|_| "could not write the usage-numbers record in your home folder".to_string())
}

/// /// Whether this project has already been asked (install asks only once).
pub fn asked(project: &Path, home: &Path) -> bool {
    read_record(project, home).is_some()
}

/// /// Whether consent is currently yes.
pub fn opted_in(project: &Path, home: &Path) -> bool {
    read_record(project, home).is_some_and(|r| r["opt_in"] == true)
}

/// /// Nearest ancestor folder that has a record (else `start`), so `telemetry off` run in a subfolder still turns off the installed project's record.
pub fn find_project(start: &Path, home: &Path) -> PathBuf {
    start.ancestors().find(|p| asked(p, home)).unwrap_or(start).to_path_buf()
}

/// /// One consent prompt (only a human-typed `y` or `yes` means yes; empty or anything else = no). The caller (main.rs) passes a human answer only in an interactive terminal.
/// /// On yes, counting starts now (re-enabling does not send counts from the off period). On no, only the "no" is recorded.
pub fn consent(project: &Path, home: &Path, answer: &str, install_path: &str) -> Result<bool, String> {
    let yes = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    let path = if install_path == "npx" { "npx" } else { "manual" };
    let v = if yes { json!({"opt_in": true, "id": random_id(), "install_path": path, "counted_to": store::now()}) } else { json!({"opt_in": false}) };
    write_record(project, home, &v)?;
    Ok(yes)
}

/// /// Prompt for `olp-hook setup` (manual install). It is also the way to re-enable after turning off, so it asks again even if already answered.
pub fn setup(project: &Path, home: &Path, answer: &str) -> Result<String, String> {
    Ok(if consent(project, home, answer, "manual")? {
        "Usage numbers on: once a day `olp-hook signal` may send the fields listed in the README (counts only, no commands or file names). OLP_HOOK_SIGNAL=off or `olp-hook telemetry off` turns it off.".to_string()
    } else {
        "Usage numbers off. Nothing will be sent.".to_string()
    })
}

/// /// Turn off: consent false (also clears the id and last-sent day). The SessionStart entry in settings.json is removed by the executable via the install module.
pub fn opt_out(project: &Path, home: &Path) -> Result<(), String> {
    write_record(project, home, &json!({"opt_in": false}))
}

/// /// Numeric fields: counts only decision records with ts in [from, to). after_decision = whether the file count of the next delete seen by the hook within 10 minutes after a decision card is
/// /// greater or equal / smaller / absent (not the answer a person chose in the prompt). `to` is always at most now minus 10 minutes, so only cards whose window closed are counted.
fn counts(log: &[Value], from: u64, to: u64) -> Value {
    let decisions: Vec<&Value> = log.iter().filter(|l| l["event"] == "decision").collect();
    let in_win = |l: &Value| l["ts"].as_u64().is_some_and(|t| t >= from && t < to);
    let (mut cards, mut blocked, mut asked) = (0u64, 0u64, 0u64);
    let mut after = [0u64; 3];
    for (i, d) in decisions.iter().enumerate().filter(|(_, d)| in_win(d)) {
        cards += u64::from(d["card"] == "card_shown");
        blocked += u64::from(["blocked", "error_blocked", "kept_by_choice"].iter().any(|v| d["verdict"] == *v));
        if d["verdict"] != "decision_asked" {
            continue;
        }
        asked += 1;
        let (n, _) = stats::counted_files(d);
        let t = d["ts"].as_u64().unwrap_or(0);
        let next = decisions[i + 1..]
            .iter()
            .take_while(|m| m["ts"].as_u64().is_some_and(|mt| mt <= t + AFTER_S))
            .map(|m| stats::counted_files(m).0)
            .find(|m| *m > 0);
        after[match next {
            Some(m) if m >= n => 0,
            Some(_) => 1,
            None => 2,
        }] += 1;
    }
    let c = |v: u64| v.min(CAP);
    json!({"cards_shown": c(cards), "blocked": c(blocked), "decision_asked": c(asked), "after_decision": {"all": c(after[0]), "fewer": c(after[1]), "none": c(after[2])}})
}

pub fn plan(project: &Path, home: &Path) -> Result<Plan, String> {
    plan_with(project, home, env_off(std::env::var("OLP_HOOK_SIGNAL").ok().as_deref()))
}

/// /// `env_off` = environment-variable decision (injected in tests).
pub fn plan_with(project: &Path, home: &Path, env_off: bool) -> Result<Plan, String> {
    if env_off {
        return Ok(Plan::Off("OLP_HOOK_SIGNAL=off"));
    }
    let Some(rec) = read_record(project, home) else { return Ok(Plan::Off("not opted in")) };
    if rec["opt_in"] != true {
        return Ok(Plan::Off("not opted in"));
    }
    let Some(id) = rec["id"].as_str().filter(|i| id_ok(i)) else { return Ok(Plan::Off("no id")) };
    let today = store::today();
    if rec["last_sent_day"].as_u64() == Some(today) {
        return Ok(Plan::Off("already sent today"));
    }
    let cfg = config::load(project).map_err(|e| e.to_string())?;
    //     // Project settings may only turn it off (empty string), so a foreign repository cannot change the endpoint. Any other endpoint comes from the home record (outside the repository).
    if cfg.signal_endpoint.as_deref().is_some_and(|e| e.trim().is_empty()) {
        return Ok(Plan::Off("no signal_endpoint set"));
    }
    let Some(endpoint) = rec["endpoint"].as_str().map(str::to_string).or_else(config::bundled_signal_endpoint).filter(|e| !e.trim().is_empty()) else {
        return Ok(Plan::Off("no signal_endpoint set"));
    };
    //     // Only when it starts with an allowed protocol (data/signal-proto.txt); otherwise no args are built. curl is restricted again with --proto.
    if !endpoint.starts_with(&spawn::scheme_prefix()) {
        return Ok(Plan::Off("signal_endpoint not accepted"));
    }
    let Some(timeout) = config::bundled_signal_timeout_s().filter(|t| *t > 0) else { return Ok(Plan::Off("no signal_timeout_s set")) };
    let now = store::now();
    let from = rec["counted_to"].as_u64().unwrap_or(0);
    let counted_to = now.saturating_sub(AFTER_S).max(from);
    let mut body = counts(&store::read_log(project), from, counted_to);
    body["v"] = json!(2);
    body["install_id"] = json!(id);
    body["hook_version"] = json!(env!("CARGO_PKG_VERSION"));
    body["install_path"] = json!(if rec["install_path"] == "npx" { "npx" } else { "manual" });
    //     // "Did the hook run since the last send (including today)?" At session start it may not have run yet today, so "today only" was almost always false.
    body["used_today"] = json!(store::last_used_day(project).is_some_and(|d| d >= from / 86_400));
    body["age_days"] = json!(age_days(store::first_run(project), now));
    body["decision_on"] = json!(cfg.decision);
    let argv = spawn::argv(&endpoint, timeout);
    let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    if !spawn::check_argv(&refs) {
        return Ok(Plan::Off("signal_endpoint not accepted"));
    }
    Ok(Plan::Ready { argv, body: body.to_string(), counted_to })
}

pub fn send(project: &Path, home: &Path) -> Result<String, String> {
    match plan(project, home)? {
        Plan::Off(why) => Ok(format!("not sent: {why}")),
        Plan::Ready { argv, body, counted_to } => {
            //             // Record the day and counted end before the attempt: even on failure it is not retried the same day (no retries, so session start never spends time).
            let mut rec: Value = read_record(project, home).unwrap_or(json!({}));
            rec["last_sent_day"] = json!(store::today());
            rec["counted_to"] = json!(counted_to);
            write_record(project, home, &rec)?;
            Ok(match spawn::run(&argv, &body) {
                Ok(()) => "sent: the usage numbers listed in the README".to_string(),
                Err(why) => format!("not sent: {why}"),
            })
        }
    }
}
