//! Signal-only spawn wrapper: the only place in this crate that launches curl (separate from the counter wrapper in counter/spawn.rs).
//! The argument list has one closed shape: fixed options + time limit (integer seconds) + destination URL (rejected if it looks like an option). The body goes only via stdin (no file read/write options).
//! The allowed protocol string comes from a data file (no URL or protocol literals in source). If curl is missing, Err = nothing is sent.

use std::io::Write;
use std::process::Stdio;
use std::sync::Mutex;

const PROTO_FILE: &str = include_str!("../../data/signal-proto.txt");
const CONTENT_TYPE: &str = "Content-Type: application/json";
/// curl prints, after the response, a last line with the seconds until the TLS connection was set up (0 = never set up,
/// so the request body never went out). Used only to word a failure; never sent anywhere.
const WRITE_OUT: &str = "\\n%{time_appconnect}";

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
        "-q", "--silent", "--show-error", "--fail", "--proto", proto(), "--max-redirs", "0", "--max-time", &timeout_s.to_string(), "--header", CONTENT_TYPE, "--data-binary", "@-", "--write-out", WRITE_OUT, "--url", endpoint,
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
        ["-q", "--silent", "--show-error", "--fail", "--proto", p, "--max-redirs", "0", "--max-time", t, "--header", ct, "--data-binary", "@-", "--write-out", w, "--url", e] => {
            // Time limit of 1-5 s. SessionStart uses the bundled 2 s (under its 3 s slot); a person running `signal` uses the bundled 5 s.
            *p == proto() && !p.is_empty() && *ct == CONTENT_TYPE && matches!(*t, "1" | "2" | "3" | "4" | "5") && *w == WRITE_OUT && endpoint_ok(e)
        }
        _ => false,
    }
}

/// /// One curl run (body on stdin). Not launched if outside the closed shape.
pub fn run(argv: &[String], body: &str) -> Result<(), String> {
    let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    if !check_argv(&refs) {
        return Err("not sent: arguments outside the closed list".to_string());
    }
    let timeout_s = refs[9].parse().unwrap_or(0);
    if let Ok(mut g) = CALLS.lock() {
        g.push(argv.to_vec());
    }
    let mut child = std::process::Command::new("curl")
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "not sent: curl not found".to_string())?;
    if let Some(mut si) = child.stdin.take() {
        let _ = si.write_all(body.as_bytes());
    }
    match child.wait_with_output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(failure_text(o.status.code(), timeout_s, request_went_out(&o.stdout))),
        Err(_) => Err(failure_text(None, timeout_s, false)),
    }
}

/// True when curl's last output line (WRITE_OUT) says the TLS connection was set up, i.e. the request could have reached the server.
fn request_went_out(stdout: &[u8]) -> bool {
    String::from_utf8_lossy(stdout).lines().last().and_then(|l| l.trim().parse::<f64>().ok()).is_some_and(|s| s > 0.0)
}

/// What a failed curl run means for the user, from its exit code and whether the request went out. A timeout (28) after
/// the connection was set up is not "not sent": a server that is still waking up may record the numbers after curl gave
/// up waiting. A timeout before that (for example a closed port that some curl builds take about 2 s to give up on) is.
pub fn failure_text(code: Option<i32>, timeout_s: u64, went_out: bool) -> String {
    match code {
        Some(28) if went_out => format!("no answer from the server within {timeout_s} s. It may still have arrived; no retry today."),
        Some(28) => format!("not sent: no connection to the server within {timeout_s} s"),
        Some(6 | 7) => "not sent: could not reach the server".to_string(),
        Some(22) => "not sent: the server answered with an error".to_string(),
        _ => "not sent: curl returned an error".to_string(),
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
        let a = argv("x://e", 6);
        let r: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        assert!(!check_argv(&r), "limit above 5 s rejected");
    }
}
