//! First-release defaults: the matcher covers Bash and MCP only, and the bundled default config carries a counting time limit (applied even without project config).

use std::time::Duration;

// Value: protects=the first install without a config file still counts how many of what, and counter limit < hook limit < Claude Code hook timeout; fails_when=the defaults disappear and the first prompt always says "count not checked"; seam=bundled data file; no_claim=the limit values are initial settings, not tuned.
#[test]
fn bundled_default_config_enables_counting() {
    let dir = std::env::temp_dir().join(format!("olp-hook-defaults-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp directory");
    let cfg = olp_hook::config::load(&dir).expect("no config means bundled defaults");
    let counter = cfg.counter_limit().expect("bundled defaults define a counter limit");
    let budget = cfg.hook_budget().expect("bundled defaults define a hook limit");
    assert!(counter < budget, "counter limit < hook limit");
    // The hook limit is shorter than the Claude Code hook timeout (seconds) documented in the README.
    assert!(budget < Duration::from_secs(10), "hook limit < Claude Code timeout of 10 s");
    assert!(!cfg.quiet, "quiet mode is off by default (project opt-in)");
    let _ = std::fs::remove_dir(&dir);
}

// Value: protects=the first release hooks only one-line command tools (Bash, PowerShell) and MCP tools (Edit/Write are tracked by Claude Code checkpoints, and no prompt claim is made for them; without the PowerShell tool, Windows shell commands would bypass the hook); fails_when=the README config line becomes "*" again or PowerShell is dropped; seam=README text; no_claim=no claim that Edit/Write can be undone.
#[test]
fn readme_matcher_is_shell_tools_and_mcp_only() {
    let readme = include_str!("../README.md");
    assert!(readme.contains(r#""matcher": "Bash|PowerShell|mcp__.*""#), "README config line matcher");
    assert!(!readme.contains(r#""matcher": "*""#), "no wildcard matcher for all tools");
    // The first release does not hook Edit/Write, so the README must not hide the limit that device files can be changed through edit tools
    assert!(readme.contains("Edit/Write") && readme.contains(".claude/settings.json") && readme.contains(".olp/hook-map.json"), "README Limits line");
    // The same executable also serves PostToolUse, the "Always allow" limit line, and the opt-in signal description
    assert!(readme.contains("\"PostToolUse\""), "README config line includes PostToolUse");
    assert!(readme.contains("Always allow"), "Always allow limit line");
    // Closed table of usage-count columns (column check lives in tests/usage.rs t8), OLP_HOOK_SIGNAL=off takes priority, day = UTC, off by default.
    assert!(
        olp_hook::signal::FIELDS.iter().all(|f| readme.contains(&format!("`{f}`")))
            && readme.contains("OLP_HOOK_SIGNAL=off")
            && readme.contains("UTC day")
            && readme.contains("off by default")
            && !readme.contains("exactly five fields"),
        "opt-in usage count column table, off takes priority, UTC day, off by default"
    );
}

// Value: protects=the public README does not describe the GUI file picker that is not in the package; fails_when=the README shows `gui serve`, olp-gui copy or the GUI picker limit line again; seam=README en/ko text; no_claim=olp-gui and pick code stay in the trunk (not removed).
#[test]
fn readme_does_not_offer_the_gui_picker() {
    for readme in [include_str!("../README.md"), include_str!("../README.ko.md")] {
        for bad in ["gui serve", "olp-gui", "\"pick\": true", "A pick in the GUI", "GUI 에서 고른 것"] {
            assert!(!readme.contains(bad), "README must not contain {bad}");
        }
    }
}

// Value: protects=public text addresses the reader with one fixed second-person form, so README.ko never reintroduces the plural form (second occurrence of the same mistake gets a guard); fails_when=a new section or edited line uses the plural form again; seam=README.ko text; no_claim=other Korean documents (site copy) are outside this test.
#[test]
fn readme_ko_says_dangsin_not_yeoreobun() {
    let ko = include_str!("../README.ko.md");
    assert_eq!(ko.matches("여러분").count(), 0, "README.ko must not contain the plural address form; use the singular form");
}
