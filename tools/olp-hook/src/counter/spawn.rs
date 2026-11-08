//! Single spawn wrapper of the counter: the only place in this crate that launches a child process.
//! The allowed shapes are a closed list of four: `git status --porcelain`, `git rev-list --count <a>..<b>`,
//! (file classification) `git ls-files -z -- <names>`, `git check-ignore -z --stdin` (names on stdin). Anything else is rejected before launch.
//! Names are only paths walked by the counter (project-relative); never original command text. Pathspecs are literal (GIT_LITERAL_PATHSPECS).
//! The original command (sent by the user or agent) never reaches here; arguments are built only from the `Allowed` enum.
//! No network access or file writes (the `counter_never_calls_external` test scans this file's text).

use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Instant;

/// /// Commands that may be launched (closed list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Allowed {
    GitStatusPorcelain,
    ///     /// Only the `<a>..<b>` shape (each side only ref-name characters).
    GitRevListCount(String),
    ///     /// Tracked names (file classification). At least one name.
    GitLsFiles(Vec<String>),
    ///     /// Ignored names (file classification). Names go on stdin (NUL-separated).
    GitCheckIgnore(Vec<String>),
}

/// /// Name shape accepted for classification: non-empty, no NUL or newline, no leading pathspec magic (`:`) or option (`-`).
pub fn name_ok(s: &str) -> bool {
    !s.is_empty() && !s.starts_with(':') && !s.starts_with('-') && !s.contains(['\0', '\n', '\r'])
}

static CALLS: Mutex<Vec<Vec<String>>> = Mutex::new(Vec::new());

/// /// git sub-arguments of the commands launched so far (in memory only, no log file). Used for test assertions.
pub fn calls() -> Vec<Vec<String>> {
    CALLS.lock().map(|g| g.clone()).unwrap_or_default()
}

fn ref_ok(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.contains("..")
        && !s.ends_with('/')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "._/-~^".contains(c))
}

/// /// Closed-list check. Always run just before launching.
pub fn check_argv(argv: &[&str]) -> bool {
    match argv {
        ["status", "--porcelain"] => true,
        ["rev-list", "--count", range] => match range.split_once("..") {
            Some((a, b)) => ref_ok(a) && ref_ok(b),
            None => false,
        },
        ["ls-files", "-z", "--", names @ ..] => !names.is_empty() && names.iter().all(|n| name_ok(n)),
        ["check-ignore", "-z", "--stdin"] => true,
        _ => false,
    }
}

fn argv_of(a: &Allowed) -> Vec<String> {
    match a {
        Allowed::GitStatusPorcelain => vec!["status".into(), "--porcelain".into()],
        Allowed::GitRevListCount(r) => vec!["rev-list".into(), "--count".into(), r.clone()],
        Allowed::GitLsFiles(names) => ["ls-files", "-z", "--"].iter().map(|s| s.to_string()).chain(names.iter().cloned()).collect(),
        Allowed::GitCheckIgnore(_) => vec!["check-ignore".into(), "-z".into(), "--stdin".into()],
    }
}

/// /// Launches one allowed command in `cwd` and returns stdout. If the limit (`deadline`) is exceeded the child is killed and Err is returned.
pub fn run(cwd: &Path, a: &Allowed, deadline: Instant) -> Result<String, &'static str> {
    let argv = argv_of(a);
    let refs: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    if !check_argv(&refs) {
        return Err("command outside the closed list");
    }
    if Instant::now() >= deadline {
        return Err("time limit reached");
    }
    if let Ok(mut g) = CALLS.lock() {
        g.push(argv.clone());
    }
    let input: Option<Vec<u8>> = match a {
        Allowed::GitCheckIgnore(names) if names.iter().all(|n| name_ok(n)) => Some(names.join("\0").into_bytes()),
        Allowed::GitCheckIgnore(_) => return Err("command outside the closed list"),
        _ => None,
    };
    //     // check-ignore reports "nothing ignored" with exit 1; that is not an error
    let quiet_ok = matches!(a, Allowed::GitCheckIgnore(_));
    //     // --no-optional-locks: keeps status from rewriting the index file. fsmonitor off: no external program execution.
    let mut child = std::process::Command::new("git")
        .arg("--no-optional-locks")
        .args(["-c", "core.fsmonitor=false", "-c", "core.untrackedCache=false"])
        .args(&argv)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        //         // ls-files names are literal (no glob). check-ignore rejects this setting (exit 128), so it is turned off there.
        .env("GIT_LITERAL_PATHSPECS", if matches!(a, Allowed::GitLsFiles(_)) { "1" } else { "0" })
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "could not start git")?;
    if let (Some(bytes), Some(mut sin)) = (input, child.stdin.take()) {
        //         // Separate thread so a full output pipe never blocks. Closed (EOF) once everything is written.
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = sin.write_all(&bytes);
        });
    }
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let s = reader.join().unwrap_or_default();
                return if st.success() || (quiet_ok && st.code() == Some(1)) { Ok(s) } else { Err("git returned an error") };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("time limit reached");
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(_) => {
                let _ = child.kill();
                return Err("could not wait for git");
            }
        }
    }
}
