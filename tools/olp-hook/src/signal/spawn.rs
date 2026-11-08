//! Signal-only spawn wrapper: the only place in this crate that launches curl (separate from the counter wrapper in counter/spawn.rs).
//! The argument list has one closed shape: fixed options + time limit (integer seconds) + destination URL (rejected if it looks like an option). The body goes only via stdin (no file read/write options).
//! The allowed protocol string comes from a data file (no URL or protocol literals in source). If curl is missing, Err = nothing is sent.

use std::io::Write;
use std::process::Stdio;
use std::sync::Mutex;

const PROTO_FILE: &str = include_str!("../../data/signal-proto.txt");
const CONTENT_TYPE: &str = "Content-Type: application/json";

static CALLS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

/// /// curl arguments launched so far (in memory only). Used for test assertions.
pub fn calls() -> Vec<Vec<String>> {
    CALLS.lock().map(|g| g.clone()).unwrap_or_default()
}

fn proto() -> &'static str {
    PROTO_FILE.trim()
}

/// /// The destination must start with this prefix, built from the allowed protocol in the data file (no protocol literals in source).
pub fn scheme_prefix() -> String {
    format!("{}://", proto().trim_start_matches('='))
}

/// /// `-q` must be the first argument (curl skips the user's `.curlrc`/`_curlrc` only when it is first) + `--max-redirs 0` (no redirects).
pub fn argv(endpoint: &str, timeout_s: u64) -> Vec<String> {
    [
        "-q", "--silent", "--show-error", "--fail", "--proto", proto(), "--max-redirs", "0", "--max-time", &timeout_s.to_string(), "--header", CONTENT_TYPE, "--data-binary", "@-", "--url", endpoint,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn endpoint_ok(e: &str) -> bool {
    !e.is_empty() && e.len() <= 512 && !e.starts_with('-') && !e.starts_with('@') && e.chars().all(|c| c.is_ascii_graphic())
}

/// /// Check against the closed shape (always run just before launching).
pub fn check_argv(a: &[&str]) -> bool {
    match a {
        ["-q", "--silent", "--show-error", "--fail", "--proto", p, "--max-redirs", "0", "--max-time", t, "--header", ct, "--data-binary", "@-", "--url", e] => {
            //             // Time limit of only 1-2 s (at most 3 s overall; shorter than the 3 s SessionStart timeout)
            *p == proto() && !p.is_empty() && *ct == CONTENT_TYPE && matches!(*t, "1" | "2") && endpoint_ok(e)
        }
        _ => false,
    }
}

/// /// One curl run (body on stdin). Not launched if outside the closed shape.
pub fn run(argv: &[String], body: &str) -> Result<(), &'static str> {
    let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    if !check_argv(&refs) {
        return Err("arguments outside the closed list");
    }
    if let Ok(mut g) = CALLS.lock() {
        g.push(argv.to_vec());
    }
    let mut child = std::process::Command::new("curl")
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "curl not found")?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(body.as_bytes());
    }
    match child.wait() {
        Ok(st) if st.success() => Ok(()),
        _ => Err("curl returned an error"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn argv_is_closed() {
        let a = argv("x://example.invalid/s", 2);
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        assert!(check_argv(&r));
        for bad in ["", "-K", "@/etc/passwd", "a b"] {
            let a = argv(bad, 2);
            let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
            assert!(!check_argv(&r), "{bad:?}");
        }
        let a = argv("x://e", 0);
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        assert!(!check_argv(&r), "시간 상한 0 거절");
        let a = argv("x://e", 3);
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        assert!(!check_argv(&r), "2초 넘는 상한 거절");
    }
}
