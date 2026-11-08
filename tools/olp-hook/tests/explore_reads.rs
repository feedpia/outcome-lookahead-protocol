//! Field-session "exploration" fixture: read commands an agent really produced must raise no permission dialog, empty card or bypass block, while delete commands from the same sessions still get a card.
// Value: protects=read commands the agent actually produced in a field session raise no permission dialog, empty card or bypass block; delete commands from the same session still get a card;
// fails_when=any command in group A produces a card; any group B delete passes without a card; why_new=third repeat of the same class of mistake, so a fixture instead of a rule;
// seam=run_hook (PowerShell and Bash tool inputs); no_claim=not every read command an agent could produce (field sample only).
// Source material: explore_commands.txt, the tool_use input.command of 12 recorded Claude Code sessions, verbatim.
// After each new field session, append the new commands to the end of tables A and B in the same form (source = record time, tool). Do not edit the text.
mod common;

use olp_hook::{run_hook, HookOutcome};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// Same shape as the field practice folder: app.log, cache.tmp, notes.txt under build\.
fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_explore_reads").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(p.join("build")).unwrap();
    for f in ["app.log", "cache.tmp", "notes.txt"] {
        fs::write(p.join("build").join(f), "x").unwrap();
    }
    p
}

fn call(p: &Path, tool: &str, cmd: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd}, "permission_mode": "default"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

