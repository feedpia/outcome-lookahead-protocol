//! `olp-hook --help` shows the full protocol name, Outcome Lookahead Protocol (OLP), on its first line.
//! Unknown arguments still go to hook mode (stdin) and block; help does not widen the hook's block path.

use std::io::Write;
use std::process::{Command, Stdio};

fn run(args: &[&str], stdin: &[u8]) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_olp-hook"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.take().expect("stdin").write_all(stdin).expect("write");
    let out = child.wait_with_output().expect("wait");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

// Value: protects=the three help forms (--help, -h, help) exit 0 without reading stdin, show the full protocol name on the first line and list the subcommands; fails_when=help falls into hook mode and ends as "blocked", or the first line shows only the abbreviation; seam=executable; no_claim=name collisions with other projects are left to a naming review.
#[test]
fn help_first_line_names_full_protocol() {
    for a in ["--help", "-h", "help"] {
        let (code, out, err) = run(&[a], b"");
        assert_eq!(code, 0, "{a}: {err}");
        assert_eq!(out.lines().next(), Some("olp-hook — Outcome Lookahead Protocol (OLP) device hook"), "{a}");
        for sub in ["install", "uninstall", "classify", "stats", "replay", "watch", "setup", "signal"] {
            assert!(out.contains(sub), "{a}: subcommand {sub} missing");
        }
        assert!(!err.contains("blocked"), "{a}: fell into hook mode");
        // Help must not mention olp-gui, which is not in the package
        assert!(!out.contains("olp-gui"), "{a}: help mentions olp-gui");
    }
}

// Value: protects=unknown arguments are still hook mode, so invalid input blocks (exit 2) as before; fails_when=adding help turns an unknown argument into "success 0" and Claude Code lets the tool through; seam=executable; no_claim=none beyond protects.
#[test]
fn unknown_arg_still_fails_closed() {
    let (code, _out, err) = run(&["--nope"], b"not json");
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("blocked"), "{err}");
}
