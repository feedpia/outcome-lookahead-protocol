//! Branches of assess that depend on the PowerShell syntax parser, moved from the src/classify.rs test `powershell_script_block_gets_no_junk_classify_lines` (shared gate).
// Value: protects=a typical block command is an interpreted read when the parser is available, and is treated as a write when it is unavailable or slow to start (no candidates, only "Not classified yet"); fails_when=parser regression or a change in the fallback verdict;
// why_new=slow powershell.exe startup on CI runners (parser unavailable) made this flaky repeatedly; seam=classify::assess; no_claim=the full card text is covered by card_golden.
mod common;

use olp_hook::classify::{assess, Assessment, Limits};
use olp_hook::store::DeviceMap;
use std::path::Path;

fn run(c: &str) -> Assessment {
    assess("PowerShell", &serde_json::json!({ "command": c }), &DeviceMap::default(), Path::new("."), Limits { counter: None, ps_parser: true })
}

/// Fallback side (parser unavailable or slow start) = built-in list: treated as a write (no read pass, effect unknown), no candidates, and the card shows only "Not classified yet" (no classify lines).
fn written_off_without_junk(a: &Assessment) -> bool {
    let card = olp_hook::card::ask_card(a);
    !a.is_read() && a.effect().is_none() && a.to_classify.is_empty() && card.contains("Not classified yet") && !card.contains("olp-hook classify")
}

#[test]
fn s1_block_command_is_a_parsed_read_or_written_off_without_junk() {
    let cmd = r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s=(Get-ChildItem $_.FullName -Recurse -File | Measure-Object Length -Sum); "{0} files={1} MB={2:N1}" -f $_.FullName,$s.Count,($s.Sum/1MB) }; git status --short | Select-Object -First 10; git check-ignore build"#;
    common::require_ps_parser_or_env_fallback("block command", || run(cmd), |a: &Assessment| a.is_read(), |a| !a.is_read() && a.effect().is_none() && a.to_classify.is_empty());
}

#[test]
fn value_only_blocks_are_parsed_reads_or_written_off_without_junk() {
    for r in ["ForEach-Object { 1 }", "where-object x"] {
        common::require_ps_parser_or_env_fallback(r, || run(r), |a: &Assessment| a.is_read(), written_off_without_junk);
    }
}
