//! olp-hook executable: only reads stdin and sets the exit code. The decision lives in the lib (`olp_hook`).
//! A panic also becomes exit 2 (block) — Claude Code treats exit codes other than 0 and 2 as non-blocking errors and runs the tool anyway.
//! Subcommands run by a person: classify · stats · setup · signal (the opt-in signal is reachable only through this subcommand, never from the hook path).

use std::io::{BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;

const HELP: &str = "olp-hook — Outcome Lookahead Protocol (OLP) device hook
Checks an agent's tool call before it runs and shows what it would remove. Runs on this device only.

With no command, it reads one Claude Code hook call on standard input (PreToolUse or PostToolUse).

Commands:
  install    copy olp-hook to <home>/olp and add the hook to .claude/settings.json (--user, --dry-run, --replace)
  uninstall  remove only the olp-hook lines from that settings file (--user, --dry-run)
  classify   remember what a tool or command does, for this project
  stats      your numbers on this device
  watch      show the latest card next to the prompt (--html for a page)
  setup      optional daily usage numbers (off by default) and .git/info/exclude
  signal     send those numbers now, if you said yes
  telemetry  telemetry off: stop sending them
  help       this text";

/// Consent question (exact text from the usage-signal design). Enter = no.
const USAGE_QUESTION: &str = "Send anonymous daily usage numbers to FeedPia? (counts only, no commands or file names) [y/N] ";

/// Asks and reads the answer only on an interactive terminal (no way to answer yes via pipe, option, environment variable or setting). None if not a terminal.
fn ask_usage() -> Option<String> {
    if !std::io::stdin().is_terminal() {
        return None;
    }
    discard_typed_ahead();
    eprint!("{USAGE_QUESTION}");
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
    Some(line)
}

/// Drops keys typed before the question is on screen (for example an Enter pressed while npx was still downloading),
/// so only an answer typed after the question counts. Failure is ignored: the question is still asked as before.
#[cfg(windows)]
fn discard_typed_ahead() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn FlushConsoleInputBuffer(input: *mut std::ffi::c_void) -> i32;
    }
    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    // SAFETY: both calls take and return plain handles. GetStdHandle returns INVALID_HANDLE_VALUE on failure (null only when no handle is set); either, or a non-console handle, only makes the flush fail.
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        if !input.is_null() {
            FlushConsoleInputBuffer(input);
        }
    }
}

#[cfg(unix)]
fn discard_typed_ahead() {
    extern "C" {
        fn tcflush(fd: i32, queue: i32) -> i32;
    }
    const TCIFLUSH: i32 = if cfg!(any(target_os = "linux", target_os = "android")) { 0 } else { 1 };
    // SAFETY: fd 0 is standard input (checked to be a terminal by the caller); a failure only means nothing was flushed.
    unsafe {
        tcflush(0, TCIFLUSH);
    }
}

#[cfg(not(any(windows, unix)))]
fn discard_typed_ahead() {}

/// Home folder (consent records go only under `<home>/olp/usage/`, outside the repository).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Consent at the end of install: on yes, a home record (including install route) plus one SessionStart entry; on no, only a "no" in the home record.
fn install_consent(env: &olp_hook::install::InstallEnv, user: bool, answer: &str) -> Result<String, String> {
    if !olp_hook::signal::consent(&env.project, &env.home, answer, olp_hook::install::install_path(&env.exe))? {
        return Ok("Usage numbers off. Nothing will be sent.
Turn on later: npx @feedpia/olp setup".to_string());
    }
    let added = olp_hook::install::edit_start(env, user, true)?;
    Ok(format!("{added}\nUsage numbers on (the fields are listed in the README). Turn off: npx @feedpia/olp telemetry off"))
}

fn project_dir() -> PathBuf {
    match std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from).or_else(|| std::env::current_dir().ok()) {
        Some(p) => p,
        None => {
            eprintln!("could not find the project folder");
            std::process::exit(1);
        }
    }
}

