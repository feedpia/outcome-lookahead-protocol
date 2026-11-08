//! Card golden card_v1 (`sdk/vectors/card_v1.json`): same input gives the same card text. For now only the hook (npx runs the same executable); MCP, CLI and SDK will read the same file after release.
// Value: protects=card text matches the characters in a single file exactly: at most 3 names plus "and N more", a preview-value marker, payments show one line saying to check the amount in the tool input (the amount is never read), GUI guidance carries no key or address, no risk (read only) means no card text, and when the GUI is on, one line saying to answer no if you picked what to keep; fails_when=card text differs from the golden by even one character, an amount or key appears on a card, or text is emitted for a read-only command; seam=run_hook (real hook path) plus a temporary project; no_claim=how Claude Code renders this text on screen is left to surface measurement.
mod common;

use olp_hook::{classify_cli, run_hook, HookOutcome};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_card_golden").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

/// `{project}` inside an input command is the absolute path of this case's temporary project (so the `cd <project>;` form of a typical command is kept verbatim).
fn with_project(input: &Value, p: &Path) -> Value {
    let mut v = input.clone();
    if let Some(c) = v["command"].as_str() {
        v["command"] = Value::String(c.replace("{project}", &p.to_string_lossy()));
    }
    v
}

fn golden() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/vectors/card_v1.json");
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).expect("card_v1.json")
}

fn setup(p: &Path, case: &Value) {
    for f in case["files"].as_array().into_iter().flatten() {
        let f = p.join(f.as_str().unwrap());
        fs::create_dir_all(f.parent().unwrap()).unwrap();
        fs::write(f, "x").unwrap();
    }
    fs::create_dir_all(p.join(".olp")).unwrap();
    let pick = case["pick"].as_bool().unwrap_or(false);
    fs::write(p.join(".olp/hook-config.json"), json!({"counter_timeout_ms": 3000, "hook_budget_ms": 8000, "pick": pick}).to_string()).unwrap();
    if case["gui_alive"].as_bool().unwrap_or(false) {
        fs::write(p.join(".olp/gui.alive"), json!({"v": 1, "port": 1}).to_string()).unwrap();
    }
    for args in case["classify"].as_array().into_iter().flatten() {
        let args: Vec<String> = args.as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect();
        classify_cli(p, &args).unwrap();
    }
}

#[test]
fn hook_cards_match_card_v1_golden() {
    let g = golden();
    let cases = g["cases"].as_array().expect("cases");
    assert!(cases.len() >= 7, "case count");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let run = || {
            let p = tmp(name);
            setup(&p, case);
            let input = json!({
                "session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(),
                "hook_event_name": "PreToolUse", "tool_name": case["tool"], "tool_input": with_project(&case["input"], &p),
                "permission_mode": case["mode"].as_str().unwrap_or("default"), "tool_use_id": "toolu_golden1",
            });
            run_hook(input.to_string().as_bytes(), Some(&p))
        };
        // The PowerShell syntax parser exists only on Windows: if `expect_windows` is present, parser available means that value; unavailable or slow start means `expect` (shared fallback)
        if case["expect_windows"].is_null() {
            let o = run();
            if let Err(why) = card_matches(&o, &case["expect"]) {
                panic!("{name}: {why}");
            }
        } else {
            common::require_ps_parser_or_env_fallback(name, run, |o: &HookOutcome| card_matches(o, &case["expect_windows"]).is_ok(), |o| card_matches(o, &case["expect"]).is_ok());
        }
    }
}

/// Does the hook output match the golden expectation (`expect` / `expect_windows` cell) character for character? If not, the first difference.
fn card_matches(o: &HookOutcome, e: &Value) -> Result<(), String> {
    if o.code != 0 {
        return Err(format!("exit {}: {}", o.code, o.stderr));
    }
    if e["decision"].is_null() {
        return if o.stdout.trim().is_empty() { Ok(()) } else { Err(format!("no risk means no card text, got {}", o.stdout)) };
    }
    let v: Value = serde_json::from_str(o.stdout.trim()).map_err(|_| format!("not JSON {}", o.stdout))?;
    let want: Vec<&str> = e["message"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
    let checks = [
        (v["hookSpecificOutput"]["permissionDecision"] == e["decision"], "decision"),
        (v["hookSpecificOutput"]["permissionDecisionReason"] == e["reason"], "reason"),
        (v["systemMessage"].as_str().unwrap_or_default() == want.join("\n"), "message"),
    ];
    if let Some((_, what)) = checks.iter().find(|(ok, _)| !ok) {
        return Err(format!("{what} differs from the golden: {}", o.stdout));
    }
    match ["99000", "#t=", "127.0.0.1"].into_iter().find(|bad| o.stdout.contains(bad)) {
        Some(bad) => Err(format!("card contains {bad}")),
        None => Ok(()),
    }
}

#[test]
fn golden_header_and_marker_match_every_case() {
    let g = golden();
    for case in g["cases"].as_array().unwrap() {
        // `expect_windows` gets the same head and marker checks
        for e in [&case["expect"], &case["expect_windows"]] {
            if e.is_null() || e["decision"].is_null() {
                continue;
            }
            assert_eq!(e["message"][0], g["header"], "{}", case["name"]);
            assert!(e["reason"].as_str().unwrap().ends_with(g["marker"].as_str().unwrap()), "{}", case["name"]);
        }
    }
}
