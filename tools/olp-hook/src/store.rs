//! Two device files (on Unix the folders are 0700 and files 0600):
//! - `hook-map.json` mapping (in the user's folder, see `trust_dir` — never read from the project): key (`tool:<tool name>` or `bash:<first word subcommand>`) → effect, reversibility, recovery declaration, declaration time, DB row count given by the user. No argument values, amounts or paths.
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

/// rc4 rev 17-3: the files that decide what passes without asking (remembered answers, quiet mode) live in the user's own folder,
/// `<home>/.olp/projects/<key>/` (under `.olp`, so every literal mention of it is OLP's own state), never inside the project. A cloned repository can carry any `.olp/` files, so files there are
/// never a reason to let a call through. The key is the first 32 hex of sha256 of the project path (made absolute, `/` slashes,
/// the Windows `//?/` prefix removed, lower case on Windows): another clone at another path gets another key.
/// No home folder = no trust folder (nothing is remembered; everything asks).
pub fn trust_dir(project: &Path) -> PathBuf {
    match home() {
        Some(h) => h.join(".olp").join("projects").join(project_key(project)),
        None => PathBuf::from("/nonexistent-olp-home").join(project_key(project)),
    }
}

/// The user's home folder (`USERPROFILE` on Windows, `HOME` elsewhere), None when unset or empty. Without it nothing is remembered:
/// `load_map` gives an empty map and `config::load` the bundled defaults, so no path outside the home can hold trusted answers.
pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Stable per-path key (see trust_dir).
pub fn project_key(project: &Path) -> String {
    let p = std::fs::canonicalize(project).unwrap_or_else(|_| std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf()));
    let mut s = p.to_string_lossy().replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    if cfg!(windows) {
        s = s.to_lowercase();
    }
    sha256_hex(s.as_bytes())[..32].to_string()
}

/// sha256 (FIPS 180-4), same code as the tests' known-vector helper; no new crate.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut m = data.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_be_bytes());
    for c in m.chunks(64) {
        let mut w = [0u32; 64];
        for (i, wi) in w.iter_mut().enumerate().take(16) {
            *wi = u32::from_be_bytes([c[4 * i], c[4 * i + 1], c[4 * i + 2], c[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            v = [t1.wrapping_add(s0.wrapping_add(maj)), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (hi, vi) in h.iter_mut().zip(v) {
            *hi = hi.wrapping_add(vi);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

fn map_path(project: &Path) -> PathBuf {
    trust_dir(project).join("hook-map.json")
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
    if home().is_none() {
        return Ok(DeviceMap { version: 1, entries: BTreeMap::new() });
    }
    match std::fs::read(map_path(project)) {
        Ok(b) => serde_json::from_slice(&b).map_err(|_| "hook-map.json could not be read"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DeviceMap { version: 1, entries: BTreeMap::new() }),
        Err(_) => Err("hook-map.json could not be read"),
    }
}

fn ensure_dir(project: &Path) -> std::io::Result<()> {
    ensure_private_dir(&dir(project))
}

/// Create a folder (and its parents) if missing; on Unix the folder itself is 0700.
pub fn ensure_private_dir(d: &Path) -> std::io::Result<()> {
    let d = d.to_path_buf();
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
    ensure_private_dir(&trust_dir(project))?;
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

#[cfg(test)]
mod key_hash_tests {
    // Value: protects=the trust-folder key (rev 17-3) uses this branch's own sha256, checked against FIPS 180-4 known values;
    // fails_when=the function gives another digest; seam=sha256_hex
    #[test]
    fn sha256_known_values() {
        assert_eq!(super::sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(super::sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            super::sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(super::project_key(std::path::Path::new("/nonexistent/olp/key/test")).len(), 32);
    }
}

