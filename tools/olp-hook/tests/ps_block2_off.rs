//! When the parser cannot be used (disabled, missing, timed out, constrained language mode): the built-in list verdict plus an info line. Uses an environment variable, so this test has its own executable.
// Value: protects=when the parser path is blocked the verdict falls back to block/ask (never pass) and there is no empty card; fails_when=a block read passes while disabled, or the info line is missing;
// seam=OLP_HOOK_PS_PARSER=off (off only; there is no switch to turn it on); no_claim=measurement under real policy environments is manual.
use olp_hook::run_hook;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

#[test]
fn p3_parser_off_falls_back_to_the_list_with_an_info_line() {
    std::env::set_var("OLP_HOOK_PS_PARSER", "off");
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_ps_block2_off").join(format!("off_{}", std::process::id()));
    fs::create_dir_all(p.join("build")).unwrap();
    let cmd = r#"Get-ChildItem build | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName"#;
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "PowerShell", "tool_input": {"command": cmd}, "permission_mode": "default"});
    let o = run_hook(v.to_string().as_bytes(), Some(&p));
    assert!(o.stdout.contains("\"ask\""), "disabled must ask as before: {}", o.stdout);
    let m = serde_json::from_str::<serde_json::Value>(o.stdout.trim()).unwrap()["systemMessage"].as_str().unwrap().to_string();
    assert!(m.lines().any(|l| l.starts_with("Not recognized: ") && l.contains("Where-Object")), "info line: {m}");
}

// Value: protects=design addendum 4-1 — on a slow Windows PC the PowerShell parser may give no answer in time; the hook then falls back to
// the closed side: the two commands from the Windows-only failures get a card (ask), never a silent pass, and a delete it cannot count is
// recorded as not counted, never with a number; fails_when=either command passes, or the delete line carries a count;
// seam=OLP_HOOK_PS_PARSER=off (the same fallback as a parser that does not answer)
#[test]
fn ps_parser_no_answer_falls_closed() {
    std::env::set_var("OLP_HOOK_PS_PARSER", "off");
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_ps_block2_off").join(format!("noanswer_{}", std::process::id()));
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("a"), "a").unwrap();
    for cmd in ["Get-ChildItem | Where-Object x:", "[IO.File]::Delete('a'); *"] {
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "PowerShell", "tool_input": {"command": cmd}, "permission_mode": "default"});
        let o = run_hook(v.to_string().as_bytes(), Some(&p));
        let closed = o.stdout.contains("\"ask\"") || o.stdout.contains("\"deny\"") || o.code == 2;
        assert!(closed, "{cmd}: parser without an answer must not pass: code {} stdout {}", o.code, o.stdout);
    }
    let log = fs::read_to_string(p.join(".olp").join("hook-log.jsonl")).unwrap_or_default();
    let last = log.lines().filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok()).last().expect("a decision line");
    let counted = last["findings"].as_array().into_iter().flatten().any(|f| f["count"]["n"].is_u64());
    assert!(!counted, "a delete the hook could not read is never shown with a number: {last}");
}
