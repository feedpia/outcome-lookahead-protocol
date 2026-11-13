//! 0.1.3 ⓪ (DESIGN_OLP_REPLAY_013 §0-A): `decide()` is the hook's verdict with no writes; `run_hook` is the shell that
//! writes the record and the card. Same input and same disk -> same Route, and the shell's outcome follows the Route.
use olp_hook::{decide, run_hook, HookRequest, Route};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_decide_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

fn with_build(project: &Path, n: usize) {
    fs::create_dir_all(project.join("build")).unwrap();
    for i in 0..n {
        fs::write(project.join("build").join(format!("f{i:03}.txt")), "x").unwrap();
    }
}

fn verdict(project: &Path, cmd: &str, mode: Option<&str>) -> Route {
    let cfg = olp_hook::config::load(project).unwrap();
    let map = Arc::new(olp_hook::store::load_map(project).unwrap());
    let limits = olp_hook::classify::Limits { counter: cfg.counter_limit(), ps_parser: true };
    let req = HookRequest { tool: "Bash".into(), input: json!({"command": cmd}), cwd: project.to_path_buf(), mode: mode.map(String::from) };
    decide(req, project, &cfg, map, limits, cfg.hook_budget()).unwrap().route
}

fn shell(project: &Path, cmd: &str, mode: Option<&str>) -> olp_hook::HookOutcome {
    let mut v: Value = json!({"session_id": "s", "transcript_path": "t", "cwd": project.to_string_lossy(),
        "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": cmd}});
    if let Some(m) = mode {
        v["permission_mode"] = json!(m);
    }
    run_hook(v.to_string().as_bytes(), Some(project))
}

fn olp_files(project: &Path) -> Vec<(String, u64)> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(project.join(".olp")) {
        for e in rd.flatten() {
            out.push((e.file_name().to_string_lossy().into_owned(), e.metadata().map(|m| m.len()).unwrap_or(0)));
        }
    }
    out.sort();
    out
}

#[test]
fn read_command_routes_read_and_the_shell_passes() {
    let p = tmp("read");
    assert!(matches!(verdict(&p, "ls", Some("auto")), Route::Read));
    assert_eq!(shell(&p, "ls", Some("auto")).code, 0);
}

#[test]
fn big_delete_in_a_prompt_mode_routes_ask_and_the_shell_asks() {
    let p = tmp("ask");
    with_build(&p, 12);
    assert!(matches!(verdict(&p, "rm -rf build", Some("auto")), Route::Ask));
    let o = shell(&p, "rm -rf build", Some("auto"));
    assert_eq!(o.code, 0);
    assert!(o.stdout.contains("\"permissionDecision\":\"ask\""), "{}", o.stdout);
}

#[test]
fn big_delete_in_a_no_prompt_mode_routes_block_with_the_mode_name() {
    let p = tmp("block");
    with_build(&p, 12);
    match verdict(&p, "rm -rf build", Some("bypassPermissions")) {
        Route::Block { no_prompt_mode } => assert_eq!(no_prompt_mode, Some("bypassPermissions")),
        _ => panic!("expected Block"),
    }
    assert_eq!(shell(&p, "rm -rf build", Some("bypassPermissions")).code, 2);
    match verdict(&p, "rm -rf build", Some("someNewMode")) {
        Route::Block { no_prompt_mode } => assert_eq!(no_prompt_mode, Some("unknown")),
        _ => panic!("expected Block"),
    }
}

#[test]
fn deleting_the_working_folder_routes_cwd_deny() {
    let p = tmp("cwd");
    with_build(&p, 2);
    assert!(matches!(verdict(&p, "rm -rf .", Some("auto")), Route::CwdDeny));
}

#[test]
fn decide_writes_nothing() {
    let p = tmp("nowrite");
    with_build(&p, 12);
    let before = olp_files(&p);
    let _ = verdict(&p, "rm -rf build", Some("auto"));
    let _ = verdict(&p, "rm -rf build", Some("bypassPermissions"));
    assert_eq!(olp_files(&p), before);
    assert!(p.join("build").join("f000.txt").exists());
}