/// Pinned material version: first 16 hex of the sha256 of explore_commands.txt = 31300cf4d231bbcf (one B line added; previous version 9270c5dc5488d5e9). Update this value whenever lines are added.
/// A. Exploration: must produce no card. (source = session record time UTC, tool, command)
const READS: &[(&str, &str, &str)] = &[
    ("2026-11-02T10:21:09.835Z", "PowerShell", r#"Get-ChildItem build -Recurse -Force | Select-Object FullName,Length,LastWriteTime; Get-Content build\notes.txt -TotalCount 30; git log --oneline -3 -- build"#),
    ("2026-11-02T10:28:05.557Z", "PowerShell", r#"Get-ChildItem -Recurse -Force build | Select-Object FullName, Length, LastWriteTime"#),
    ("2026-11-02T11:29:35.669Z", "PowerShell", r#"Get-ChildItem build -Recurse -File | Select-Object FullName, Length, LastWriteTime; git ls-files build; "---notes.txt---"; Get-Content build\notes.txt -TotalCount 20"#),
    ("2026-11-02T12:59:19.071Z", "Bash", r#"cd "D:/work/proj" && ls -d build 2>&1 && du -sh build 2>/dev/null && git check-ignore build; git ls-files build | head -3"#),
    ("2026-11-02T13:08:31.017Z", "PowerShell", r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Select-Object FullName; git status --short | Select-Object -First 20; if (Test-Path build) { (Get-ChildItem build -Recurse -File | Measure-Object -Property Length -Sum) | Select-Object Count, Sum; git ls-files build | Select-Object -First 5; git check-ignore build }"#),
    ("2026-11-02T13:09:27.894Z", "PowerShell", r#"Get-ChildItem build -Recurse -Force | Select-Object FullName, Length, LastWriteTime; Write-Output "--- notes.txt ---"; Get-Content build\notes.txt -TotalCount 20"#),
    ("2026-11-02T13:14:15.934Z", "PowerShell", r#"Get-ChildItem -Directory -Force | Select-Object Name; git status --short | Select-Object -First 20; if (Test-Path build) { (Get-ChildItem build -Recurse -Force | Measure-Object -Property Length -Sum) | Select-Object Count, Sum; git ls-files build | Select-Object -First 5; git check-ignore build }"#),
    ("2026-11-02T13:14:49.072Z", "PowerShell", r#"Get-ChildItem build -Recurse -Force | Select-Object FullName, Length, LastWriteTime; Get-Content build\notes.txt -TotalCount 30"#),
    ("2026-11-03T01:56:32.679Z", "PowerShell", r#"Get-ChildItem -Directory -Filter build -Recurse -Depth 2 -ErrorAction SilentlyContinue | Select-Object FullName, LastWriteTime; git status --short | Select-Object -First 10; git check-ignore build"#),
    ("2026-11-03T01:56:56.778Z", "PowerShell", r#"Get-ChildItem -Recurse -Force build | Select-Object FullName, Length; git ls-files build | Select-Object -First 5"#),
    ("2026-11-03T01:59:38.419Z", "PowerShell", r#"Test-Path build; git status --short"#),
    ("2026-11-03T01:59:42.630Z", "PowerShell", r#"Get-ChildItem -Recurse -Force build | Select-Object FullName, Length, LastWriteTime; Get-Content build\notes.txt"#),
    ("2026-11-03T02:04:06.711Z", "PowerShell", r#"Get-ChildItem -Directory -Force | Select-Object Name; if (Test-Path build) { "--- build ---"; Get-ChildItem build -Force | Select-Object -First 20 Name; (Get-ChildItem build -Recurse -File -Force | Measure-Object -Property Length -Sum) | Select-Object Count,Sum; git status --short --ignored build | Select-Object -First 5; git ls-files build | Select-Object -First 5 }"#),
];

/// Group A commands containing Where-Object / ForEach-Object blocks: where a PowerShell parser is available (Windows) no card and within 2 s,
/// elsewhere (macOS/Linux CI) a card as today (fallback = ask).
const PARSER_READS: &[(&str, &str, &str)] = &[
    ("2026-11-02T10:21:04.168Z", "PowerShell", r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName; git status --short | Select-Object -First 20; if (Test-Path build) { (Get-ChildItem build -Recurse -File | Measure-Object -Sum Length) | Select-Object Count,Sum; git ls-files build | Select-Object -First 5 }"#),
    ("2026-11-02T10:26:19.091Z", "PowerShell", r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName, LastWriteTime; git status --short | Select-Object -First 20; git check-ignore build"#),
    ("2026-11-02T11:25:40.809Z", "PowerShell", r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum); "{0}  files={1}  MB={2:N1}" -f $_.FullName, $s.Count, ($s.Sum/1MB) }; git status --short -- build 2>$null | Select-Object -First 5; git ls-files build | Measure-Object | Select-Object -ExpandProperty Count"#),
    ("2026-11-03T02:08:14.977Z", "PowerShell", r#"Get-ChildItem -Directory -Recurse -Depth 3 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum); "{0} | files={1} | MB={2:N1}" -f $_.FullName, $s.Count, ($s.Sum/1MB) }; git status --short | Select-Object -First 10; git check-ignore build"#),
];

/// B. Deletes from the same sessions: must produce a card (the 2 lines without a time have no time in the source file either).
const DELETES: &[(&str, &str, &str)] = &[
    ("2026-11-02T10:21:16.330Z", "PowerShell", r#"Remove-Item -Recurse -Force build -Confirm:$false; Test-Path build; git status --short"#),
    ("(no timestamp)", "PowerShell", r#"Remove-Item -Recurse -Force build -Confirm:$false; Test-Path build"#),
    ("2026-11-02T10:28:59.005Z", "PowerShell", r#"Remove-Item -Force build\app.log, build\cache.tmp -Confirm:$false; Get-ChildItem -Force build | Select-Object Name"#),
    ("2026-11-02T11:30:42.439Z", "PowerShell", r#"Remove-Item build\app.log, build\cache.tmp -Confirm:$false; Get-ChildItem build -File | Select-Object Name, Length; git status --short -- build"#),
    ("2026-11-02T11:36:56.418Z", "PowerShell", r#"Remove-Item -Recurse -Force build"#),
    ("2026-11-02T11:37:32.996Z", "PowerShell", r#"Remove-Item -Force build\app.log, build\cache.tmp; Get-ChildItem build"#),
    ("2026-11-02T12:59:40.497Z", "Bash", r#"cd "D:/work/proj" && rm -rf build && ls -d build 2>&1; git status --short | head"#),
    ("2026-11-02T13:10:29.333Z", "PowerShell", r#"Remove-Item build\app.log, build\cache.tmp -Force -Confirm:$false; Get-ChildItem build -Force | Select-Object Name; git status --short"#),
    ("2026-11-03T02:00:32.021Z", "PowerShell", r#"Remove-Item -Force build\app.log, build\cache.tmp -Confirm:$false; Get-ChildItem -Force build | Select-Object Name; git status --short"#),
    ("2026-11-03T02:05:27.821Z", "PowerShell", r#"Remove-Item -Force build\app.log, build\cache.tmp -Confirm:$false; Get-ChildItem build -Force | Select-Object Name; git status --short"#),
    ("2026-11-03T02:08:23.888Z", "Bash", r#"git ls-files build; git check-ignore -v build/app.log; rm -rf build && ls"#),
    ("(no timestamp)", "Bash", r#"cd /d/work/proj && git ls-files build; git check-ignore -v build/app.log; rm -rf build && ls"#),
];

#[test]
fn field_explore_reads_show_no_card() {
    let mut bad = Vec::new();
    for (src, tool, cmd) in READS {
        let o = call(&tmp("read"), tool, cmd);
        if o.code != 0 || !o.stdout.trim().is_empty() {
            bad.push(format!("{src} {tool}: {cmd}\n  → code {} {}{}", o.code, o.stdout.trim(), o.stderr.trim()));
        }
    }
    assert!(bad.is_empty(), "{} exploration commands produced a card:\n{}", bad.len(), bad.join("\n"));
}

/// Parser present = no card; absent or slow start = ask as today (shared gate common::require_ps_parser_or_env_fallback).
/// The 1.5 s product limit and the 2 s per-call cap in this test are unchanged.
#[test]
fn block_reads_need_the_parser_and_stay_fast() {
    for (src, tool, cmd) in PARSER_READS {
        let run = || {
            let t = std::time::Instant::now();
            let o = call(&tmp("block"), tool, cmd);
            assert!(t.elapsed().as_millis() < 2000, "{src}: {} ms", t.elapsed().as_millis());
            o
        };
        common::require_ps_parser_or_env_fallback(&format!("{src} {tool}: {cmd}"), run, |o: &HookOutcome| o.stdout.trim().is_empty(), |o| o.stdout.contains("\"ask\""));
    }
}

#[test]
fn field_deletes_still_show_a_card() {
    for (src, tool, cmd) in DELETES {
        let o = call(&tmp("del"), tool, cmd);
        assert!(o.stdout.contains("\"ask\"") || o.code != 0, "{src} {tool}: delete but no card: {cmd}\n{}", o.stdout);
    }
}

// ---- Bash syntax words (for/do/done/if/then/fi/while/{ }) and shell builtins `[`, test, read ----

fn call_mode(p: &Path, cmd: &str, mode: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": cmd}, "permission_mode": mode});
    run_hook(v.to_string().as_bytes(), Some(p))
}

/// A. Reads containing only syntax words: no card (modes default and auto).
const R1_READS: &[&str] = &[
    r#"for d in build; do du -sh "$d"; done"#,
    "if [ -d build ]; then ls build; fi",
    r#"while read f; do cat "$f"; done < list.txt"#,
    "test -d build && ls build",
    "{ ls; git status --short; }",
];

#[test]
fn r1_syntax_word_reads_show_no_card() {
    let mut bad = Vec::new();
    for mode in ["default", "auto"] {
        for cmd in R1_READS {
            let o = call_mode(&tmp("r1read"), cmd, mode);
            if o.code != 0 || !o.stdout.trim().is_empty() {
                bad.push(format!("[{mode}] {cmd}\n  → code {} {}{}", o.code, o.stdout.trim(), o.stderr.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "{} syntax-word reads produced a card:\n{}", bad.len(), bad.join("\n"));
}

/// B baseline: stored stdout from a trunk build with permission_mode=default, verbatim.
const B_BASELINE: &[(&str, &str)] = &[
    (r#"rm -rf build"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: delete files — 3 files (preview: counted on this device, read-only) | Files: build/app.log, build/cache.tmp, build/notes.txt | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: delete files — 3 files (preview: counted on this device, read-only)\nFiles: build/app.log, build/cache.tmp, build/notes.txt\nUndo: not confirmed — no recovery location declared\nWhy: WRITE_IRREVERSIBLE, WRITE_RECOVERY_UNKNOWN"}"###),
    (r#"for d in build; do rm -rf "$d"; done"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: delete files — count not checked (this form is not counted) | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: delete files — count not checked (this form is not counted)\nNot classified yet for this project, so it is treated as a write.\nClassify it once in your own terminal (not through the agent), then run it again:\n  olp-hook classify --bash \"for d\" read_only\n  olp-hook classify --bash \"done\" read_only\n  (if it writes: writes_other or moves_money, add --reversible --recovery-declared only if true)"}"###),
    (r#"for d in build; do cp a "$d"; done"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: overwrite files — count not checked (this form is not counted) | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: overwrite files — count not checked (this form is not counted)\nNot classified yet for this project, so it is treated as a write.\nClassify it once in your own terminal (not through the agent), then run it again:\n  olp-hook classify --bash \"for d\" read_only\n  olp-hook classify --bash \"done\" read_only\n  (if it writes: writes_other or moves_money, add --reversible --recovery-declared only if true)"}"###),
    (r#"ls > out.txt"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: overwrite files — 0 files (preview: counted on this device, read-only) | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: overwrite files — 0 files (preview: counted on this device, read-only)\nUndo: not confirmed — no recovery location declared\nWhy: WRITE_IRREVERSIBLE, WRITE_RECOVERY_UNKNOWN"}"###),
    (r#"if true; then git reset --hard; fi"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: discard uncommitted git changes — count not checked (this form is not counted) | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: discard uncommitted git changes — count not checked (this form is not counted)\nNot classified yet for this project, so it is treated as a write.\nClassify it once in your own terminal (not through the agent), then run it again:\n  olp-hook classify --bash \"if true\" read_only\n  olp-hook classify --bash \"fi\" read_only\n  (if it writes: writes_other or moves_money, add --reversible --recovery-declared only if true)"}"###),
    (r#"{ rm -rf build; }"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: delete files — count not checked (this form is not counted) | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: delete files — count not checked (this form is not counted)\nNot classified yet for this project, so it is treated as a write."}"###),
    (r#"done > out.txt"#, r###"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"[OLP] Before this runs: overwrite files — 0 files (preview: counted on this device, read-only) | before engine check · no receipt"},"systemMessage":"OLP hook: check before it runs — before engine check · no receipt\nWhat it does: overwrite files — 0 files (preview: counted on this device, read-only)\nNot classified yet for this project, so it is treated as a write.\nClassify it once in your own terminal (not through the agent), then run it again:\n  olp-hook classify --bash \"done\" read_only\n  (if it writes: writes_other or moves_money, add --reversible --recovery-declared only if true)"}"###),
];

#[test]
fn r1_writes_keep_the_exact_baseline_card() {
    for (cmd, want) in B_BASELINE {
        let o = call_mode(&tmp("r1b"), cmd, "default");
        assert_eq!(o.code, 0, "{cmd}");
        // Must be identical text, except that the trailing "Not classified yet … classify for d/done" note on baseline cards (noise from treating for/do/done as commands)
        // is dropped and the generic delete card's Undo/Why lines come instead; everything else (decision, reason line, "What it does" line) is verbatim.
        if o.stdout != *want {
            let (new, old): (serde_json::Value, serde_json::Value) = (serde_json::from_str(&o.stdout).unwrap(), serde_json::from_str(want).unwrap());
            assert_eq!(new["hookSpecificOutput"], old["hookSpecificOutput"], "decision/reason differ from baseline: {cmd}");
            let head = |v: &serde_json::Value| v["systemMessage"].as_str().unwrap().lines().take(2).collect::<Vec<_>>().join("\n");
            assert_eq!(head(&new), head(&old), "the two card header lines differ from baseline: {cmd}");
            let (nm, om) = (new["systemMessage"].as_str().unwrap(), old["systemMessage"].as_str().unwrap());
            assert!(om.contains("Not classified yet") && !nm.contains("olp-hook classify"), "the change is not just removal of the noise note: {cmd}\n{nm}");
        }
        // mode without a dialog = blocked (exit 2)
        for mode in ["bypassPermissions", "dontAsk"] {
            assert_eq!(call_mode(&tmp("r1b"), cmd, mode).code, 2, "[{mode}] {cmd}");
        }
    }
}

/// C. Ask as today. Second value = text that must appear in the card (empty if none).
const R1_ASKS: &[(&str, &str)] = &[
    ("mytool --apply", r#"mytool\" read_only"#),
    (r#"for d in x; do mytool "$d"; done"#, r#"mytool\" read_only"#),
    ("case $x in a) ls;; esac", ""),
    (r#"eval "ls""#, ""),
    // Shell builtin names (`[`, test, read) count only as bare words without a path: `./test` and `tests/read` are real programs
    ("./test --wipe-all", ""),
    ("tests/read all", ""),
    (r"bin\test x", ""),
    // Additionally: path-qualified name + write `$(…)` = ask
    ("./echo $(rm -rf build)", ""),
];

#[test]
fn r1_unknown_commands_still_ask() {
    for (cmd, must) in R1_ASKS {
        for mode in ["default", "auto"] {
            let o = call_mode(&tmp("r1c"), cmd, mode);
            assert!(o.stdout.contains("\"ask\""), "[{mode}] must ask: {cmd}\n{}", o.stdout);
            assert!(o.stdout.contains(must), "[{mode}] candidate text {must} missing: {cmd}\n{}", o.stdout);
        }
        assert_eq!(call_mode(&tmp("r1c"), cmd, "bypassPermissions").code, 2, "{cmd}");
    }
}

// ---- read-only `$(…)` is expanded and inspected ----

/// A. `$(…)` that is read-only inside and out: no card (modes default and auto). The first line is the verbatim case from an auto-mode measurement.
const R2_READS: &[&str] = &[
    r#"ls -la && git status --short | head; find . -maxdepth 3 -type d -name build -not -path "*/node_modules/*" 2>/dev/null; for d in $(find . -maxdepth 3 -type d -name build -not -path "*/node_modules/*"); do du -sh "$d"; git ls-files "$d" | head -3; git check-ignore "$d"; done"#,
    "du -sh $(git rev-parse --show-toplevel)",
    r#"echo "$(git status --short)""#,
    r#"for f in $(find build -type f); do wc -c "$f"; done"#,
    r#"echo "$(git log -1 --format="%h %s")""#,
    "X=$(pwd) && ls build",
    "cd $(echo /tmp) && ls",
];

#[test]
fn r2_read_substitutions_show_no_card() {
    let mut bad = Vec::new();
    for mode in ["default", "auto"] {
        for cmd in R2_READS {
            let o = call_mode(&tmp("r2read"), cmd, mode);
            if o.code != 0 || !o.stdout.trim().is_empty() {
                bad.push(format!("[{mode}] {cmd}\n  → code {} {}{}", o.code, o.stdout.trim(), o.stderr.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "{} read-only `$(…)` cases produced a card:\n{}", bad.len(), bad.join("\n"));
}

/// B/C. `$(…)` containing writes, unknowns or self-protection: verdicts identical to the trunk build (baseline in a stored jsonl, modes default and bypassPermissions).
/// Option injection (`find … $(echo -delete)`, `git log $(echo --output=x)`, `rg $(echo --pre=sh)`) and command-position substitution (`$(echo rm)`, `env $(…) ls`) always ask.
#[test]
fn r2_guarded_substitutions_keep_the_exact_baseline() {
    let mut bad = Vec::new();
    for line in include_str!("readfp1_r2_guard.jsonl").lines().filter(|l| !l.trim().is_empty()) {
        let want: serde_json::Value = serde_json::from_str(line).unwrap();
        let (cmd, mode) = (want["cmd"].as_str().unwrap(), want["mode"].as_str().unwrap());
        let o = call_mode(&tmp("r2guard"), cmd, mode);
        if o.code as i64 != want["code"].as_i64().unwrap() || o.stdout != want["stdout"].as_str().unwrap() {
            bad.push(format!("[{mode}] {cmd}\n  → code {} {}", o.code, o.stdout.trim()));
        }
    }
    assert!(bad.is_empty(), "{} verdicts differ from baseline:\n{}", bad.len(), bad.join("\n"));
}

// ---- four names with no file-write or system-change option at all = reads ----
// Basis: every option in the GNU coreutils/diffutils `--help` was checked (sha256sum, md5sum: -c only reads the list file; tr: stdin only; cmp: compare only).

const R3_READS: &[&str] = &[
    "sha256sum build/app.log | cut -c1-16",
    "md5sum build/app.log build/notes.txt",
    "sha256sum -c sums.txt --quiet 2>/dev/null",
    "cmp -s build/app.log build/notes.txt && echo same",
    "echo ab | tr a-z A-Z",
    "tr -d x < build/app.log",
    "echo $(sha256sum build/app.log | cut -c1-16)",
];

#[test]
fn r3_hash_and_compare_reads_show_no_card() {
    let mut bad = Vec::new();
    for mode in ["default", "auto"] {
        for cmd in R3_READS {
            let o = call_mode(&tmp("r3read"), cmd, mode);
            if o.code != 0 || !o.stdout.trim().is_empty() {
                bad.push(format!("[{mode}] {cmd}\n  → code {} {}{}", o.code, o.stdout.trim(), o.stderr.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "{} read names produced a card:\n{}", bad.len(), bad.join("\n"));
}

/// Same names still ask as today: path-qualified executables, and file-write redirects.
#[test]
fn r3_names_still_ask_when_pathed_or_redirected() {
    for cmd in ["./sha256sum x", "tools/cmp a b", "sha256sum build/app.log > out.txt", "echo a | tr a b > out.txt"] {
        let o = call_mode(&tmp("r3ask"), cmd, "auto");
        assert!(o.stdout.contains("\"ask\""), "must ask: {cmd}\n{}", o.stdout);
        assert_eq!(call_mode(&tmp("r3ask"), cmd, "bypassPermissions").code, 2, "{cmd}");
    }
}

// ---- narrow option-aware rules: sed, sort, uniq, date + git ls-tree, cat-file, merge-base ----
// Principle: if any option or form that can write is visible, ask as today. Arguments containing variables or globs (which could expand to `-i` or `-o`) also ask.

const R3B_READS: &[&str] = &[
    "sed -n 1,40p build/notes.txt",
    "cd build && sed -n '10,20p' notes.txt",
    "sed -n '1,5p;8p' build/notes.txt",
    "sed -n '1,30p;30q' build/notes.txt | cut -c1-200",
    "sort build/app.log | uniq",
    "sort -u -k2 build/app.log",
    "uniq build/app.log",
    "sort build/app.log | uniq -c | sort -rn | head",
    "date +%Y-%m-%d",
    "date -u",
    "date -d yesterday +%s",
    "git ls-tree HEAD build",
    "git cat-file -p HEAD",
    "git merge-base HEAD main",
];

#[test]
fn r3b_option_checked_reads_show_no_card() {
    let mut bad = Vec::new();
    for mode in ["default", "auto"] {
        for cmd in R3B_READS {
            let o = call_mode(&tmp("r3bread"), cmd, mode);
            if o.code != 0 || !o.stdout.trim().is_empty() {
                bad.push(format!("[{mode}] {cmd}\n  → code {} {}{}", o.code, o.stdout.trim(), o.stderr.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "{} option-rule reads produced a card:\n{}", bad.len(), bad.join("\n"));
}

/// Write forms must always ask (bypass and dontAsk = exit 2).
const R3B_WRITES: &[&str] = &[
    "sed -i s/a/b/ build/notes.txt",
    "sed -n 1p build/notes.txt -i",
    "sed -n -i 1p build/notes.txt",
    "sed -ni 1p build/notes.txt",
    "sed -n '1w out.txt' build/notes.txt",
    "sed -n '1e rm -rf build' build/notes.txt",
    "sed -n 1p $F",
    "sed -n '$p' build/notes.txt", // `$` inside quotes cannot be told from text alone = ask (fail closed)
    "sed -n 1p *.log",
    "sed s/a/b/ build/notes.txt",
    "sed -n -e 1p build/notes.txt",
    "sed -n 1p build/notes.txt > out.txt",
    "/usr/bin/sed -n 1p build/notes.txt",
    "sort -o out.txt build/app.log",
    "sort -ro out.txt build/app.log",
    "sort --output=out.txt build/app.log",
    "sort --out=out.txt build/app.log",
    "sort --compress-program=sh build/app.log",
    "sort --co=sh build/app.log",
    "sort $OPT build/app.log",
    "./sort build/app.log",
    "uniq build/app.log out.txt",
    "uniq - out.txt",
    "uniq -- -a -b",
    "uniq build/*.log",
    "date -s 2020-01-01",
    "date --set=2020-01-01",
    "date --se=2020-01-01",
    "date -us 2020-01-01",
    "date 11081200",
    "date 110812002026.30",
    "git cat-file --textconv HEAD:build/app.log",
    "git cat-file --filters HEAD:build/app.log",
];

#[test]
fn r3b_write_forms_still_ask() {
    let mut bad = Vec::new();
    for cmd in R3B_WRITES {
        let o = call_mode(&tmp("r3bw"), cmd, "auto");
        if !o.stdout.contains("\"ask\"") || call_mode(&tmp("r3bw"), cmd, "bypassPermissions").code != 2 || call_mode(&tmp("r3bw"), cmd, "dontAsk").code != 2 {
            bad.push(format!("{cmd}\n  → {}", o.stdout.trim()));
        }
    }
    assert!(bad.is_empty(), "{} write forms did not ask:\n{}", bad.len(), bad.join("\n"));
}

// ---- Names carrying a path separator (`/`, `\`) are not treated as read names or wrappers ----
// `./ls` is a file named `ls` in the repo (whatever it does); `C:\x\ls.exe` and `/usr/bin/cat` cannot be verified as real read tools -> ask.
// Names without a path (`ls`, `ls.exe`, `Get-ChildItem`) are unchanged.

const M1_PATHED: &[(&str, &str)] = &[
    ("Bash", "./ls --x"),
    ("Bash", "bin/cat build/app.log"),
    ("Bash", "/usr/bin/cat build/app.log"),
    ("Bash", "./echo hi"),
    ("Bash", "tools/git status"),
    ("Bash", "./find build"),
    ("Bash", "./rg x"),
    ("Bash", "./sudo ls"),
    ("Bash", "./env ls"),
    ("Bash", "cd build && ./ls"),
    ("PowerShell", r".\echo hi"),
    ("PowerShell", r".\ls"),
    ("PowerShell", r"C:\x\ls.exe build"),
    ("PowerShell", r".\Get-ChildItem build"),
    ("PowerShell", r"Get-ChildItem build | .\ForEach-Object { $_ }"),
    ("PowerShell", r"Get-ChildItem build | .\Where-Object Name -eq a"),
];

#[test]
fn m1_pathed_names_are_not_read_names() {
    let mut bad = Vec::new();
    for (tool, cmd) in M1_PATHED {
        let p = tmp("m1");
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd}, "permission_mode": "auto"});
        let o = run_hook(v.to_string().as_bytes(), Some(&p));
        if !o.stdout.contains("\"ask\"") {
            bad.push(format!("{tool}: {cmd}\n  → code {} {}", o.code, o.stdout.trim()));
        }
    }
    assert!(bad.is_empty(), "{} path-qualified names did not ask:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn m1_bare_names_still_pass() {
    for (tool, cmd) in [("Bash", "ls -la"), ("Bash", "ls.exe build"), ("Bash", "sudo ls"), ("Bash", "cat build/app.log"), ("PowerShell", "Get-ChildItem build"), ("PowerShell", "echo hi")] {
        let o = call(&tmp("m1ok"), tool, cmd);
        assert!(o.code == 0 && o.stdout.trim().is_empty(), "{tool}: {cmd} → {}", o.stdout);
    }
}
