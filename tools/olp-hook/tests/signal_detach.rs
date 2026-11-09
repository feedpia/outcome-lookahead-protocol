//! Session-start usage signal must not make the session wait (T7, 0.1.2).
// Value: protects=Claude's first reply never waits on the usage signal, even when the server is slow to wake; the send still goes out after the hook exits and curl gives up on its own; fails_when=`signal --session-start` waits for the server, prints anything, exits non-zero, or leaves curl running past its 5 s limit; seam=the `olp-hook signal --session-start` executable against a 127.0.0.1 TCP server that accepts but never answers (TLS is never completed, so only "curl connected and sent its first bytes" is observable); no_claim=that the body reaches the real server (checked on the production daily count).
use olp_hook::signal;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> (PathBuf, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_detach_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let (p, h) = (root.join("proj"), root.join("home"));
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::create_dir_all(&h).unwrap();
    (p, h)
}

fn opt_in_to(p: &Path, h: &Path, endpoint: &str) {
    assert!(signal::consent(p, h, "y", "npx").unwrap());
    let path = signal::record_path(p, h);
    let mut r: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    r["endpoint"] = json!(endpoint);
    r["counted_to"] = json!(0);
    fs::write(&path, r.to_string()).unwrap();
}

fn last_sent_day(p: &Path, h: &Path) -> Option<u64> {
    let r: serde_json::Value = serde_json::from_str(&fs::read_to_string(signal::record_path(p, h)).unwrap()).unwrap();
    r["last_sent_day"].as_u64()
}

/// Runs `olp-hook signal --session-start` and returns (elapsed, exit code, stdout, stderr).
fn session_start(p: &Path, h: &Path) -> (Duration, Option<i32>, Vec<u8>, Vec<u8>) {
    let t = Instant::now();
    let o = Command::new(env!("CARGO_BIN_EXE_olp-hook"))
        .args(["signal", "--session-start"])
        .env("CLAUDE_PROJECT_DIR", p)
        .env("HOME", h)
        .env("USERPROFILE", h)
        .env_remove("OLP_HOOK_SIGNAL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    (t.elapsed(), o.status.code(), o.stdout, o.stderr)
}

/// A TCP server on 127.0.0.1 that accepts one connection and never answers.
/// Reports (seconds after start when the first bytes arrived, seconds after start when the client closed).
fn silent_server() -> (u16, mpsc::Receiver<(f64, f64)>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    let start = Instant::now();
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 512];
            let mut first = None;
            loop {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        first.get_or_insert(start.elapsed().as_secs_f64());
                    }
                }
            }
            let _ = tx.send((first.unwrap_or(-1.0), start.elapsed().as_secs_f64()));
        }
    });
    (port, rx)
}

#[test]
fn start_returns_fast_when_server_never_answers() {
    let (p, h) = tmp("slow");
    let (port, rx) = silent_server();
    opt_in_to(&p, &h, &format!("https://127.0.0.1:{port}/v1/olp/signal"));
    let (elapsed, code, out, err) = session_start(&p, &h);
    assert!(elapsed <= Duration::from_millis(500), "session start waited {elapsed:?}");
    assert_eq!(code, Some(0));
    assert!(out.is_empty() && err.is_empty(), "session start prints nothing");
    assert!(last_sent_day(&p, &h).is_some(), "the day is recorded before the attempt");
    // After the hook has exited, curl still connects and sends its first bytes, then gives up on its own at 5 s.
    let (first, closed) = rx.recv_timeout(Duration::from_secs(9)).expect("curl connected after the hook exited");
    assert!(first >= 0.0, "curl sent its first bytes");
    assert!((4.0..=7.0).contains(&closed), "curl gave up at about 5 s (closed at {closed:.1} s)");
}

/// A `curl` that exits at once without reading stdin, so writing the body may hit a closed pipe.
/// Windows resolves `curl` from the executable's own folder before PATH (and before System32), so the hook is copied next to
/// a fake `curl.exe` (a copy of `whoami.exe`, which rejects curl's options and exits). Unix uses a shell script first on PATH.
/// Returns (hook executable to run, PATH value).
fn with_fake_curl(dir: &Path) -> (PathBuf, std::ffi::OsString) {
    fs::create_dir_all(dir).unwrap();
    let path = std::env::var_os("PATH").unwrap_or_default();
    if cfg!(windows) {
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        fs::copy(sys.join("System32").join("whoami.exe"), dir.join("curl.exe")).unwrap();
        let hook = dir.join("olp-hook.exe");
        fs::copy(env!("CARGO_BIN_EXE_olp-hook"), &hook).unwrap();
        (hook, path)
    } else {
        let curl = dir.join("curl");
        fs::write(&curl, "#!/bin/sh\nexit 7\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&curl, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut p = std::ffi::OsString::from(dir.as_os_str());
        p.push(":");
        p.push(&path);
        (PathBuf::from(env!("CARGO_BIN_EXE_olp-hook")), p)
    }
}

#[test]
fn start_is_quiet_and_fast_when_curl_exits_at_once() {
    let (p, h) = tmp("fakecurl");
    opt_in_to(&p, &h, "https://127.0.0.1:9/v1/olp/signal");
    let (hook, path) = with_fake_curl(&p.parent().unwrap().join("fakebin"));
    let t = Instant::now();
    let o = Command::new(&hook)
        .args(["signal", "--session-start"])
        .env("CLAUDE_PROJECT_DIR", &p)
        .env("HOME", &h)
        .env("USERPROFILE", &h)
        .env("PATH", path)
        .env_remove("OLP_HOOK_SIGNAL")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let elapsed = t.elapsed();
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty() && o.stderr.is_empty(), "session start prints nothing: {:?} {:?}", o.stdout, o.stderr);
    assert!(elapsed <= Duration::from_millis(500), "session start waited {elapsed:?}");
    assert!(last_sent_day(&p, &h).is_some(), "the day is recorded before the attempt");
}

#[test]
fn start_returns_fast_when_port_is_closed() {
    let (p, h) = tmp("closed");
    opt_in_to(&p, &h, "https://127.0.0.1:9/v1/olp/signal");
    let (elapsed, code, out, err) = session_start(&p, &h);
    assert!(elapsed <= Duration::from_millis(500), "session start waited {elapsed:?}");
    assert_eq!(code, Some(0));
    assert!(out.is_empty() && err.is_empty());
    assert!(last_sent_day(&p, &h).is_some());
}
