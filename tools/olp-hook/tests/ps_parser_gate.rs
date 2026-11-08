//! Guard: tests that depend on the PowerShell syntax parser must use only the shared gate `common::require_ps_parser_or_env_fallback`.
// Value: protects=slow powershell.exe startup on CI runners (exceeding the 1.5 s limit makes the product ask = safe side) does not make tests flaky, while parser regressions are still caught;
// fails_when=a test file (tests/*.rs, or src/*.rs below #[cfg(test)]) calls `ps_ast::enabled(` or `ps_ast::parse(` directly;
// why_new=the same flakiness occurred repeatedly in CI, so a guard replaces a written rule;
// seam=test source text; no_claim=product code (non-test parts of src) is not checked.
use std::fs;
use std::path::Path;

const DIRECT: [&str; 2] = ["ps_ast::enabled(", "ps_ast::parse("];

#[test]
fn parser_dependent_tests_go_through_the_shared_gate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut bad = Vec::new();
    let mut scan = |name: String, text: &str| {
        for (i, line) in text.lines().enumerate() {
            if DIRECT.iter().any(|d| line.contains(d)) {
                bad.push(format!("{name}:{}: {}", i + 1, line.trim()));
            }
        }
    };
    for e in fs::read_dir(root.join("tests")).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if p.extension().is_some_and(|x| x == "rs") && name != "ps_parser_gate.rs" {
            scan(format!("tests/{name}"), &fs::read_to_string(&p).unwrap());
        }
    }
    for e in fs::read_dir(root.join("src")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "rs") {
            let text = fs::read_to_string(&p).unwrap();
            if let Some(at) = text.find("#[cfg(test)]") {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                let skip = text[..at].lines().count();
                for (i, line) in text[at..].lines().enumerate() {
                    if DIRECT.iter().any(|d| line.contains(d)) {
                        bad.push(format!("src/{name}:{}: {}", skip + i + 1, line.trim()));
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "tests that depend on the parser must use require_ps_parser_or_env_fallback from tests/common/mod.rs ({} places):\n{}", bad.len(), bad.join("\n"));
}
