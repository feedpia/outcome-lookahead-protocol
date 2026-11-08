//! Interpret PowerShell commands properly: a hybrid of the built-in Rust classifier and the PowerShell parser.
//! PowerShell commands that the Rust classifier cannot decide are read by the PowerShell syntax parser (nothing executed); only parsed reads pass, and unknown input asks as before.
// Value: protects=read-only listings containing Where-Object/ForEach-Object blocks raise no permission prompt and are not blocked in bypass mode, while deletes and method calls inside a block prompt;
// fails_when=any of the five read forms prompts, any of the five write forms or five unknown forms passes without a prompt, or command text is executed by the parser script;
// why_new=the built-in list cannot see inside blocks; seam=classify::assess_shell -> ps_ast (Windows only; on failure fall back to the built-in list);
// no_claim=PowerShell on macOS/Linux (no parser: built-in list plus an info line) and Bash.
mod common;

use olp_hook::{run_hook, HookOutcome};
use common::require_ps_parser_or_env_fallback as gate;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_ps_block2").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(p.join("build")).unwrap();
    fs::write(p.join("build/a.txt"), "x").unwrap();
    p
}

/// The parser path launches powershell.exe, so concurrent tests would exceed the 1.5 s cap and mix in fallbacks; tests in this file run one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn call(p: &Path, cmd: &str) -> HookOutcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    call_locked(p, cmd)
}

