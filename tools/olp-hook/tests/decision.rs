//! Decision-card experiment: the hook denies a delete once and the reason lists 3 choices plus each outcome (strong numbers only), so that
//! the agent asks through the Claude Code question dialog. The chosen answer is only a next instruction; the next delete again goes through the normal card and permission dialog.
// Value: protects=off by default; blocks only with new information (k >= 1 files not in git) and a choice (>= 2 files); one block per 10 minutes; reason = fixed words + numbers (no echo of input text);
//        fails_when=blocks when not enabled, blocks without new information, blocks more than once (stall loop), or file names or command text appear in the reason;
//        why_new=measure whether the agent really opens a question dialog; seam=hook::run_hook; no_claim=whether the agent actually raises the question dialog is left to measurement (RESULT.md).
use olp_hook::{run_hook, HookOutcome};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    // CARGO_TARGET_TMPDIR is inside this repository, so a "not git" case would hit the parent repository; use the OS temp folder outside it.
    let p = std::env::temp_dir().join("olp_decision_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

fn git(p: &Path, args: &[&str]) {
    let ok = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "core.hooksPath=/dev/null"]).args(args).current_dir(p).output().unwrap().status.success();
    assert!(ok, "git {args:?}");
}

/// build/app.log, build/cache.tmp, build/notes.txt; only `tracked` is committed. With `git_repo=false` there is no git.
fn project(name: &str, decision: Option<bool>, git_repo: bool, tracked: &[&str]) -> PathBuf {
    let p = tmp(name);
    fs::create_dir_all(p.join("build")).unwrap();
    for f in ["build/app.log", "build/cache.tmp", "build/notes.txt"] {
        fs::write(p.join(f), "x").unwrap();
    }
    fs::create_dir_all(p.join(".olp")).unwrap();
    let mut cfg = json!({"counter_timeout_ms": 3000, "hook_budget_ms": 8000});
    if let Some(d) = decision {
        cfg["decision"] = json!(d);
    }
    fs::write(p.join(".olp/hook-config.json"), cfg.to_string()).unwrap();
    if git_repo {
        git(&p, &["init", "-q"]);
        if !tracked.is_empty() {
            git(&p, &[&["add", "--"][..], tracked].concat());
            git(&p, &["commit", "-q", "-m", "t"]);
        }
    }
    p
}

fn pre(p: &Path, tool: &str, cmd: &str, mode: &str, id: &str) -> HookOutcome {
    let v = json!({
        "session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(),
        "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd},
        "permission_mode": mode, "tool_use_id": id,
    });
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn out(o: &HookOutcome) -> Value {
    serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null)
}
fn decision(o: &HookOutcome) -> String {
    out(o)["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string()
}
fn reason(o: &HookOutcome) -> String {
    out(o)["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()
}

/// Golden: fixed words + numbers only (n=3, k=2).
/// Choice order: cancel -> keep some -> delete all, so the default first slot never means "delete everything".
const REASON_3_2: &str = "[OLP] decision: this delete would remove 3 files, and 2 of them are not in git (git cannot bring them back). It did not run. Ask the person with AskUserQuestion: one question, these three options in this order (the first one is the default), then do only what they choose. 1) \"Cancel\": nothing removed. 2) \"Keep some\": ask which files to keep, then delete only the others. 3) \"Delete all 3\": 3 files removed. The next delete will show the usual OLP card.";

#[test]
fn off_by_default_keeps_the_usual_card() {
    let p = project("off", None, true, &["build/notes.txt"]);
    let o = pre(&p, "Bash", "rm -rf build", "default", "toolu_d0");
    assert_eq!(decision(&o), "ask");
    assert!(reason(&o).starts_with("[OLP] Before this runs: delete files — 3 files"), "{}", reason(&o));
}

#[test]
fn on_with_new_info_denies_once_with_fixed_options() {
    let p = project("on", Some(true), true, &["build/notes.txt"]);
    let o = pre(&p, "Bash", "rm -rf build", "default", "toolu_d1");
    assert_eq!(decision(&o), "deny");
    assert_eq!(reason(&o), REASON_3_2);
    // The dialog line starts with "OLP hook:", so it does not overlap the reason marker.
    assert!(!out(&o)["systemMessage"].as_str().unwrap_or("").contains("[OLP]"), "{}", o.stdout);
    // Limit 1: the next delete within the same 10 minutes gets a normal card (no stall loop).
    let o2 = pre(&p, "Bash", "rm -rf build", "default", "toolu_d2");
    assert_eq!(decision(&o2), "ask");
    assert!(reason(&o2).starts_with("[OLP] Before this runs: delete files — 3 files"), "{}", reason(&o2));
}

#[test]
fn powershell_delete_gets_the_same_options() {
    let p = project("ps", Some(true), true, &["build/notes.txt"]);
    let o = pre(&p, "PowerShell", r"Remove-Item -LiteralPath .\build -Recurse -Force", "default", "toolu_d3");
    assert_eq!(decision(&o), "deny");
    assert_eq!(reason(&o), REASON_3_2);
}

#[test]
fn modes_without_a_prompt_also_get_options_not_a_block_card() {
    for mode in ["bypassPermissions", "auto", "acceptEdits"] {
        let p = project(&format!("mode_{mode}"), Some(true), true, &["build/notes.txt"]);
        let o = pre(&p, "Bash", "rm -rf build", mode, "toolu_d4");
        assert_eq!(decision(&o), "deny", "{mode}");
        assert_eq!(reason(&o), REASON_3_2, "{mode}");
    }
}

#[test]
fn rule_1_no_new_info_or_no_real_choice_keeps_the_card() {
    // everything is in git (k=0) -> no new information
    let all = project("all_tracked", Some(true), true, &["build/app.log", "build/cache.tmp", "build/notes.txt"]);
    assert_eq!(decision(&pre(&all, "Bash", "rm -rf build", "default", "toolu_d5")), "ask");
    // not a git repo -> k unknown -> no new information
    let nogit = project("nogit", Some(true), false, &[]);
    assert_eq!(decision(&pre(&nogit, "Bash", "rm -rf build", "default", "toolu_d6")), "ask");
    // a single file -> no "keep some" choice
    let one = project("one", Some(true), true, &["build/notes.txt"]);
    assert_eq!(decision(&pre(&one, "Bash", "rm build/app.log", "default", "toolu_d7")), "ask");
    // uncountable shape -> no number -> no block
    let pat = project("pattern", Some(true), true, &["build/notes.txt"]);
    assert_eq!(decision(&pre(&pat, "Bash", "rm -rf build/*", "default", "toolu_d8")), "ask");
}

#[test]
fn reason_echoes_no_file_names_or_command_text() {
    let p = project("echo", Some(true), true, &["build/notes.txt"]);
    let r = reason(&pre(&p, "Bash", "rm -rf build", "default", "toolu_d9"));
    for w in ["app.log", "cache.tmp", "notes.txt", "rm -rf", "build"] {
        assert!(!r.contains(w), "{w} in {r}");
    }
}
