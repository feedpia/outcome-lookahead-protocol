//! Closes holes where AST nodes that the parser script reports no facts for used to pass silently.
//! Anything outside the allowlist is treated as unknown and asks instead of passing.
// Value: protects=`using module`, type-cast constructors, `-Process:$sb`, positional input and value-type static write methods never pass without a prompt;
// fails_when=any of those forms gets no prompt, the allowlist is widened without a test change, or a read-only block prompts again; seam=ps_ast::SCRIPT allowlist, classify::judge_ps_facts;
// no_claim=hosts without the parser (macOS/Linux use the built-in list and always ask).
mod common;

use olp_hook::{run_hook, HookOutcome};
use common::require_ps_parser_or_env_fallback as gate;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);
/// Running many powershell.exe processes at once hits the 1.5 s cap and mixes in fallbacks, so tests in this file run one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_ps_block2b").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(p.join("build")).unwrap();
    fs::write(p.join("build/a.txt"), "x").unwrap();
    p
}

fn call(p: &Path, cmd: &str) -> HookOutcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "PowerShell", "tool_input": {"command": cmd}, "permission_mode": "default"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn asks(cmd: &str) {
    let o = call(&tmp("ask"), cmd);
    assert!(o.stdout.contains("\"ask\""), "{cmd}: must ask (no prompt would be a hole): {}{}", o.stdout, o.stderr);
}

/// Parser available: interpreted read-only (no prompt). Unavailable or slow start: ask (shared fallback).
fn passes_with_parser(cmd: &str) {
    gate(cmd, || call(&tmp("read"), cmd), |o: &HookOutcome| o.stdout.trim().is_empty(), |o| o.stdout.contains("\"ask\""));
}

fn sys_msg(o: &HookOutcome) -> String {
    serde_json::from_str::<serde_json::Value>(o.stdout.trim()).ok().and_then(|v| v["systemMessage"].as_str().map(str::to_string)).unwrap_or_default()
}

