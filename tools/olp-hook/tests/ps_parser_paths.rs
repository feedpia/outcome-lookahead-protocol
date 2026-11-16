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

// Block command (test s1_block_command_is_a_parsed_read_or_written_off_without_junk):
// moved to simple_form_only::reads_that_now_ask (rc4 rev 15: not a simple form → asks)

#[test]
fn value_only_blocks_are_parsed_reads_or_written_off_without_junk() {
    // `ForEach-Object { 1 }`: moved to simple_form_only::reads_that_now_ask (rc4 rev 15: not a simple form → asks)
    let r = "where-object x";
    common::require_ps_parser_or_env_fallback(r, || run(r), |a: &Assessment| a.is_read(), written_off_without_junk);
}
