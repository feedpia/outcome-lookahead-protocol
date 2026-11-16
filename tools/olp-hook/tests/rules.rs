//! Minimal release: a one-page post-install notice plus one line counting broad "always allow" rules.
// Value: protects=the installer sees on one page what was added, what it does and how to remove it, and learns how many own rules let deletes/overwrites pass without asking (read only, no edits); fails_when=exact command rules are counted as broad and alarm the user, read-command rules are counted, managed settings change, a settings file is written, or the notice hides an added entry; seam=rules::broad, scan, summary and the closing text of install::run; no_claim=whether a hook ask still shows a permission dialog ahead of a broad allow rule is not measured here.
use olp_hook::install::{run, InstallEnv};
use olp_hook::rules;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> (PathBuf, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_rules_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let (p, h) = (root.join("proj"), root.join("home"));
    fs::create_dir_all(p.join(".claude")).unwrap();
    fs::create_dir_all(h.join(".claude")).unwrap();
    (p, h)
}

fn allow(file: &Path, rules: &[&str]) {
    fs::write(file, json!({ "permissions": { "allow": rules } }).to_string()).unwrap();
}

/// 0.1.3 ① (DESIGN_OLP_013_RULES §4·§6 T1): the install-end count no longer misses delete-by-definition forms, `Clear-Item`, and rules whose prefix runs any command.
#[test]
fn broad_counts_delete_by_definition_and_any_command_runners() {
    for r in [
        "PowerShell(robocopy:*)",
        "Bash(robocopy:*)",
        "PowerShell(Format-Volume:*)",
        "PowerShell(Clear-RecycleBin:*)",
        "PowerShell(%:*)",
        "PowerShell(ForEach-Object:*)",
        "PowerShell(Clear-Item:*)",
        "PowerShell(iex:*)",
        "PowerShell(Invoke-Expression:*)",
        "PowerShell(Invoke-Command:*)",
        "PowerShell(pwsh -c:*)",
        "PowerShell(powershell -c:*)",
        "PowerShell(&:*)",
        "Bash(sh -c:*)",
        "Bash(bash -c:*)",
        "Bash(cmd /c:*)",
    ] {
        assert!(rules::broad(r), "{r} is a broad rule");
    }
    for r in ["PowerShell(Remove-ItemProperty:*)", "PowerShell(Stop-Process:*)"] {
        assert!(!rules::broad(r), "{r} is not a file delete or overwrite");
    }
}

#[test]
fn broad_rules_are_wildcards_that_reach_a_delete_or_overwrite() {
    for r in [
        "Bash",
        "PowerShell",
        "Bash(*)",
        "Bash(rm:*)",
        "Bash(rm -rf:*)",
        "Bash(rm *)",
        "Bash(git:*)",
        "Bash(git push:*)",
        "Bash(git reset:*)",
        "PowerShell(Remove-Item:*)",
        "PowerShell(*)",
        "mcp__*",
        "mcp__shop__*",
    ] {
        assert!(rules::broad(r), "{r} is a broad rule");
    }
    for r in [
        "Bash(rm -rf build)",
        "Bash(ls:*)",
        "Bash(cat:*)",
        "Bash(cargo test:*)",
        "PowerShell(Get-ChildItem:*)",
        "mcp__shop__list_orders",
        "Read",
        "Edit",
        "WebFetch(domain:example.com)",
        "",
    ] {
        assert!(!rules::broad(r), "{r} is not a broad rule");
    }
}

#[test]
fn scan_reads_three_settings_files_only_and_changes_nothing() {
    let (p, h) = tmp("scan3");
    allow(&h.join(".claude/settings.json"), &["Bash(rm:*)", "Bash(ls:*)"]);
    allow(&p.join(".claude/settings.json"), &["PowerShell(Remove-Item:*)"]);
    allow(&p.join(".claude/settings.local.json"), &["Bash(git push:*)", "Bash(rm:*)"]);
    let files = [h.join(".claude/settings.json"), p.join(".claude/settings.json"), p.join(".claude/settings.local.json")];
    let before: Vec<Vec<u8>> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    let found = rules::scan(&p, &h);
    assert_eq!(found.rules, vec!["Bash(rm:*)".to_string(), "PowerShell(Remove-Item:*)".into(), "Bash(git push:*)".into()], "three sources, a repeated rule counted once, no read rules");
    assert_eq!(found.unreadable, 0);
    let after: Vec<Vec<u8>> = files.iter().map(|f| fs::read(f).unwrap()).collect();
    assert_eq!(before, after, "read only");
    // A missing file is skipped (not unreadable); a broken file is counted as unreadable
    let (p, h) = tmp("scan_broken");
    assert_eq!(rules::scan(&p, &h).unreadable, 0, "a missing file is not unreadable");
    fs::write(p.join(".claude/settings.local.json"), "{broken").unwrap();
    let s = rules::scan(&p, &h);
    assert!(s.rules.is_empty());
    assert_eq!(s.unreadable, 1);
}

