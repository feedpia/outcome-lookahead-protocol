//! Deleting the home folder itself is blocked even when it is not above the working folder, and also when HOME is in Git Bash form (`/c/…`).
//! HOME is modified, so this is a single-test binary (it does not run concurrently with other tests).
// Value: protects=deleting home, where the Claude Code built-in critical-path dialog appears, is not executed without an OLP card; fails_when=blocks non-home paths, or misses depending on the HOME form;
//   why_new=home-folder coverage; seam=run_hook; no_claim=Windows 8.3 short names.
use olp_hook::run_hook;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[test]
fn home_folder_itself_is_denied_in_any_spelling() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("olp_cwd_home_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let proj = root.join("work").join("proj"); // working folder is outside home
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join("build")).unwrap();
    let slash = |p: &PathBuf| p.to_string_lossy().replace('\\', "/");
    let run = |cmd: &str| -> String {
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": proj.to_string_lossy(), "hook_event_name": "PreToolUse",
            "tool_name": "Bash", "tool_input": {"command": cmd}, "permission_mode": "default", "tool_use_id": "toolu_home"});
        let o = run_hook(v.to_string().as_bytes(), Some(&proj));
        let v: Value = serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null);
        v["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string()
    };
    let mut spellings = vec![slash(&home)];
    if cfg!(windows) {
        // HOME in Git Bash form: `/d/…`
        let s = slash(&home);
        spellings.push(format!("/{}{}", s[..1].to_lowercase(), &s[2..]));
    }
    for h in spellings {
        std::env::set_var("HOME", &h);
        assert_eq!(run("rm -rf ~"), "deny", "HOME={h}: home itself is blocked");
        assert_eq!(run("rm -rf ~/"), "deny", "HOME={h}");
        assert_eq!(run(&format!("rm -rf {}", slash(&home))), "deny", "HOME={h}: absolute home path");
        // a subfolder inside home is unchanged (outside the working folder, so ask)
        assert_eq!(run("rm -rf ~/sub"), "ask", "HOME={h}: subfolder inside home is unchanged");
    }
}