// F1: using module runs module code when executed, so it must ask and the parser side must run nothing (marker file)
#[test]
fn f1_using_module_asks_and_nothing_runs() {
    let p = tmp("f1");
    fs::write(p.join("x.psm1"), "Set-Content -LiteralPath (Join-Path $PSScriptRoot 'marker.txt') -Value 1\n").unwrap();
    // Use an absolute path so the marker assertion is meaningful regardless of the parser child's cwd
    let o = call(&p, &format!("using module {}; Get-ChildItem build", p.join("x.psm1").display()));
    assert!(o.stdout.contains("\"ask\""), "using module must ask: {}", o.stdout);
    assert!(!p.join("marker.txt").exists(), "module code ran during parsing");
    asks(r#"using assembly .\x.dll; Get-ChildItem build"#);
}

// F2: non-value-type casts are constructors (create/truncate files) and must ask; value-type casts are reads
#[test]
fn f2_type_conversion_to_non_value_type_asks() {
    asks(r#"[System.IO.StreamWriter]'build\a.txt'"#);
    asks(r#"Get-ChildItem build | ForEach-Object { [IO.FileStream]$_.FullName }"#);
    passes_with_parser(r#"Get-ChildItem build | ForEach-Object { [int]$_.Length }"#);
    // `-as [type]` also calls a constructor (creates a file on PS 5.1), so only value types count as reads
    asks(r#"Get-ChildItem build | ForEach-Object { $_.FullName -as [IO.StreamWriter] }"#);
    passes_with_parser(r#"Get-ChildItem build | ForEach-Object { '1' -as [int] }"#);
}

// Value-type static write heads must ask (unknown); no claim that they modify files (`[regex]::Replace` touches no file)
#[test]
fn c2_value_type_write_like_static_asks_without_claiming_a_file_write() {
    let cmd = r#"Get-ChildItem build | ForEach-Object { [regex]::Replace($_.Name,'a','b') }"#;
    let o = call(&tmp("c2"), cmd);
    assert!(o.stdout.contains("\"ask\""), "{}", o.stdout);
    // Parser available: info line and no false overwrite claim. Unavailable or slow start: ask (still no false claim)
    let honest = |o: &HookOutcome| o.stdout.contains("\"ask\"") && !sys_msg(o).contains("overwrite files");
    gate(cmd, || call(&tmp("c2"), cmd), |o: &HookOutcome| honest(o) && sys_msg(o).contains("Not recognized: [regex]::Replace()"), honest);
}

// F3: blocks passed as parameter arguments (colon form) or by abbreviation are opaque and must ask; literal blocks are reads
#[test]
fn f3_parameter_arguments_and_abbreviations() {
    asks(r#"Get-ChildItem build | ForEach-Object -Process:$sb"#);
    asks(r#"Get-ChildItem build | ForEach-Object -Process $sb"#);
    asks(r#"Get-ChildItem build | Where-Object -F $sb"#);
    asks(r#"Get-ChildItem build | Where-Object -FilterScript:$sb"#);
    asks(r#"Get-ChildItem build | Where-Object -Fi:$sb"#);
    passes_with_parser(r#"Get-ChildItem build | ForEach-Object -Process:{ $_.Name }"#);
    passes_with_parser(r#"Get-ChildItem build | Where-Object -FilterScript:{ $_.Length -gt 0 }"#);
}

// No facts means unknown (ask). Statements outside the allowlist (switch, class, trap, ...) also ask
#[test]
fn r1_r2_no_facts_or_unlisted_nodes_ask() {
    asks("$x");
    asks(r#"Get-ChildItem build | ForEach-Object { switch ($_.Name) { 'a' { 1 } } }"#);
    asks(r#"class A { [void] Go() { } }; Get-ChildItem build"#);
    asks(r#"Get-ChildItem build | ForEach-Object { trap { 1 }; $_.Name }"#);
}

// F7: static methods with write semantics are prompted even on value types
#[test]
fn f7_static_write_methods_on_value_types_card() {
    asks(r#"Get-ChildItem build | ForEach-Object { [IO.Path]::GetTempFileName() }"#);
    passes_with_parser(r#"Get-ChildItem build | ForEach-Object { [IO.Path]::GetFileName($_.FullName) }"#);
}

// Info line is ASCII and at most 80 chars (no control characters, no long text)
#[test]
fn r5_info_line_is_short_ascii() {
    let long = "a".repeat(200);
    let o = call(&tmp("r5"), &format!("Get-ChildItem build | ForEach-Object {{ & $한글{long} }}"));
    assert!(o.stdout.contains("\"ask\""), "{}", o.stdout);
    let m = serde_json::from_str::<serde_json::Value>(o.stdout.trim()).unwrap()["systemMessage"].as_str().unwrap().to_string();
    for l in m.lines().filter(|l| l.starts_with("Not recognized: ")) {
        let rest = &l["Not recognized: ".len()..];
        assert!(rest.chars().count() <= 80 && rest.chars().all(|c| (' '..='~').contains(&c)), "info line: {l}");
    }
}

// Dynamic method names, foreach provider variables, positional Where-Object, instance Replace
#[test]
fn recheck1_dynamic_names_foreach_provider_where_positional_and_replace() {
    // Dynamic method name on a value-type static call
    asks(r#"$m="GetTempFileName"; Get-ChildItem build | ForEach-Object { [IO.Path]::$m() }"#);
    // foreach loop variable is a provider path (writes to the file while looping)
    asks(r#"foreach (${c:\x\a.txt} in 1,2) { Get-ChildItem build }"#);
    // Positional `$sb` for Where-Object is bound as FilterScript and runs; name/comparison forms stay reads
    asks(r#"Get-ChildItem build | Where-Object -InputObject 1 $sb"#);
    passes_with_parser(r#"Get-ChildItem build | Where-Object Name -eq a"#);
    // Instance `.Replace()` is unknown (string replace or FileInfo.Replace); never claim it overwrites a file
    let cmd = r#"Get-ChildItem build | ForEach-Object { $_.Name.Replace("a","b") }"#;
    let o = call(&tmp("c7"), cmd);
    assert!(o.stdout.contains("\"ask\""), "{}", o.stdout);
    let honest = |o: &HookOutcome| o.stdout.contains("\"ask\"") && !sys_msg(o).contains("overwrite files");
    gate(cmd, || call(&tmp("c7"), cmd), |o: &HookOutcome| honest(o) && sys_msg(o).contains("Not recognized: .Replace()"), honest);
}

// The value-type list (types whose casts and static calls count as reads) is pinned literally; growing it fails this test
#[test]
fn r4_value_types_are_pinned() {
    let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/classify.rs")).unwrap();
    let line = src.lines().find(|l| l.starts_with("const PS_VALUE_TYPES: &[&str] = &[")).expect("PS_VALUE_TYPES line");
    let got: Vec<&str> = line.split('[').nth(2).unwrap().trim_end_matches("];").split(',').map(|s| s.trim().trim_matches('"')).collect();
    assert_eq!(got, ["math", "string", "io.path", "convert", "datetime", "timespan", "regex", "text.regularexpressions.regex", "int", "int32", "int64", "double"], "value-type list changed; update this table together with a review");
}

// The allowlist is pinned to the script text; any change fails this test (no widening without review)
#[test]
fn r2_allow_list_is_pinned() {
    let src = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ps_ast.rs")).unwrap();
    let line = src.lines().find(|l| l.starts_with("$ok=@(")).expect("allowlist line `$ok=@(`");
    let mut got: Vec<&str> = line.trim_start_matches("$ok=@(").trim_end_matches(')').split(',').map(|s| s.trim().trim_matches('\'')).collect();
    got.sort_unstable();
    let mut want = vec![
        "ArrayExpressionAst", "ArrayLiteralAst", "AssignmentStatementAst", "BinaryExpressionAst", "BreakStatementAst", "CatchClauseAst", "CommandAst",
        "CommandExpressionAst", "CommandParameterAst", "ConstantExpressionAst", "ContinueStatementAst", "ConvertExpressionAst", "DoUntilStatementAst",
        "DoWhileStatementAst", "ExpandableStringExpressionAst", "FileRedirectionAst", "ForEachStatementAst", "ForStatementAst", "HashtableAst",
        "IfStatementAst", "IndexExpressionAst", "InvokeMemberExpressionAst", "MemberExpressionAst", "MergingRedirectionAst", "NamedBlockAst",
        "ParenExpressionAst", "PipelineAst", "ReturnStatementAst", "ScriptBlockAst", "ScriptBlockExpressionAst", "StatementBlockAst",
        "StringConstantExpressionAst", "SubExpressionAst", "TryStatementAst", "TypeConstraintAst", "TypeExpressionAst", "UnaryExpressionAst", "VariableExpressionAst",
        "WhileStatementAst",
    ];
    want.sort_unstable();
    assert_eq!(got, want, "allowlist changed; update this table together with a review");
}
