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