fn finish(r: Result<String, String>) -> ! {
    match r {
        Ok(m) => {
            println!("{m}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}

/// Redraws the stats one-screen view for about 1 second. Only two control sequences: cursor up n lines, then erase below.
fn play(frames: &[String]) {
    let mut out = std::io::stdout().lock();
    let mut prev = 0;
    for f in frames {
        if prev > 0 {
            let _ = write!(out, "\x1b[{prev}A\x1b[J");
        }
        let _ = writeln!(out, "{f}");
        let _ = out.flush();
        prev = f.lines().count();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = writeln!(out);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("classify") => finish(olp_hook::classify_cli(&project_dir(), &args[1..])),
        Some("stats") => {
            let p = project_dir();
            let plain = args.iter().any(|a| a == "--plain");
            if olp_hook::stats::animate_ok(std::io::stdout().is_terminal(), std::env::var_os("NO_COLOR").is_some(), std::env::var("TERM").ok().as_deref(), plain) {
                play(&olp_hook::stats::frames(&olp_hook::stats::week(&olp_hook::store::read_log(&p), olp_hook::store::now()), 20));
            }
            finish(olp_hook::stats::stats(&p))
        }
        Some(c @ ("install" | "uninstall")) => {
            let env = olp_hook::install::InstallEnv::from_process(&project_dir()).unwrap_or_else(|e| finish(Err(e)));
            let r = olp_hook::install::run(c, &args[1..], &env);
            // Ask for consent only after install has completed, on an actual change (not --dry-run), and when not yet asked for this project.
            let fresh = c == "install" && r.is_ok() && !args.iter().any(|a| a == "--dry-run") && !olp_hook::signal::asked(&env.project, &env.home);
            match (fresh, r) {
                (true, Ok(m)) => {
                    println!("{m}");
                    let Some(answer) = ask_usage() else { std::process::exit(0) };
                    finish(install_consent(&env, args.iter().any(|a| a == "--user"), &answer))
                }
                (_, r) => finish(r),
            }
        }
        Some("setup") => {
            let p = project_dir();
            let home = home_dir().unwrap_or_else(|| finish(Err("could not find your home folder".to_string())));
            // If not a terminal, neither ask nor record (never write a permanent "no").
            let r = match ask_usage() {
                // On yes, add the SessionStart entry again too (so signals send automatically when re-enabled after telemetry off).
                Some(line) => olp_hook::signal::setup(&p, &home, &line).and_then(|m| {
                    if !olp_hook::signal::opted_in(&p, &home) {
                        return Ok(m);
                    }
                    let env = olp_hook::install::InstallEnv::from_process(&p)?;
                    // Add it where the hook is installed (the user settings for a --user install).
                    olp_hook::install::edit_start(&env, olp_hook::install::hook_in_user_settings(&env), true).map(|a| format!("{m}\n{a}"))
                }),
                None => Ok("Not asked (no terminal): run `olp-hook setup` in a terminal to answer the usage-numbers question.".to_string()),
            };
            let note = olp_hook::watch::ensure_git_exclude(&p);
            finish(r.map(|m| format!("{m}\n{note}")))
        }
        Some("watch") => finish(olp_hook::watch::run(&project_dir(), &args[1..], &olp_hook::watch::Env::from_process())),
        // SessionStart entry (present only for consenting users): always no output and exit 0 (SessionStart output enters Claude's context; failure never affects the session).
        Some("signal") if args.get(1).map(|s| s.as_str()) == Some("--session-start") => {
            // Do not leave the panic text on stderr either.
            std::panic::set_hook(Box::new(|_| {}));
            if let (Some(p), Some(h)) = (std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from).or_else(|| std::env::current_dir().ok()), home_dir()) {
                let _ = std::panic::catch_unwind(|| olp_hook::signal::send(&p, &h));
            }
            std::process::exit(0)
        }
        Some("signal") => {
            let home = home_dir().unwrap_or_else(|| finish(Err("could not find your home folder".to_string())));
            finish(olp_hook::signal::send_by_hand(&project_dir(), &home))
        }
        Some("telemetry") => {
            if args[1..] != ["off"] {
                finish(Err("usage: olp-hook telemetry off".to_string()));
            }
            let home = home_dir().unwrap_or_else(|| finish(Err("could not find your home folder".to_string())));
            let project = olp_hook::signal::find_project(&project_dir(), &home);
            let env = olp_hook::install::InstallEnv::from_process(&project).unwrap_or_else(|e| finish(Err(e)));
            println!("project: {}", project.display());
            let r = olp_hook::signal::opt_out(&env.project, &env.home)
                .and_then(|_| olp_hook::install::edit_start(&env, false, false))
                .and_then(|a| olp_hook::install::edit_start(&env, true, false).map(|b| format!("{a}\n{b}\nUsage numbers off. Nothing will be sent.")));
            finish(r)
        }
        // Help is handled before reading stdin (first line = full name). Any other unknown argument falls through to hook mode below = block, unchanged.
        Some("--help" | "-h" | "help") => finish(Ok(HELP.to_string())),
        _ => {}
    }
    let mut buf = Vec::new();
    if std::io::stdin().read_to_end(&mut buf).is_err() {
        eprintln!("{}", olp_hook::card::error_card("could not read the tool call"));
        std::process::exit(olp_hook::hook::EXIT_BLOCK);
    }
    let out = std::panic::catch_unwind(|| olp_hook::run_hook(&buf, None)).unwrap_or_else(|_| olp_hook::HookOutcome {
        code: olp_hook::hook::EXIT_BLOCK,
        stderr: olp_hook::card::error_card("internal error"),
        stdout: String::new(),
    });
    if !out.stdout.is_empty() {
        println!("{}", out.stdout);
    }
    if !out.stderr.is_empty() {
        eprintln!("{}", out.stderr);
    }
    std::process::exit(out.code);
}
