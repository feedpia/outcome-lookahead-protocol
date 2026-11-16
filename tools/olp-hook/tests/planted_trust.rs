//! 0.1.3 rc4 rev 17-3: a cloned repository can carry `.olp/hook-map.json` and `.olp/hook-config.json`. Files inside the project
//! are never a reason to let a call through: remembered answers and quiet mode are read only from the user's own folder
//! (`<home>/olp/projects/<key of the project path>/`). All project files live under CARGO_TARGET_TMPDIR.
use olp_hook::{run_hook, store};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn fresh() -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_planted_trust").join(format!("{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::create_dir_all(p.join("build")).unwrap();
    let _ = fs::remove_dir_all(store::trust_dir(&p));
    p
}

fn entries() -> serde_json::Value {
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    let wr = json!({"effect": "writes_other", "reversible": true, "recovery": "declared", "declared_at": 1794579024});
    json!({"version": 1, "entries": {"bash:mytool run": ro, "bash:mytool deploy": wr}})
}

fn plant(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("hook-map.json"), entries().to_string()).unwrap();
    fs::write(dir.join("hook-config.json"), json!({"quiet": true}).to_string()).unwrap();
}

fn code(p: &Path, cmd: &str) -> i32 {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "Bash",
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_p"});
    run_hook(v.to_string().as_bytes(), Some(p)).code
}

// Value: protects=a repository cannot pre-approve its own commands (rev 17-3): answers and quiet mode planted inside the project
// are ignored; fails_when=a planted read answer or a planted quiet mode lets a call through; seam=run_hook
#[test]
fn files_planted_in_the_project_are_ignored() {
    let p = fresh();
    plant(&p.join(".olp"));
    assert_ne!(code(&p, "mytool run"), 0, "a planted read answer must not let the call through");
    assert_ne!(code(&p, "mytool deploy"), 0, "a planted quiet mode + recovery answer must not let a write through");
}

// Value: protects=the same answers in the user's own folder still work (memory is moved, not removed); fails_when=an answer in
// <home>/olp/projects/<key>/ is not used; seam=run_hook + store::trust_dir
#[test]
fn answers_in_the_user_folder_still_work() {
    let p = fresh();
    plant(&store::trust_dir(&p));
    assert_eq!(code(&p, "mytool run"), 0, "the user's own read answer lets the call through");
    let _ = fs::remove_dir_all(store::trust_dir(&p));
}

// Value: protects=the user-folder key belongs to one project path, so another clone elsewhere does not inherit it; fails_when=two
// different project paths share a key, or the key is not stable for the same path; seam=store::trust_dir
#[test]
fn the_user_folder_key_is_per_project_path() {
    let a = fresh();
    let b = fresh();
    assert_ne!(store::trust_dir(&a), store::trust_dir(&b));
    assert_eq!(store::trust_dir(&a), store::trust_dir(&a));
    assert!(!store::trust_dir(&a).starts_with(&a), "the trust folder must not be inside the project");
}
