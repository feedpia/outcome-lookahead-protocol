//! Project config `<project>/.olp/hook-config.json`. There is no global config or environment-variable switch (quiet mode is a per-project opt-in).
//! The two time limits come only from here (no constants in code). If either is missing or `counter_timeout_ms >= hook_budget_ms`, counting is skipped (= "unchecked").

use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub quiet: bool,
    pub counter_timeout_ms: Option<u64>,
    pub hook_budget_ms: Option<u64>,
    /// Opt-in usage-count endpoint. Default = value from the bundled data file (no URL text in source). In project config only an empty string (= off) takes effect. Nothing is sent without consent (home record).
    #[serde(default)]
    pub signal_endpoint: Option<String>,
    /// Time limit (integer seconds) for one opt-in signal, read by the signal module. Default from the bundled data file.
    #[serde(default)]
    pub signal_timeout_s: Option<u64>,
    /// Time limit (integer seconds) when a person runs `olp-hook signal` (no SessionStart slot to stay under). Bundled data file only.
    #[serde(default)]
    pub signal_manual_timeout_s: Option<u64>,
    /// Picked-files-only mode (default false). Only when enabled and `olp-gui` is alive does a delete card write pending and compare with the selection (verdict unchanged).
    #[serde(default)]
    pub pick: bool,
    /// Decision-card experiment (default false). When enabled, a delete with new info or choices is blocked once and the choices are given to the agent (decision.rs).
    #[serde(default)]
    pub decision: bool,
}

impl Config {
    /// Counting limit. None = no counting.
    pub fn counter_limit(&self) -> Option<Duration> {
        match (self.counter_timeout_ms, self.hook_budget_ms) {
            (Some(c), Some(b)) if c < b => Some(Duration::from_millis(c)),
            _ => None,
        }
    }
    /// Overall hook limit (exceeding it blocks). README: set the Claude Code hook `timeout` longer than this.
    pub fn hook_budget(&self) -> Option<Duration> {
        self.hook_budget_ms.map(Duration::from_millis)
    }
}

pub fn path(project: &Path) -> std::path::PathBuf {
    project.join(".olp").join("hook-config.json")
}

/// Bundled default config: used when the project config is missing or leaves a limit empty. Values live only in data files (no constants in code).
const DEFAULT_JSON: &str = include_str!("../data/hook-config.default.json");

fn bundled() -> Config {
    serde_json::from_str(DEFAULT_JSON).expect("동봉 hook-config.default.json 꼴")
}

/// Bundled usage-count endpoint (project config can only change it to empty = off; the signal module decides).
pub fn bundled_signal_endpoint() -> Option<String> {
    bundled().signal_endpoint
}

/// Bundled time limit (seconds) for the usage signal. Project config values are ignored (so the sender's limit never reaches the SessionStart 3 s slot).
pub fn bundled_signal_timeout_s() -> Option<u64> {
    bundled().signal_timeout_s
}

/// Bundled time limit (seconds) when a person runs `olp-hook signal`: long enough for a server that is waking up.
pub fn bundled_signal_manual_timeout_s() -> Option<u64> {
    bundled().signal_manual_timeout_s
}

/// No file = bundled default. If present, its values are used and only empty limit fields are filled from the bundled default. A malformed file is Err (the hook blocks).
pub fn load(project: &Path) -> Result<Config, &'static str> {
    let base = bundled();
    match std::fs::read(path(project)) {
        Ok(b) => {
            let mut cfg: Config = serde_json::from_slice(&b).map_err(|_| "hook-config.json could not be read")?;
            cfg.counter_timeout_ms = cfg.counter_timeout_ms.or(base.counter_timeout_ms);
            cfg.hook_budget_ms = cfg.hook_budget_ms.or(base.hook_budget_ms);
            cfg.signal_timeout_s = cfg.signal_timeout_s.or(base.signal_timeout_s);
            // missing field = bundled URL. The signal module honors only an empty project value (= off) and never another URL (a foreign repo cannot redirect it).
            cfg.signal_endpoint = cfg.signal_endpoint.or(base.signal_endpoint);
            Ok(cfg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(base),
        Err(_) => Err("hook-config.json could not be read"),
    }
}
