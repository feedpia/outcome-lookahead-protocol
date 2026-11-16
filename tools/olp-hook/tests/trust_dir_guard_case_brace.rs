//! 0.1.3 rc4 rev 17-3, design addendum 3 (second review RECHECK 14 of rel 49ed499ff): a glob range is read with the letters as
//! written (`[Z-p]` holds `o`; letter case is folded only when comparing), and a brace group too big to list is cut down to the
//! letters that can come first, not forgotten — so `touch out/{a..z}{a..z}.txt` stays ordinary and `cd ~ && … {.o,a}{l,b}…` reaches.
//! Nothing here touches the real trust folder: the hook only judges the text.
use olp_hook::classify::{self, Limits};
use olp_hook::store::DeviceMap;
use serde_json::json;
use std::path::PathBuf;

fn cwd() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_trust_dir_guard_case_brace");
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn guarded(tool: &str, input: serde_json::Value, at: &std::path::Path) -> bool {
    classify::assess(tool, &input, &DeviceMap::default(), at, Limits { counter: None, ps_parser: false }).self_guard
}

// Value: protects=a mixed-case range or a brace group past the list limit cannot reach the trust folder unseen, and big unrelated brace
// lists are not always-blocked; fails_when=the pattern is lower-cased before matching, or a big brace group is read as «anything» or
// «nothing»; seam=classify::assess
#[test]
fn ranges_keep_their_letters_and_big_braces_keep_their_first_letters() {
    let p = cwd();
    let cases: &[(&str, bool, &str, &str)] = &[
        // root 1: the range as written (`o` 0x6F is inside `Z` 0x5A .. `p` 0x70)
        ("P13-1", true, "Bash", r#"echo x > ~/.[Z-p]lp/projects/k/hook-map.json"#),
        ("P13-2", true, "Bash", r#"cd ~/.[Z-p]lp/projects/k && git init"#),
        ("P13-3", true, "PowerShell", r#"Set-Content -Path "$env:USERPROFILE\.[Z-p]lp\projects\k\hook-map.json" -Value x"#),
        ("D3-1", true, "Bash", r#"echo x > ~/.[N-P]lp/projects/k/hook-map.json"#),
        ("Q13-14", false, "Bash", r#"echo x > ~/.config/[A-Z]lp.txt"#),
        // root 2: a brace group past the list limit keeps the letters that can come first
        ("P13-6", true, "Bash", r#"cd ~ && for d in {.o,a}{l,b}{p,c}{,1}{,2}{,3}{,4}{,5}{,6}/projects/k; do cd $d && git init; done"#),
        ("U12-5", true, "Bash", r#"for d in ~/{.o,a}{l,b}{p,c}{,1}{,2}{,3}{,4}{,5}{,6}/projects/k; do cd $d && git init; done"#),
        ("P13-16", true, "Bash", r#"cd ~/.o{a..z}{a..z}/projects/k && git init"#),
        ("D3-2", true, "Bash", r#"cd ~ && echo x > {.o,q}lp/hook-map.json"#),
        ("D3-3", true, "Bash", r#"cd ~ && echo x > .o{a..z}p/hook-map.json"#),
        ("D3-4", true, "Bash", r#"echo x > ~/{.,}olp/projects/k/hook-map.json"#),
        ("Q13-2", false, "Bash", r#"touch out/{a..z}{a..z}.txt"#),
        ("Q13-3", false, "Bash", r#"touch out/{1..20}x{1..20}.txt"#),
        ("D3-5", false, "Bash", r#"touch {1..300}.txt"#),
        ("D3-6", false, "Bash", r#"touch a{1..5000}"#),
        // R15 blocking 1: the cut-down of a big brace group stays inside its own path piece; the pieces after it are still read
        ("R15-1", true, "Bash", r#"for d in ~/{a..z}{a..z}/../{.o,x}lp/projects/k; do cd $d && git init; done"#),
        ("S15-8", false, "Bash", r#"rm -f build/{a..z}{a..z}/*.o"#),
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter(|(_, want, tool, cmd)| guarded(tool, json!({ "command": cmd }), &p) != *want)
        .map(|(id, want, _, _)| format!("{id} want self-protection={want}"))
        .collect();
    assert!(bad.is_empty(), "case or brace check differs: {bad:?}");
}
