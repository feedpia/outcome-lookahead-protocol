//! In PowerShell, a statement consisting only of a quoted string is output and runs nothing, so it counts as a read.
// Value: protects=a line like `"--- build ---"` inside a read command raises no empty card or bypass block, while `$(...)`, `&`, `.`, redirects, pipes and Bash inside strings behave as before;
// fails_when=a lone string prompts, `"$(Remove-Item ...)"`, `& "x.exe"` or `. "x.ps1"` passes, or `"x" > file` / `| Out-File` gets no prompt;
// why_new=once classify-candidate lines were removed, a nameless word produced an empty card; seam=shell::Word.quoted, classify::assess_shell; no_claim=this is not a full interpretation of PowerShell expressions.
use olp_hook::{run_hook, HookOutcome};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_ps_strlit").join(format!("{name}_{}_{n}", std::process::id()));
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

fn no_card(cmd: &str) {
    let o = call(&tmp("read"), "PowerShell", cmd);
    assert_eq!(o.code, 0, "{cmd}: {}", o.stderr);
    assert!(o.stdout.trim().is_empty(), "{cmd}: must produce no prompt: {}", o.stdout);
}

fn card(tool: &str, cmd: &str) -> String {
    let o = call(&tmp("card"), tool, cmd);
    assert!(o.stdout.contains("\"ask\""), "{tool} {cmd}: must prompt: {}{}", o.stdout, o.stderr);
    o.stdout
}

// Realistic English-language command verbatim plus lone strings (double and single quotes)
#[test]
fn t1_lone_string_statements_are_output_only() {
    no_card(r#"Get-ChildItem -Directory -Force | Select-Object Name; if (Test-Path build) { "--- build ---"; Get-ChildItem build -Force | Select-Object -First 20 Name; (Get-ChildItem build -Recurse -File -Force | Measure-Object -Property Length -Sum) | Select-Object Count,Sum; git status --short --ignored build | Select-Object -First 5; git ls-files build | Select-Object -First 5 }"#);
    no_card(r#""--- build ---""#);
    no_card("'--- build ---'");
    no_card(r#"Get-ChildItem build; "---notes.txt---"; Get-Content build\a.txt"#);
}

// $(...) inside double quotes runs the string, so it is not a read
#[test]
fn t7_subexpression_inside_double_quotes_is_not_read() {
    for cmd in [r#""$(Remove-Item build -Recurse)""#, r#""$(mytool run)""#, r#"if ($x) { "$(Remove-Item build\a.txt)" }"#] {
        let o = call(&tmp("t7"), "PowerShell", cmd);
        assert!(o.stdout.contains("\"ask\"") || o.code != 0, "{cmd}: must not pass: {}", o.stdout);
    }
}

// Call operator & and dot-sourcing . followed by a quoted word execute, so they prompt as before
#[test]
fn t2_call_operator_and_dot_source_still_card() {
    card("PowerShell", r#"& "mytool.exe""#);
    card("PowerShell", r#"& 'mytool.exe'"#);
    card("PowerShell", r#". "x.ps1""#);
    card("PowerShell", r#"Get-ChildItem build; & "mytool.exe""#);
}

// Deletes inside a block get the delete prompt
#[test]
fn t3_delete_after_string_in_block_still_delete_card() {
    let out = card("PowerShell", r#"if (Test-Path build) { "a"; Remove-Item build -Recurse }"#);
    assert!(out.contains("delete files"), "{out}");
}

// A redirect or Out-File after a string gets the overwrite prompt
#[test]
fn t4_string_redirect_or_out_file_still_overwrite_card() {
    for cmd in [r#""x" > build\a.txt"#, r#""x" | Out-File build\a.txt"#] {
        let out = card("PowerShell", cmd);
        assert!(out.contains("overwrite") || out.contains("Not classified"), "{cmd}: {out}");
    }
}

// A redirect target of exactly `$null` (case-insensitive) is a discard sink; `$nullx` or any other variable prompts as an overwrite as before
#[test]
fn c9_redirect_to_dollar_null_is_a_null_sink() {
    no_card("git status --short -- build 2>$null");
    no_card("Get-ChildItem build 2>$NULL | Select-Object Name");
    no_card("Get-ChildItem build >$Null");
    for cmd in ["git status 2>$nullx", "git status 2>$x", "Get-ChildItem build > $out"] {
        let out = card("PowerShell", cmd);
        assert!(out.contains("overwrite"), "{cmd}: {out}");
    }
}

// Bash is unchanged (`"mytool" run` executes), and two or more words are unchanged
#[test]
fn t5_t6_bash_and_multi_word_unchanged() {
    card("Bash", r#""mytool" run"#);
    card("Bash", r#""mytool""#);
    card("PowerShell", r#""a" b"#);
}