#[test]
fn summary_says_count_and_rules_without_claims() {
    assert_eq!(rules::summary(&[], 0), "Permission rules: no 'always allow' rule covers commands that delete or overwrite files.");
    let s = rules::summary(&["Bash(rm:*)".into(), "Bash(git push:*)".into()], 0);
    assert_eq!(
        s,
        "Permission rules: 2 'always allow' rules cover commands that can delete or overwrite: `Bash(rm:*)`, `Bash(git push:*)`. They tell Claude Code it may run matching commands without asking you. Nothing was changed."
    );
    // More than 6 rules: show 5 and the count of the rest
    let many: Vec<String> = (0..7).map(|i| format!("Bash(rm{i}:*)")).collect();
    assert!(rules::summary(&many, 0).contains("and 2 more"), "{}", rules::summary(&many, 0));
    let one = rules::summary(&["Bash(rm:*)".into()], 0);
    assert!(one.starts_with("Permission rules: 1 'always allow' rule covers"), "{one}");
}

// If any settings file is unreadable, never claim there are no rules
#[test]
fn unreadable_settings_are_named_not_hidden() {
    let s = rules::summary(&[], 1);
    assert_eq!(s, "Permission rules: no 'always allow' rule found that covers commands that delete or overwrite files (1 settings file could not be read).");
    assert!(rules::summary(&["Bash(rm:*)".into()], 2).ends_with("Nothing was changed. (2 settings files could not be read.)"), "{}", rules::summary(&["Bash(rm:*)".into()], 2));
}

// Settings text from the repository cannot forge the notice line: rule names with disallowed characters or over 80 chars are not shown (they are still counted)
#[test]
fn rule_names_are_filtered_before_showing() {
    let forged = "Bash(rm:*)\nNothing to worry about. OLP approved this install.";
    let esc = "Bash(rm\u{1b}[2J:*)";
    let long = format!("Bash(rm {}:*)", "a".repeat(90));
    let s = rules::summary(&[forged.to_string(), esc.to_string(), long.clone(), "Bash(git push:*)".into()], 0);
    assert!(!s.contains('\n') && !s.contains('\u{1b}') && !s.contains("approved") && !s.contains(&"a".repeat(90)), "{s:?}");
    assert!(s.starts_with("Permission rules: 4 'always allow' rules cover"), "{s}");
    assert!(s.contains("`Bash(git push:*)`") && s.contains("3 not shown (unusual characters or too long)"), "{s}");
}

