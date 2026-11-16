//! PowerShell control keywords and braces are not command names, and git check-ignore is a read.
// Value: protects=a read-only `if (...) { ... }` block does not raise one more empty-card permission prompt, and no risky hint tells the user to classify "if" as read_only; deletes inside a block still prompt; fails_when=a control keyword shows up as a classify candidate, Remove-Item/rm inside a block passes, a read block prompts, or check-ignore prompts; seam=classify::assess_shell (stripping the PowerShell fragment head) and GIT_READ_SUBS; no_claim=this is not a full interpretation of block syntax.
use olp_hook::{run_hook, HookOutcome};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_ps_block").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(p.join("build")).unwrap();
    fs::write(p.join("build/a.txt"), "x").unwrap();
    p
}

fn call(p: &Path, tool: &str, cmd: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd}, "permission_mode": "default"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

/// Card text (systemMessage unpacked from the JSON) plus stderr.
fn text(o: &HookOutcome) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(o.stdout.trim()).ok().and_then(|v| v["systemMessage"].as_str().map(str::to_string)).unwrap_or_default();
    format!("{msg}\n{}", o.stderr)
}

const CONTROL: [&str; 12] = ["if", "elseif", "else", "foreach", "for", "while", "do", "until", "switch", "try", "catch", "finally"];

// Control keywords and braces are not classify candidates (if an unknown command is inside a block, only that command is a candidate)
#[test]
fn c1_control_words_and_braces_are_never_classify_candidates() {
    let p = tmp("c1");
    let o = call(&p, "PowerShell", "if (Test-Path build) { mytool run } else { othertool go }; foreach ($f in $list) { mytool run }; try { mytool run } catch { othertool go } finally { mytool run }");
    let t = text(&o);
    // Ledger 1321 ①: a block is a bundle, so no remembered answer is offered at all (control words never were candidates either)
    assert!(!t.contains("classify --bash"), "bundled command offers no remembered answer: {t}");
    for w in CONTROL {
        assert!(!t.contains(&format!("\"{w}\"")) && !t.contains(&format!("\"{w} ")), "control keyword {w} is a candidate: {t}");
    }
    assert!(!t.contains("\"{\"") && !t.contains("\"}\""), "brace is a candidate: {t}");
}

// Deletes inside a block still get the delete prompt, as before
#[test]
fn c2_deletes_inside_blocks_still_get_the_delete_card() {
    for cmd in [
        "if (Test-Path build) { Remove-Item build -Recurse -Force }",
        "if ($x) { Get-ChildItem build } else { Remove-Item build\\a.txt }",
        "foreach ($f in $list) { rm $f }",
        "try { Get-ChildItem build } finally { Remove-Item build\\a.txt }",
    ] {
        let p = tmp("c2");
        let o = call(&p, "PowerShell", cmd);
        assert_eq!(o.code, 0, "{cmd}: {}", o.stderr);
        assert!(o.stdout.contains("\"ask\"") && o.stdout.contains("delete files"), "{cmd}: must be a delete prompt: {}", o.stdout);
    }
}

// A block with reads only (test c3_read_only_blocks_show_no_card):
// moved to simple_form_only::reads_that_now_ask (rc4 rev 15: not a simple form → asks)

// git check-ignore is a read (no prompt, no candidate), in both Bash and PowerShell
#[test]
fn c4_git_check_ignore_is_read() {
    for tool in ["Bash", "PowerShell"] {
        let p = tmp("c4");
        let o = call(&p, tool, "git check-ignore build");
        assert!(o.stdout.trim().is_empty() && !text(&o).contains("check-ignore\""), "{tool}: {}", text(&o));
    }
}
