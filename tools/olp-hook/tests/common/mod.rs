//! Shared test helpers (tests/common/mod.rs; cargo does not run this file as a test target). Used by tests via `mod common;`.
#![allow(dead_code)]

/// The single gate for tests that depend on the PowerShell syntax parser path (guard checked literally by tests/ps_parser_gate.rs).
/// Reason: on CI runners powershell.exe can take longer than the product limit of 1.5 s to start, and then the product falls back to the built-in list (parser unavailable, ask = safe side);
/// the same flakiness showed up repeatedly in CI runs of the classify and card_golden tests.
/// - Parser off (non-Windows or `OLP_HOOK_PS_PARSER=off`): run once; the result must satisfy `fallback_ok`.
/// - Parser on: one warm-up plus up to 3 attempts. If `parsed_ok` holds, done. Otherwise the value must satisfy `fallback_ok` (the built-in list); if neither holds, fail.
///   If all 3 attempts fall back, try to parse the easy `Get-Date`: if it parses (Some) the parser is alive but this command is not parsed = parser regression, fail;
///   if it does not parse (None) the start was just slow, so accept. There is no way to wrongly accept `parsed_ok`.
pub fn require_ps_parser_or_env_fallback<T: std::fmt::Debug>(what: &str, mut run: impl FnMut() -> T, parsed_ok: impl Fn(&T) -> bool, fallback_ok: impl Fn(&T) -> bool) {
    if !olp_hook::ps_ast::enabled() {
        let v = run();
        assert!(fallback_ok(&v), "{what}: parser unavailable, so the result must match the built-in list (ask side): {v:?}");
        return;
    }
    let _ = olp_hook::ps_ast::parse("Get-Date"); // warm-up (result ignored)
    for _ in 0..3 {
        let v = run();
        if parsed_ok(&v) {
            return;
        }
        assert!(fallback_ok(&v), "{what}: result matches neither the parser path nor the fallback (built-in list): {v:?}");
    }
    assert!(olp_hook::ps_ast::parse("Get-Date").is_none(), "{what}: parser is alive (parsed Get-Date) but this command never got a parser result in 3 attempts = parser regression");
    eprintln!("{what}: parser did not start within 1.5 s; fell back to the built-in list (safe side), accepted");
}
