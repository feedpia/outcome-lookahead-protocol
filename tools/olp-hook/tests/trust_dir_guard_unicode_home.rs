//! 0.1.3 rc4 rev 17-3 (second review R15 coord side of 5a35d41eb, blocking 2): a home whose name has a non-English letter (`hö`)
//! written in the other letter case (`HÖ`) is the same folder on Windows and macOS, so a glob piece under it still reaches the trust
//! folder — letter case is folded the Unicode way when comparing, not only for A-Z.
//! HOME and USERPROFILE are changed, so this is a single-test binary (it does not run concurrently with other tests).
//! Nothing here touches the real trust folder: the hook only judges the text.
use olp_hook::classify::{self, Limits};
use olp_hook::store::DeviceMap;
use serde_json::json;
use std::path::PathBuf;

// Value: protects=a home name with a non-English letter cannot be spelled in the other case to slip a `*` piece past self-protection
// (weaker than 49ed499ff otherwise); fails_when=the literal compare folds only A-Z; seam=classify::assess
#[test]
fn a_non_english_home_name_in_the_other_case_still_reaches_the_trust_folder() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("olp_unicode_home_{}", std::process::id()));
    let home = root.join("hö");
    let work = root.join("work");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("USERPROFILE", &home);
    let slash = root.to_string_lossy().replace('\\', "/");
    let back = root.to_string_lossy().replace('/', "\\");
    let guarded = |tool: &str, cmd: String| {
        classify::assess(tool, &json!({ "command": cmd }), &DeviceMap::default(), &work, Limits { counter: None, ps_parser: false }).self_guard
    };
    let mut bad = Vec::new();
    let mut cases = vec![
        ("NA-5", "Bash", format!("echo x > {slash}/HÖ/*/projects/k/hook-map.json")),
        ("NA-6", "Bash", format!("echo x > {slash}/hö/*/projects/k/hook-map.json")),
    ];
    if cfg!(windows) {
        // a `\`-rooted PowerShell path is an absolute home path only on Windows (`C:\Users\…`)
        cases.push(("NA-7", "PowerShell", format!(r#"Set-Content -Path "{back}\HÖ\*\projects\k\hook-map.json" -Value x"#)));
    }
    for (id, tool, cmd) in cases {
        if !guarded(tool, cmd.clone()) {
            bad.push(format!("{id} {cmd}"));
        }
    }
    assert!(bad.is_empty(), "a home name in the other letter case slips past self-protection: {bad:?}");
}
