//! 0.1.3 rc4 rev 17-3: rules in the project's own `.claude/settings*.json` never turn an OLP ask into a pass — not even an "always
//! allow" rule that is byte for byte the command (rev 17-2 (6) tried that and it was removed: a cloned repository can carry these
//! files). Writing the settings files is an overwrite that asks. All file operations happen only in throwaway folders under CARGO_TARGET_TMPDIR.
use olp_hook::run_hook;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn project(allow: &[&str]) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_exact_rule").join(format!("{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::create_dir_all(p.join(".claude")).unwrap();
    fs::create_dir_all(p.join("build")).unwrap();
    fs::write(p.join("build").join("a.txt"), "x").unwrap();
    // rev 17-3: answers live in the user's trust folder, not in the project
    let t = olp_hook::store::trust_dir(&p);
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    fs::write(t.join("hook-map.json"), json!({"version": 1, "entries": {}}).to_string()).unwrap();
    fs::write(p.join(".claude").join("settings.local.json"), json!({"permissions": {"allow": allow}}).to_string()).unwrap();
    p
}

fn code(tool: &str, input: serde_json::Value, allow: &[&str]) -> i32 {
    let p = project(allow);
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                   "tool_input": input, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_e"});
    run_hook(v.to_string().as_bytes(), Some(&p)).code
}

fn bash(cmd: &str, allow: &[&str]) -> i32 {
    code("Bash", json!({ "command": cmd }), allow)
}

const CMD: &str = "echo hi; ls build";

// Value: protects=a rule planted in the project settings never lets an OLP ask through (rev 17-3); fails_when=an exact, a pattern
// or a prefix "always allow" rule in .claude/settings.local.json lets a not-simple read or a delete pass; seam=run_hook
#[test]
fn project_settings_rules_never_let_a_call_through() {
    let mut bad = Vec::new();
    for (cmd, rule) in [
        (CMD, format!("Bash({CMD})")),
        (CMD, "Bash(echo hi:*)".to_string()),
        (CMD, "Bash(*)".to_string()),
        (CMD, "Bash".to_string()),
        ("rm -rf build", "Bash(rm -rf build)".to_string()),
        ("echo hi; rm -rf build", "Bash(echo hi; rm -rf build)".to_string()),
    ] {
        if bash(cmd, &[&rule]) == 0 {
            bad.push(format!("{cmd:?} with {rule:?}"));
        }
    }
    let ps_cmd = "echo hi; Get-ChildItem build";
    if code("PowerShell", json!({ "command": ps_cmd }), &[&format!("PowerShell({ps_cmd})")]) == 0 {
        bad.push(format!("{ps_cmd:?} with its PowerShell rule"));
    }
    assert!(bad.is_empty(), "a project settings rule let a call through: {bad:?}");
}

// Value: protects=the AI cannot give itself the memory: writing or editing the project settings files is an overwrite that asks;
// fails_when=one way of changing .claude/settings*.json passes without asking; seam=run_hook
#[test]
fn changing_the_settings_files_asks() {
    let mut bad = Vec::new();
    for f in [".claude/settings.local.json", ".claude/settings.json"] {
        if code("Write", json!({"file_path": f, "content": "{}"}), &[]) == 0 {
            bad.push(format!("Write {f}"));
        }
        if code("Edit", json!({"file_path": f, "old_string": "{", "new_string": "{ "}), &[]) == 0 {
            bad.push(format!("Edit {f}"));
        }
        for cmd in [format!("printf x > {f}"), format!("cp build/a.txt {f}"), format!("echo x >> {f}")] {
            if bash(&cmd, &[]) == 0 {
                bad.push(cmd);
            }
        }
    }
    assert!(bad.is_empty(), "changing a settings file passed without asking: {bad:?}");
}
