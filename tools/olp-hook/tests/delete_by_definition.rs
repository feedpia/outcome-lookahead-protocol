//! 0.1.3 (DESIGN_OLP_013_RULES §4·§6 T2~T4): forms whose definition is a delete show the existing delete card, and a delete whose target is only known at run time says why it was not counted.
// Value: protects=mirror-copy purge, volume format, recycle-bin clear and `% Delete` ask with the delete card instead of the unknown-command card, and stay a card even after the first word is classified read_only;
// a delete inside `sh -c` whose target is a `$` value says the target is only known at run time; fails_when=any of these forms passes, or shows the unknown-command card, or the tail reason stays "this form is not counted";
// seam=classify::detect_dangers + classify::count_for; no_claim=no count for these forms (still "count not checked"), card text frame unchanged (card.rs).
use olp_hook::{classify_cli, run_hook, HookOutcome};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);
/// PowerShell may start the syntax parser (powershell.exe); one at a time keeps the 1.5 s cap from mixing in fallbacks.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_delete_by_definition").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(p.join("build")).unwrap();
    fs::write(p.join("build/a.txt"), "x").unwrap();
    p
}

fn call(p: &Path, tool: &str, cmd: &str, mode: &str) -> HookOutcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd}, "permission_mode": mode});
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn reason(o: &HookOutcome) -> String {
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null);
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask", "ask expected: {}", o.stdout);
    v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or_default().to_string()
}

const DELETE_TAIL: &str = "delete files — count not checked (this form is not counted)";
const RUNTIME_TAIL: &str = "delete files — count not checked (the target is only known at run time)";

const PS_FORMS: [&str; 6] = [
    "robocopy src build /MIR",
    "robocopy src build /PURGE",
    "Format-Volume -DriveLetter Z",
    "Clear-RecycleBin -Force",
    "Get-ChildItem build | % Delete",
    "Get-ChildItem build | ForEach-Object -MemberName Delete",
];

#[test]
fn delete_by_definition_shows_the_delete_card() {
    let p = tmp("card");
    for c in PS_FORMS {
        let r = reason(&call(&p, "PowerShell", c, "default"));
        assert!(r.contains(DELETE_TAIL), "PowerShell {c}: {r}");
        assert_eq!(call(&p, "PowerShell", c, "bypassPermissions").code, 2, "PowerShell {c} bypass");
    }
    let c = "robocopy src build /MIR";
    let r = reason(&call(&p, "Bash", c, "default"));
    assert!(r.contains(DELETE_TAIL), "Bash {c}: {r}");
    assert_eq!(call(&p, "Bash", c, "bypassPermissions").code, 2, "Bash {c} bypass");
    assert!(p.join("build/a.txt").exists());
}

#[test]
fn delete_by_definition_cannot_be_family_answered() {
    let p = tmp("family");
    let _ = classify_cli(&p, &["--bash".into(), "robocopy".into(), "read_only".into()]);
    assert_eq!(call(&p, "Bash", "robocopy src build /MIR", "bypassPermissions").code, 2);
    let r = reason(&call(&p, "Bash", "robocopy src build /MIR", "default"));
    assert!(r.contains(DELETE_TAIL), "{r}");
}

#[test]
fn runtime_target_delete_says_why() {
    let p = tmp("runtime");
    for c in ["sh -c 'rm -f \"$1\"' _ build/a.txt", "printf 'build/a.txt' | xargs bash -c 'rm -f \"$@\"' _"] {
        let r = reason(&call(&p, "Bash", c, "default"));
        assert!(r.contains(RUNTIME_TAIL), "{c}: {r}");
    }
    // a loop keeps the old reason (explore_reads B_BASELINE bytes)
    let r = reason(&call(&p, "Bash", "for d in build; do rm -rf \"$d\"; done", "default"));
    assert!(r.contains(DELETE_TAIL), "{r}");
    assert!(p.join("build/a.txt").exists());
}