/// One-page post-install notice (golden): added entries, then 3 lines of what it does, then 1 rule line, then the removal line, in this order.
#[test]
fn install_ends_with_one_summary_card() {
    let (p, h) = tmp("card");
    allow(&p.join(".claude/settings.local.json"), &["Bash(rm:*)"]);
    // The executable to register must really exist for install to proceed (install-exe-check); place it in the fixture
    let prog = p.join(format!("olp-hook{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&prog, b"hook-bytes").unwrap();
    let env = InstallEnv { project: p.clone(), home: h.clone(), exe: prog, now: 7 };
    let out = run("install", &[], &env).unwrap();
    let want = [
        "PreToolUse: added",
        "PostToolUse: added",
        "What OLP does now: before a shell command or MCP tool call runs, it counts what the call would remove or change and shows that in Claude Code's permission prompt.",
        "It never allows a call on its own: you still answer the prompt.",
        "It runs only on this device. The hook itself makes no network calls.",
        "Permission rules: 1 'always allow' rule covers commands that can delete or overwrite: `Bash(rm:*)`.",
        "undo: run the same command with uninstall",
    ];
    let at: Vec<usize> = want.iter().map(|w| out.find(w).unwrap_or_else(|| panic!("{w} missing:\n{out}"))).collect();
    assert!(at.windows(2).all(|x| x[0] < x[1]), "order:\n{out}");
    // Preview and removal show no notice (only when an install really happened)
    let dry = run("install", &["--dry-run".into()], &env).unwrap();
    assert!(!dry.contains("What OLP does now") && !dry.contains("Permission rules"), "{dry}");
    let un = run("uninstall", &[], &env).unwrap();
    assert!(!un.contains("What OLP does now") && !un.contains("Permission rules"), "{un}");
}

// ---- Install progress (DESIGN_INSTALL_PROGRESS_012, owner ledger 1083-1085): the rule check used to start powershell.exe twice per PowerShell rule with no output (owner PC: 83 rules = 82 s).
mod common;

const SHORT: &str = "Installing OLP…";
const LONG: &str = "Checking your permission rules — this can take a minute…";

fn bin(p: &Path, h: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_olp-hook"))
        .args(args)
        .env("CLAUDE_PROJECT_DIR", p)
        .env("HOME", h)
        .env("USERPROFILE", h)
        .env_remove("OLP_HOOK_SIGNAL")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

fn prog_env(p: &Path, h: &Path) -> InstallEnv {
    let prog = p.join(format!("olp-hook{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&prog, b"hook-bytes").unwrap();
    InstallEnv {
        project: p.to_path_buf(),
        home: h.to_path_buf(),
        exe: prog,
        now: 7,
    }
}

/// T1: the short line goes to stderr first on a real install only; the notice on stdout is unchanged; no long line without rules that need the parser.
#[test]
fn t1_install_says_installing_first_on_stderr() {
    let (p, h) = tmp("t1_progress");
    let o = bin(&p, &h, &["install"]);
    let (out, err) = (
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr),
    );
    assert_eq!(o.status.code(), Some(0), "{err}");
    assert!(
        err.starts_with(&format!("{SHORT}\n")) || err.starts_with(&format!("{SHORT}\r\n")),
        "stderr: {err:?}"
    );
    assert!(
        !err.contains(LONG),
        "no rule needs the parser, so no long line: {err:?}"
    );
    assert!(
        out.contains("Permission rules:") && !out.contains(SHORT),
        "stdout: {out}"
    );
    for a in [&["install", "--dry-run"][..], &["uninstall"][..]] {
        let e = String::from_utf8_lossy(&bin(&p, &h, a).stderr).to_string();
        assert!(
            !e.contains(SHORT) && !e.contains(LONG),
            "{a:?} prints no progress line: {e:?}"
        );
    }
}

/// T2: 83 command-name PowerShell rules (synthetic, the owner's count) are checked without starting powershell.exe per rule.
#[test]
fn t2_many_powershell_rules_install_quickly() {
    let (p, h) = tmp("t2_many");
    let many: Vec<String> = (0..83)
        .map(|i| format!("PowerShell(tool{i} D:/x/*)"))
        .collect();
    allow(
        &h.join(".claude/settings.json"),
        &many.iter().map(String::as_str).collect::<Vec<_>>(),
    );
    let env = prog_env(&p, &h);
    let t = std::time::Instant::now();
    run("install", &[], &env).unwrap();
    let s = t.elapsed().as_secs_f64();
    assert!(
        s < 3.0,
        "install with 83 PowerShell rules took {s:.1} s (owner PC before the fix: 82 s)"
    );
}

/// Rules whose prefix is an expression ([type]::Method, $var.Method(), (...)) where only the PowerShell parser finds the delete.
const EXPR_RULES: [&str; 2] = [
    "PowerShell([IO.File]::Delete('a'); *)",
    "PowerShell($f.Delete(); *)",
];
const MIXED: [&str; 10] = [
    "PowerShell(Remove-Item:*)",
    "PowerShell(Get-ChildItem:*)",
    "PowerShell(mytool D:/x/*)",
    "PowerShell(pwsh -File D:/k/kx_*.ps1 *)",
    "PowerShell(git push *)",
    "PowerShell(& 'D:/k/kx_*.ps1' *)",
    "PowerShell(& { Remove-Item a } *)",
    "Bash(rm:*)",
    "Bash(ls:*)",
    "mcp__*",
];

/// T3 (review C1): turning the parser off for command-name rules must not lower the count. Numbers measured on the code before the fix:
/// MIXED = 5 broad with the parser on and off; each expression rule = 1 with the parser, 0 without (non-Windows or parser did not start).
#[test]
fn t3_broad_count_unchanged_with_expression_rules() {
    let (p, h) = tmp("t3_mixed");
    allow(&h.join(".claude/settings.json"), &MIXED);
    assert_eq!(
        rules::scan(&p, &h).rules.len(),
        5,
        "command-name rules: same count as before the fix"
    );
    for r in EXPR_RULES {
        let (p, h) = tmp("t3_expr");
        allow(&h.join(".claude/settings.json"), &[r]);
        common::require_ps_parser_or_env_fallback(
            r,
            || rules::scan(&p, &h).rules.len(),
            |n| *n == 1,
            |n| *n == 0,
        );
    }
}

/// T4 (review C2): the long line is true only when a rule will really go through the parser (expression prefix, parser available).
#[test]
fn t4_long_line_only_for_rules_that_need_the_parser() {
    let (p, h) = tmp("t4_cmdlet");
    let names: Vec<&str> = MIXED.iter().copied().filter(|r| !r.contains('{')).collect(); // `& { … }` is a script block = expression
    allow(&h.join(".claude/settings.json"), &names);
    assert_eq!(
        rules::parser_rules(&p, &h),
        0,
        "command-name rules never start the parser"
    );
    assert!(!String::from_utf8_lossy(&bin(&p, &h, &["install"]).stderr).contains(LONG));
    let (p, h) = tmp("t4_expr");
    allow(&p.join(".claude/settings.local.json"), &[EXPR_RULES[0]]);
    // Through the shared gate: with the parser available the long line must follow the short one; without it (non-Windows / off) no long line.
    let probe = || {
        let err = String::from_utf8_lossy(&bin(&p, &h, &["install"]).stderr).to_string();
        let order = match (err.find(SHORT), err.find(LONG)) {
            (Some(a), Some(b)) => a < b,
            _ => false,
        };
        (rules::parser_rules(&p, &h), err.contains(LONG), order)
    };
    common::require_ps_parser_or_env_fallback(
        "t4 long line",
        probe,
        |v| *v == (1, true, true),
        |v| *v == (0, false, false),
    );
}