/// Called by the holder of ONE_AT_A_TIME (timing excludes the wait and measures only the hook).
fn call_locked(p: &Path, cmd: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "PowerShell", "tool_input": {"command": cmd}, "permission_mode": "default"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn msg(o: &HookOutcome) -> String {
    serde_json::from_str::<serde_json::Value>(o.stdout.trim()).ok().and_then(|v| v["systemMessage"].as_str().map(str::to_string)).unwrap_or_default()
}

fn asked(o: &HookOutcome) -> bool {
    o.stdout.contains("\"ask\"")
}

// The five read forms: only interpreted reads pass without a prompt
const READS: [&str; 5] = [
    r#"Get-ChildItem build | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName"#,
    r#"Get-ChildItem build | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse | Measure-Object Length -Sum); "{0} files={1}" -f $_.FullName, $s.Count }"#,
    r#"Get-ChildItem build | ForEach-Object { $_.Name.ToLower() }"#,
    r#"Get-ChildItem build | Where-Object Name -like '*.log'"#,
    r#"if (Test-Path build) { "--- build ---"; git ls-files build | ForEach-Object { $_ } }"#,
];

#[test]
fn b3_interpreted_read_blocks_show_no_card() {
    for cmd in READS {
        // Parser available: no prompt. Unavailable or slow start: ask as before (shared fallback)
        gate(cmd, || call(&tmp("read"), cmd), |o: &HookOutcome| o.code == 0 && o.stdout.trim().is_empty(), |o| o.code == 0 && asked(o));
    }
}

// The five write/delete forms (plus one static call) prompt; deletes get the delete prompt
#[test]
fn b4_writes_and_deletes_inside_blocks_show_a_card() {
    for (cmd, delete) in [
        (r#"Get-ChildItem build | ForEach-Object { Remove-Item $_.FullName }"#, true),
        (r#"Get-ChildItem build | ForEach-Object { $_.Delete() }"#, true),
        (r#"[System.IO.File]::Delete('build\a.txt')"#, true),
        (r#"[IO.Directory]::Delete('build', $true)"#, true),
        (r#"Get-ChildItem build | ForEach-Object { Set-Content $_.FullName 'x' }"#, false),
        (r#"foreach ($f in $list) { rm $f }"#, true),
        (r#"Get-ChildItem build | ForEach-Object { $_.IsReadOnly = $true }"#, false),
        (r#"Get-ChildItem build | ForEach-Object Delete"#, false),
        // Self-check of the engine: drive-qualified variable assignment (alias swap, file write), property ++, block passed via variable
        (r#"$alias:ls = 'Clear-Content'; ls build\a.txt"#, false),
        (r#"${c:build\a.txt} = 'x'"#, false),
        (r#"Get-ChildItem build | ForEach-Object { $_.Attributes++ }"#, false),
        (r#"Get-ChildItem build | Where-Object $sb"#, false),
        (r#"Get-ChildItem build | Where-Object -FilterScript $sb"#, false),
    ] {
        let o = call(&tmp("write"), cmd);
        assert!(o.stdout.contains("\"ask\""), "{cmd}: must prompt: {}{}", o.stdout, o.stderr);
        if delete {
            // Parser available: delete prompt. Unavailable or slow start: ask (prompt kind is left to the built-in list)
            gate(cmd, || call(&tmp("write"), cmd), |o: &HookOutcome| asked(o) && msg(o).contains("delete files"), asked);
        }
    }
}

// The five unknown forms: ask as before, plus an info line (no empty prompt)
#[test]
fn b2_unknown_stays_ask_with_an_info_line() {
    for (cmd, token) in [
        (r#"Get-ChildItem build | ForEach-Object { & $cmd }"#, "$cmd"),
        (r#"Invoke-Expression $x"#, "Invoke-Expression"),
        (r#"Get-ChildItem build | ForEach-Object { $_.Refresh() }"#, "Refresh"),
        (r#". $script"#, "$script"),
        (r#"[Activator]::CreateInstance($t)"#, "CreateInstance"),
    ] {
        let o = call(&tmp("unknown"), cmd);
        assert!(o.stdout.contains("\"ask\""), "{cmd}: unknown must ask: {}", o.stdout);
        // Parser available: info line names the item. Unavailable or slow start: ask only
        gate(cmd, || call(&tmp("unknown"), cmd), |o: &HookOutcome| asked(o) && msg(o).lines().any(|l| l.starts_with("Not recognized: ") && l.contains(token)), asked);
    }
}

// Korean text and spaces in command text (UTF-8 pinned)
#[test]
fn p2_korean_and_spaces_in_command_text() {
    let p = tmp("p2");
    fs::write(p.join("build").join("한글 파일.txt"), "x").unwrap();
    let cmd = r#"Get-ChildItem build | Where-Object { $_.Name -ne '한글 파일.txt' } | Select-Object Name"#;
    // Parser available: Korean text is still a plain read (no prompt). Unavailable or slow start: ask
    gate("p2 korean text", || call(&p, cmd), |o: &HookOutcome| o.code == 0 && o.stdout.trim().is_empty(), |o| o.code == 0 && asked(o));
}

// Command text is data only: text that tries to break out of quotes executes nothing on the parser side
#[test]
fn p1_command_text_is_never_run_by_the_parser() {
    let p = tmp("p1");
    for cmd in [
        r#"Get-ChildItem build | Where-Object { $_.Name -ne 'x'); New-Item -Path pwned1.txt; ('' }"#,
        "Get-ChildItem build | ForEach-Object { $_ }\n'@\nNew-Item -Path pwned2.txt\n@'",
        r#"Get-ChildItem build | ForEach-Object { $_ } #`"; New-Item -Path pwned3.txt"#,
    ] {
        let _ = call(&p, cmd);
    }
    for f in ["pwned1.txt", "pwned2.txt", "pwned3.txt"] {
        assert!(!p.join(f).exists(), "{f} was created: command text was executed");
    }
}

// Four realistic commands (parser path) also finish within 2 s for the whole hook
#[test]
fn p5_parser_path_stays_under_two_seconds() {
    let cmd = r#"Get-ChildItem -Directory -Recurse -Depth 3 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum); "{0} | files={1} | MB={2:N1}" -f $_.FullName, $s.Count, ($s.Sum/1MB) }; git status --short | Select-Object -First 10; git check-ignore build"#;
    let p = tmp("p5");
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let t = std::time::Instant::now();
    let _ = call_locked(&p, cmd);
    assert!(t.elapsed().as_millis() < 2000, "{} ms", t.elapsed().as_millis());
}
