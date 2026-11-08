//! Integration tests for the hook binary. Tests are written first; test names label the locked behaviour. All file and git operations happen only in throwaway fake repositories under CARGO_TARGET_TMPDIR.
use olp_hook::{classify_cli, guarded, run_hook, GuardFail, HookOutcome};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("olp_hook_tests")
        .join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

/// Counting-time limit and hook limit live in the project config file (no constants in code).
fn set_config(project: &Path, quiet: Option<bool>, counter_ms: u64, budget_ms: u64) {
    fs::create_dir_all(project.join(".olp")).unwrap();
    let mut v = json!({"counter_timeout_ms": counter_ms, "hook_budget_ms": budget_ms});
    if let Some(q) = quiet {
        v["quiet"] = json!(q);
    }
    fs::write(project.join(".olp").join("hook-config.json"), v.to_string()).unwrap();
}

/// Existing tests classify in a mode that never shows a prompt (bypassPermissions), where risky commands are blocked (exit 2). Prompt-mode behaviour is covered via `hook_full`.
fn hook(project: &Path, tool: &str, tool_input: Value) -> HookOutcome {
    hook_full(project, "PreToolUse", tool, tool_input, Some("bypassPermissions"), None)
}

fn hook_full(project: &Path, event: &str, tool: &str, tool_input: Value, mode: Option<&str>, tool_use_id: Option<&str>) -> HookOutcome {
    let mut v = json!({
        "session_id": "s", "transcript_path": "t", "cwd": project.to_string_lossy(),
        "hook_event_name": event, "tool_name": tool, "tool_input": tool_input,
    });
    if let Some(m) = mode {
        v["permission_mode"] = json!(m);
    }
    if let Some(id) = tool_use_id {
        v["tool_use_id"] = json!(id);
    }
    run_hook(v.to_string().as_bytes(), Some(project))
}

fn ask_json(o: &HookOutcome) -> Value {
    serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null)
}

fn bash(project: &Path, cmd: &str) -> HookOutcome {
    hook(project, "Bash", json!({"command": cmd}))
}

fn classify(project: &Path, args: &[&str]) -> Result<String, String> {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    classify_cli(project, &a)
}

fn log_lines(project: &Path) -> Vec<Value> {
    let p = project.join(".olp").join("hook-log.jsonl");
    match fs::read_to_string(p) {
        Ok(s) => s.lines().map(|l| serde_json::from_str(l).unwrap()).collect(),
        Err(_) => vec![],
    }
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null", "-c", "user.email=t@t", "-c", "user.name=t", "-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

/// Fake repo: 2 commits; the remote-tracking ref points at the 2nd commit while local main is reset to the 1st, so a force push would lose exactly 1 commit.
fn fake_repo(name: &str) -> PathBuf {
    let d = tmp(name);
    git(&d, &["init", "-q", "-b", "main"]);
    fs::write(d.join("a.txt"), "1").unwrap();
    git(&d, &["add", "a.txt"]);
    git(&d, &["commit", "-q", "-m", "one"]);
    fs::write(d.join("a.txt"), "2").unwrap();
    git(&d, &["commit", "-q", "-am", "two"]);
    git(&d, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(&d, &["reset", "-q", "--hard", "HEAD~1"]);
    // 2 uncommitted changes (1 modified tracked file + 1 newly added) plus 1 untracked file
    fs::write(d.join("a.txt"), "dirty").unwrap();
    fs::write(d.join("b.txt"), "new").unwrap();
    git(&d, &["add", "b.txt"]);
    fs::write(d.join("untracked.txt"), "u").unwrap();
    d
}

/// Name, size and mtime listing of the whole folder (including .git); identical before and after counting means zero writes.
fn tree_fingerprint(root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().to_string();
            if rel.starts_with(".olp") {
                continue; // the device log folder is written after the hook verdict (outside the counter)
            }
            if m.is_dir() {
                // Folders by name only: NTFS folder mtimes update late in read_dir entries and are flaky (file creation/deletion is caught by the name list).
                out.push(format!("{rel}|dir"));
                stack.push(e.path());
            } else {
                let m = fs::metadata(e.path()).unwrap();
                out.push(format!("{rel}|{}|{:?}", m.len(), m.modified().ok()));
            }
        }
    }
    out.sort();
    out
}

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read_src(rel: &str) -> String {
    fs::read_to_string(src_dir().join(rel)).unwrap()
}

fn all_src_files() -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![src_dir()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------

#[test]
fn hook_ask_human_blocks_with_card() {
    let p = tmp("ask_human");
    set_config(&p, None, 2000, 5000);
    fs::create_dir_all(p.join("victim").join("sub")).unwrap();
    for f in ["victim/1.txt", "victim/2.txt", "victim/sub/3.txt"] {
        fs::write(p.join(f), "x").unwrap();
    }
    let out = bash(&p, "rm -rf victim");
    assert_eq!(out.code, 2, "ASK_HUMAN is blocked by the hook (exit 2)");
    let c = &out.stderr;
    assert!(c.contains("delete files"), "{c}");
    assert!(c.contains("3 files"), "{c}");
    assert!(c.contains("counted on this device, read-only"), "{c}");
    assert!(c.contains("Undo: not confirmed"), "{c}");
    // In a non-prompting mode a block tells the user to change mode; it never hands execution over to the human
    assert!(c.contains("Shift+Tab"), "{c}");
    assert!(!c.contains("Run it again yourself"), "{c}");
    assert!(p.join("victim/sub/3.txt").exists(), "original command was not executed");
}

#[test]
fn no_prompt_mode_block_points_to_a_mode_that_asks() {
    // After a block, an agent may hand `! rm ...` to the human; the reason must say that switching to a prompting mode proceeds as chosen, and must not suggest the human run it directly
    let p = tmp("no_prompt_mode");
    victim3(&p);
    for (mode, shown) in [(Some("bypassPermissions"), "bypassPermissions"), (Some("dontAsk"), "dontAsk"), (None, "unknown"), (Some("x; rm -rf /"), "unknown")] {
        let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), mode, Some("toolu_np"));
        assert_eq!(o.code, 2, "{mode:?}");
        let c = &o.stderr;
        assert!(c.contains(&format!("(permission mode: {shown})")), "{mode:?}: {c}");
        assert!(c.contains("Shift+Tab") && c.contains("do not ask the person to run this command themselves"), "{c}");
        // no echo of the command text (input mode text only if it is a known value)
        assert!(!c.contains("rm -rf") && !c.contains("Run it again yourself"), "{c}");
    }
    assert!(p.join("victim/1.txt").exists());
}

#[test]
fn self_guard_block_keeps_run_it_yourself() {
    // Cases touching the hook itself or .olp stay blocked even after a mode change; the message tells the user to do it directly (the human edits their own settings)
    let p = tmp("self_guard_rerun");
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm .olp/hook-map.json"}), Some("bypassPermissions"), Some("toolu_sg"));
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("Run it again yourself") && !o.stderr.contains("Shift+Tab"), "{}", o.stderr);
}

#[test]
fn hook_error_or_timeout_blocks() {
    let p = tmp("error_timeout");
    // (1) input is not JSON
    let o = run_hook(b"not json", Some(&p));
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("blocked"), "{}", o.stderr);
    // (2) tool_name missing
    let o = run_hook(br#"{"tool_input":{}}"#, Some(&p));
    assert_eq!(o.code, 2);
    // (3) config file is corrupt -> blocked
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::write(p.join(".olp/hook-config.json"), "{broken").unwrap();
    assert_eq!(bash(&p, "ls").code, 2);
    // (4) panic or timeout -> GuardFail (the caller turns it into a block)
    let r: Result<(), GuardFail> = guarded(|| panic!("boom"), Some(Duration::from_millis(500)));
    assert_eq!(r.unwrap_err(), GuardFail::Panic);
    let r: Result<(), GuardFail> = guarded(|| std::thread::sleep(Duration::from_millis(400)), Some(Duration::from_millis(20)));
    assert_eq!(r.unwrap_err(), GuardFail::Timeout);
    // (5) the executable itself: malformed input -> exit 2 (the blocking code for Claude Code)
    let exe = env!("CARGO_BIN_EXE_olp-hook");
    let mut child = Command::new(exe)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"{oops").unwrap();
    }
    let o = child.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn unknown_tool_is_write_until_classified() {
    let p = tmp("unknown_tool");
    let o = hook(&p, "mcp__shop__refund", json!({"amount": 1}));
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("Not classified yet"), "{}", o.stderr);
    assert!(o.stderr.contains("olp-hook classify --tool mcp__shop__refund"), "{}", o.stderr);
    // built-in read tools pass
    assert_eq!(hook(&p, "Read", json!({"file_path": "x"})).code, 0);
}

#[test]
fn unknown_tool_asks_once_then_remembers() {
    let p = tmp("asks_once");
    assert_eq!(hook(&p, "mcp__docs__search", json!({})).code, 2);
    classify(&p, &["--tool", "mcp__docs__search", "read_only"]).unwrap();
    assert_eq!(hook(&p, "mcp__docs__search", json!({"q": "a"})).code, 0, "answer once, remembered");
    // a tool answered as write is still blocked, but "not classified" is not asked again
    classify(&p, &["--tool", "mcp__docs__write", "writes_other"]).unwrap();
    let o = hook(&p, "mcp__docs__write", json!({}));
    assert_eq!(o.code, 2);
    assert!(!o.stderr.contains("Not classified yet"), "{}", o.stderr);
    // built-in write tools and Bash cannot be classified per tool
    assert!(classify(&p, &["--tool", "Write", "read_only"]).is_err());
    assert!(classify(&p, &["--tool", "Bash", "read_only"]).is_err());
    assert!(classify(&p, &["--tool", "PowerShell", "read_only"]).is_err(), "PowerShell tool is never passed wholesale");
    assert_eq!(hook(&p, "PowerShell", json!({"command": "Remove-Item -Recurse x"})).code, 2);
}

#[test]
fn bash_unlisted_command_is_write_until_classified() {
    let p = tmp("bash_unlisted");
    // (b) allowed read list = pass
    for c in ["ls -la", "cat a.txt | head -n 3", "git status", "git log --oneline -5", "grep -r foo .", "find . -name '*.rs'"] {
        assert_eq!(bash(&p, c).code, 0, "{c}");
    }
    // even on the read list, write options do not pass
    assert_eq!(bash(&p, "find . -name x -delete").code, 2);
    assert_eq!(bash(&p, "git diff --output=x.patch").code, 2);
    // (c) outside the list = treated as write, with a one-line classify hint
    let o = bash(&p, "cargo test --release");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("olp-hook classify --bash \"cargo test\" read_only"), "{}", o.stderr);
    classify(&p, &["--bash", "cargo test --release", "read_only"]).unwrap();
    assert_eq!(bash(&p, "cargo test -p x").code, 0, "bundled answer (first word + subcommand)");
    assert_eq!(bash(&p, "cargo build").code, 2, "other bundles are separate");
    // if any part of a chain is unclassified, block
    assert_eq!(bash(&p, "cargo test && cargo publish").code, 2);
}

#[test]
fn device_map_stores_paths_and_kind_only() {
    let p = tmp("device_map");
    classify(&p, &["--bash", "npm run deploy --token=SECRET123 --amount 99000", "writes_other", "--reversible", "--recovery-declared"]).unwrap();
    classify(&p, &["--tool", "mcp__pay__charge", "moves_money"]).unwrap();
    let raw = fs::read_to_string(p.join(".olp/hook-map.json")).unwrap();
    for bad in ["SECRET123", "99000", "--token", "deploy", "--amount"] {
        assert!(!raw.contains(bad), "no argument values in the map: {bad} in {raw}");
    }
    let v: Value = serde_json::from_str(&raw).unwrap();
    let entries = v["entries"].as_object().unwrap();
    assert!(entries.contains_key("bash:npm run"));
    assert!(entries.contains_key("tool:mcp__pay__charge"));
    let allowed = ["effect", "reversible", "recovery", "declared_at", "db_count"];
    for (_, e) in entries {
        for k in e.as_object().unwrap().keys() {
            assert!(allowed.contains(&k.as_str()), "key outside allowed columns: {k}");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = fs::metadata(p.join(".olp/hook-map.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(m, 0o600);
    }
}

#[test]
fn counter_is_read_only() {
    use olp_hook::counter::{self, spawn};
    let p = fake_repo("counter_ro");
    set_config(&p, None, 3000, 6000);
    fs::create_dir_all(p.join("victim")).unwrap();
    fs::write(p.join("victim/v.txt"), "v").unwrap();
    let before = tree_fingerprint(&p);
    // Assertion 1: the original command never runs; assessing a dangerous command leaves target and repo untouched
    for c in ["rm -rf victim", "git reset --hard", "git push --force origin main", "psql -c \"DELETE FROM t WHERE a=1\""] {
        assert_eq!(bash(&p, c).code, 2, "{c}");
    }
    assert!(p.join("victim/v.txt").exists());
    assert_eq!(before, tree_fingerprint(&p), "folder state (including .git) identical before and after counting");
    // Assertion 2: no command outside the allow list; every recorded child process is in the closed shape, anything else is rejected
    for argv in spawn::calls() {
        let a: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
        assert!(spawn::check_argv(&a), "call outside the closed list: {argv:?}");
    }
    assert!(!spawn::check_argv(&["rm", "-rf", "x"]));
    assert!(!spawn::check_argv(&["push", "--force"]));
    assert!(!spawn::check_argv(&["rev-list", "--count", "--all"]));
    assert!(!spawn::check_argv(&["status", "--porcelain", "--ignored"]));
    // Assertion 3: time limit -> unknown
    let big = tmp("counter_big");
    for i in 0..50 {
        fs::write(big.join(format!("{i}.txt")), "x").unwrap();
    }
    let r = counter::count_paths(&big, &["."], std::time::Instant::now());
    assert!(r.is_err(), "count past the limit is unknown");
    set_config(&big, None, 0, 5000);
    let o = bash(&big, "rm -rf .");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("count not checked"), "{}", o.stderr);
}

#[test]
fn counter_never_calls_external() {
    use olp_hook::counter::spawn;
    let p = fake_repo("counter_ext");
    set_config(&p, None, 3000, 6000);
    let before = tree_fingerprint(&p);
    let o = bash(&p, "git push --force origin main");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("1 commit"), "1 commit would be lost: {}", o.stderr);
    let o = bash(&p, "git reset --hard");
    assert!(o.stderr.contains("2 changes"), "2 uncommitted changes: {}", o.stderr);
    assert_eq!(before, tree_fingerprint(&p), "no file writes (including .git)");
    let calls = spawn::calls();
    assert!(calls.iter().any(|c| c.first().map(|s| s == "rev-list").unwrap_or(false)));
    for c in &calls {
        let a: Vec<&str> = c.iter().map(|s| s.as_str()).collect();
        assert!(spawn::check_argv(&a), "{c:?}");
    }
    // Compile-unit check: no network or file writes in counter modules; child processes only in spawn.rs
    let banned_net = ["std::net", "TcpStream", "UdpSocket", "TcpListener", "ToSocketAddrs"];
    let banned_write = ["fs::write", "File::create", "OpenOptions", "remove_file", "remove_dir", "fs::rename", "create_dir", "set_permissions"];
    for f in ["counter/mod.rs", "counter/spawn.rs"] {
        let s = read_src(f);
        for b in banned_net.iter().chain(banned_write.iter()) {
            assert!(!s.contains(b), "{f} contains {b}");
        }
    }
    // counting = counter/spawn.rs only (git); signal = signal/spawn.rs only (curl); PowerShell parsing = ps_ast.rs only; nowhere else
    for f in all_src_files() {
        let s = fs::read_to_string(&f).unwrap();
        let is_counter = f.ends_with(Path::new("counter").join("spawn.rs"));
        let is_signal = f.ends_with(Path::new("signal").join("spawn.rs"));
        let is_ps = f.ends_with(Path::new("ps_ast.rs"));
        if !is_counter && !is_signal && !is_ps {
            assert!(!s.contains("Command::new") && !s.contains("process::Command"), "no child processes outside the wrappers: {f:?}");
        }
    }
    let c = read_src("counter/spawn.rs");
    assert_eq!(c.matches("Command::new(").count(), 1, "exactly one counter spawn wrapper site");
    assert!(c.contains("Command::new(\"git\")") && !c.contains("curl"));
    let s = read_src("signal/spawn.rs");
    assert_eq!(s.matches("Command::new(").count(), 1, "exactly one signal spawn wrapper site");
    assert!(s.contains("Command::new(\"curl\")") && !s.contains("\"git\""));
    // PowerShell parser wrapper: one site, absolute powershell.exe path, fixed script via -EncodedCommand, command text on stdin, no -File, no network, no file writes
    let p = read_src("ps_ast.rs");
    assert_eq!(p.matches("Command::new(").count(), 1, "exactly one parser spawn wrapper site");
    assert!(p.contains("Command::new(exe())") && p.contains(r"System32\WindowsPowerShell\v1.0\powershell.exe"), "absolute path only");
    for need in ["\"-NoProfile\"", "\"-NonInteractive\"", "\"-NoLogo\"", "\"-EncodedCommand\"", "Stdio::piped()"] {
        assert!(p.contains(need), "ps_ast.rs must contain {need}");
    }
    for b in banned_net.iter().chain(banned_write.iter()).chain(["\"-File\"", "\"-Command\"", "curl", "\"git\""].iter()) {
        assert!(!p.contains(b), "ps_ast.rs must not contain {b}");
    }
}

#[test]
fn db_count_is_unknown_or_user_declared() {
    use olp_hook::counter::spawn;
    let p = tmp("db_count");
    set_config(&p, None, 2000, 5000);
    let o = bash(&p, "psql -c \"DELETE FROM orders WHERE id=7\"");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("delete database rows"), "{}", o.stderr);
    assert!(o.stderr.contains("count not checked"), "{}", o.stderr);
    // wrapping does not open DB connections or run commands
    assert!(spawn::calls().iter().all(|c| !c.iter().any(|a| a.contains("psql") || a.contains("DELETE"))));
    assert!(!fs::read_to_string(src_dir().join("counter/spawn.rs")).unwrap().contains("psql"));
    // only counts the user gave for that entry become user_declared
    classify(&p, &["--bash", "psql", "writes_other", "--db-count", "42"]).unwrap();
    let o = bash(&p, "psql -c \"DELETE FROM orders WHERE id=7\"");
    assert!(o.stderr.contains("42 rows (user-given count · not checked by the guard)"), "{}", o.stderr);
    let last = log_lines(&p).into_iter().rfind(|l| l["verdict"] == "blocked").unwrap();
    let c = &last["findings"][0]["count"];
    assert_eq!(c["n"], 42);
    assert_eq!(c["source"], "user_declared", "no bare number; source is recorded with it");
    assert!(c["declared_at"].as_u64().is_some(), "declaration time recorded");
    // no propagation to other command shapes or tools
    let o = bash(&p, "mysql -e \"DELETE FROM orders\"");
    assert!(o.stderr.contains("count not checked") && !o.stderr.contains("42"), "{}", o.stderr);
    // not an integer -> unknown (CLI rejects it; a hand-edited map also reads as unknown)
    assert!(classify(&p, &["--bash", "sqlite3", "writes_other", "--db-count", "4.2"]).is_err());
    let mp = p.join(".olp/hook-map.json");
    let mut v: Value = serde_json::from_str(&fs::read_to_string(&mp).unwrap()).unwrap();
    v["entries"]["bash:psql"]["db_count"]["n"] = json!("42");
    fs::write(&mp, v.to_string()).unwrap();
    let o = bash(&p, "psql -c \"UPDATE t SET a=1 WHERE b=2\"");
    assert!(o.stderr.contains("count not checked") && !o.stderr.contains("42 rows"), "{}", o.stderr);
}

#[test]
fn recovery_unknown_is_not_reversible() {
    let p = tmp("recovery");
    classify(&p, &["--bash", "make release", "writes_other", "--reversible"]).unwrap();
    let o = bash(&p, "make release");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("Undo: not confirmed"), "{}", o.stderr);
    assert!(o.stderr.contains("WRITE_IRREVERSIBLE"), "no 'reversible' claim without a declaration: {}", o.stderr);
    let last = log_lines(&p).into_iter().rfind(|l| l["verdict"] == "blocked").unwrap();
    assert_eq!(last["reversible"], false);
    assert_eq!(last["recovery"], "unknown");
    // with a declaration it says "declared recovery location exists", never "can be undone"
    classify(&p, &["--bash", "make deploy", "writes_other", "--reversible", "--recovery-declared"]).unwrap();
    let o = bash(&p, "make deploy");
    assert!(o.stderr.contains("recovery location declared by you"), "{}", o.stderr);
    assert!(!o.stderr.to_lowercase().contains("can be undone"), "{}", o.stderr);
}

#[test]
fn quiet_mode_is_per_project_opt_in() {
    let a = tmp("quiet_a");
    let b = tmp("quiet_b");
    for p in [&a, &b] {
        classify(p, &["--bash", "make deploy", "writes_other", "--reversible", "--recovery-declared"]).unwrap();
    }
    set_config(&a, Some(true), 2000, 5000);
    set_config(&b, None, 2000, 5000); // default = off
    assert_eq!(bash(&a, "make deploy").code, 0, "only the opted-in project passes quietly");
    assert_eq!(bash(&b, "make deploy").code, 2, "other projects are unaffected");
    let la = log_lines(&a);
    assert!(la.iter().any(|l| l["verdict"] == "quiet_pass"), "passes are still logged");
    // even in quiet mode: dangerous shapes, money and undeclared writes are blocked
    assert_eq!(bash(&a, "rm -rf x").code, 2);
    classify(&a, &["--bash", "make pay", "moves_money", "--reversible", "--recovery-declared"]).unwrap();
    assert_eq!(bash(&a, "make pay").code, 2);
    classify(&a, &["--bash", "make wipe", "writes_other"]).unwrap();
    assert_eq!(bash(&a, "make wipe").code, 2);
    // no global switch via environment variable
    std::env::set_var("OLP_HOOK_QUIET", "1");
    assert_eq!(bash(&b, "make deploy").code, 2);
    std::env::remove_var("OLP_HOOK_QUIET");
}

// Value: protects=the three README payment examples (Stripe, KakaoPay, Toss Payments MCP) once classified as moves_money always show a "moves money" prompt card on each call, quiet mode does not let them through, and neither card nor log contains amounts or card numbers; fails_when=the example classify line does not actually raise a card, payment services are handled differently, or argument values leak; seam=classify --tool + run_hook ask path; no_claim=connecting to payment servers and the engine check_action verdict are out of scope until the server is public.
#[test]
fn payment_tools_from_readme_examples_get_moves_money_card() {
    let p = tmp("pay_examples");
    set_config(&p, Some(true), 3000, 8000);
    for tool in ["mcp__stripe__create_payment_intent", "mcp__kakaopay__ready_payment", "mcp__tosspayments__confirm_payment"] {
        classify(&p, &["--tool", tool, "moves_money"]).unwrap();
        let o = hook_full(&p, "PreToolUse", tool, json!({"amount": 99000, "card_number": "4111111111111111"}), Some("default"), Some("toolu_pay1"));
        assert_eq!(o.code, 0, "{tool}: default mode = prompt, even in quiet mode");
        let v = ask_json(&o);
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask", "{tool}");
        let reason = v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or_default();
        assert!(reason.contains("moves money"), "{tool}: {reason}");
        for bad in ["99000", "4111111111111111"] {
            assert!(!o.stdout.contains(bad) && !o.stderr.contains(bad), "{tool}: no argument values in the card");
        }
    }
    let raw = fs::read_to_string(p.join(".olp/hook-log.jsonl")).unwrap();
    assert!(!raw.contains("99000") && !raw.contains("4111111111111111"), "no argument values in the log");
}

#[test]
fn writes_without_engine_domain_block() {
    let p = tmp("no_engine");
    classify(&p, &["--tool", "mcp__db__insert", "writes_other"]).unwrap();
    classify(&p, &["--tool", "mcp__pay__charge", "moves_money"]).unwrap();
    for (tool, inp) in [("mcp__db__insert", json!({})), ("mcp__pay__charge", json!({})), ("Write", json!({"file_path": "n.txt", "content": "x"})), ("Edit", json!({"file_path": "n.txt"}))] {
        let o = hook(&p, tool, inp);
        assert_eq!(o.code, 2, "{tool}: blocked before the engine write domain");
        assert!(o.stderr.contains("before engine check · no receipt"), "{}", o.stderr);
        let low = o.stderr.to_lowercase();
        for bad in ["receipt no", "receipt #", "receipt id", "receipt_id", "olp check receipt", "olp 확인 영수증"] {
            assert!(!low.contains(bad), "{bad}: {}", o.stderr);
        }
    }
    for l in log_lines(&p) {
        assert!(l.get("receipt_id").is_none() && l.get("receipt").is_none());
    }
}

#[test]
fn hook_log_keeps_blocked_unchecked_and_declined() {
    let p = tmp("log");
    // no limit configured -> cannot count (unchecked)
    fs::create_dir_all(p.join("secret_dir_name")).unwrap();
    bash(&p, "rm -rf secret_dir_name");
    bash(&p, "terraform destroy -auto-approve");
    classify(&p, &["--bash", "terraform destroy", "writes_other"]).unwrap();
    let lines = log_lines(&p);
    let blocked: Vec<&Value> = lines.iter().filter(|l| l["verdict"] == "blocked").collect();
    assert_eq!(blocked.len(), 2, "both blocks logged");
    assert!(blocked.iter().any(|l| l["unchecked"] == true), "unchecked state logged");
    assert!(blocked.iter().all(|l| l["human_choice"] == "not_chosen"), "choices not made are logged");
    assert!(lines.iter().any(|l| l["event"] == "human_choice" && l["human_choice"] == "classified"));
    let raw = fs::read_to_string(p.join(".olp/hook-log.jsonl")).unwrap();
    for bad in ["secret_dir_name", "terraform", "destroy", "rm -rf", "auto-approve", &p.to_string_lossy()] {
        assert!(!raw.contains(bad), "no raw path or command in the log: {bad}");
    }
    let allowed = ["ts", "event", "verdict", "effect", "findings", "reversible", "recovery", "unchecked", "classified", "reasons", "human_choice", "why", "card", "tool_use_id"];
    for l in &lines {
        for k in l.as_object().unwrap().keys() {
            assert!(allowed.contains(&k.as_str()), "key outside logged columns: {k}");
        }
    }
}

#[test]
fn hook_makes_no_network_calls() {
    let banned = ["std::net", "TcpStream", "UdpSocket", "TcpListener", "ToSocketAddrs", "reqwest", "ureq", "hyper", "tokio", "http://", "https://"];
    for f in all_src_files() {
        let s = fs::read_to_string(&f).unwrap();
        for b in banned {
            assert!(!s.contains(b), "{f:?} contains {b}");
        }
    }
    // dependencies: only serde and serde_json and their transitive deps (versions already in the repo)
    let lock = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock")).unwrap();
    let allowed = ["olp-hook", "serde", "serde_core", "serde_derive", "serde_json", "itoa", "memchr", "zmij", "proc-macro2", "quote", "syn", "unicode-ident"];
    for line in lock.lines().filter(|l| l.starts_with("name = ")) {
        let n = line.trim_start_matches("name = ").trim_matches('"');
        assert!(allowed.contains(&n), "dependency outside the allowed list: {n}");
    }
}

/// Deleting a remote branch (`--delete`, `-d`, empty-src refspec) is the same dangerous shape as a force push; counting = unknown.
#[test]
fn git_push_delete_is_force_like() {
    let p = tmp("push_delete");
    classify(&p, &["--bash", "git push", "read_only"]).unwrap();
    assert_eq!(bash(&p, "git push origin main").code, 0, "plain push passes once answered");
    for c in ["git push --delete origin dev", "git push -d origin dev", "git push origin :dev", "git push origin +:dev", "git push origin --delete dev"] {
        let o = bash(&p, c);
        assert_eq!(o.code, 2, "{c}");
        assert!(o.stderr.contains("count not checked (remote branch deletion is not counted)"), "{c}: {}", o.stderr);
    }
}

/// The counting limit applies once per hook run: even with several findings, total counting time <= counter_timeout (plus child cleanup slack).
#[test]
fn counter_deadline_shared_across_findings() {
    use olp_hook::classify::{assess_bash, Limits};
    let p = fake_repo("deadline_shared");
    let limit = Duration::from_millis(150);
    let cmd = ["git reset --hard"; 12].join("; ");
    let map = olp_hook::store::load_map(&p).unwrap();
    let t0 = std::time::Instant::now();
    let a = assess_bash(&cmd, &map, &p, Limits { counter: Some(limit) });
    let took = t0.elapsed();
    assert_eq!(a.findings.len(), 12);
    assert!(took <= limit + Duration::from_millis(150), "total counting {took:?} > limit {limit:?}");
    // within the limit: counted number; past the limit: unknown (the card is not dropped)
    assert!(a.findings.iter().all(|f| f.count.n.is_some() || f.count.why_unknown.is_some()));
}

#[test]
fn dangerous_forms_cannot_be_family_answered() {
    let p = tmp("dangerous");
    fs::write(p.join("f.txt"), "x").unwrap();
    // the first word of a dangerous shape rejects a bundled answer
    for fam in ["rm", "rmdir", "cp", "mv", "olp-hook"] {
        assert!(classify(&p, &["--bash", fam, "read_only"]).is_err(), "{fam}");
    }
    // even with a bundled answer, dangerous shapes do not pass
    classify(&p, &["--bash", "git push", "read_only", "--reversible", "--recovery-declared"]).unwrap();
    classify(&p, &["--bash", "git reset", "read_only"]).unwrap();
    classify(&p, &["--bash", "psql", "read_only"]).unwrap();
    classify(&p, &["--bash", "bash", "read_only"]).unwrap();
    set_config(&p, Some(true), 2000, 5000);
    assert_eq!(bash(&p, "git push origin main").code, 0, "not forced + answer = pass");
    classify(&p, &["--bash", "git checkout", "read_only"]).unwrap();
    assert_eq!(bash(&p, "git checkout dev").code, 0, "branch switch is not a dangerous shape");
    assert_eq!(bash(&p, "git checkout -- .").code, 2, "restoring files = overwrite");
    for c in [
        "git push --force origin main",
        "git push -f origin main",
        "git push origin +main",
        "git push --force-with-lease origin main",
        "git reset --hard HEAD~3",
        "psql -c 'DROP TABLE users'",
        "psql -c 'UPDATE users SET a=1'",
        "bash -c 'rm -rf build'",
        "echo hi > f.txt",
        "cat f.txt | tee g.txt",
        "ls && rm f.txt",
        "sudo rm f.txt",
        "bash -c \"olp-hook classify --bash rm read_only\"",
    ] {
        assert_eq!(bash(&p, c).code, 2, "{c}");
    }
    assert!(p.join("f.txt").exists());
}

// ---- Extension 3: prompt-mode ask, PostToolUse pairing, stats, opt-in signal ----

fn victim3(p: &Path) {
    fs::create_dir_all(p.join("victim")).unwrap();
    for f in ["victim/1.txt", "victim/2.txt", "victim/3.txt"] {
        fs::write(p.join(f), "x").unwrap();
    }
}

// Value: protects=the confirmation prompt reason is one line starting with "[OLP] Before this runs:" (OLP marker first, then the meaning words) and keeps the "before engine check · no receipt" marker inside that line, with Undo only in systemMessage (card emphasis is line order only); fails_when=the marker drops out of the reason or what is being done is pushed back; seam=card::ask_reason; no_claim=no claim about how Claude Code renders the reason text.
#[test]
fn card_reason_leads_with_what_it_does() {
    let p = tmp("ask_reason_order");
    victim3(&p);
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), Some("default"), Some("toolu_order1"));
    let v = ask_json(&o);
    let reason = v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    assert!(reason.starts_with("[OLP] Before this runs: delete files — 3 files"), "{reason}");
    assert!(reason.ends_with(" | before engine check · no receipt"), "{reason}");
    assert!(!reason.contains("Undo:"), "{reason}");
    let msg = v["systemMessage"].as_str().unwrap();
    assert!(msg.starts_with(olp_hook::card::ASK_HEADER) && msg.contains("\nUndo: "), "systemMessage unchanged: {msg}");
}

#[test]
fn hook_ask_when_prompt_mode() {
    let p = tmp("ask_mode");
    victim3(&p);
    // Official docs (glossary, cli-reference): `manual` is an alias of `default`.
    for mode in ["default", "manual", "plan", "acceptEdits"] {
        let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), Some(mode), Some("toolu_ask1"));
        assert_eq!(o.code, 0, "{mode}: prompting mode = exit 0 + ask");
        let v = ask_json(&o);
        let h = &v["hookSpecificOutput"];
        assert_eq!(h["hookEventName"], "PreToolUse");
        assert_eq!(h["permissionDecision"], "ask", "{mode}");
        let reason = h["permissionDecisionReason"].as_str().unwrap();
        assert!(reason.contains("before engine check · no receipt") && !reason.contains('\n'), "one-line card: {reason}");
        let msg = v["systemMessage"].as_str().unwrap();
        assert!(msg.contains("3 files (preview: counted on this device, read-only)"), "{msg}");
        assert!(!msg.contains("Run it again yourself") && !msg.contains("cannot pass"), "no 'run it again' line in ask: {msg}");
        for bad in ["receipt no", "receipt #", "receipt id", "olp check receipt"] {
            assert!(!msg.to_lowercase().contains(bad));
        }
    }
    assert!(p.join("victim/3.txt").exists(), "original command not executed");
    // unclassified commands also go to the prompt (with the classify hint)
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "cargo publish"}), Some("default"), Some("toolu_ask2"));
    assert_eq!(ask_json(&o)["hookSpecificOutput"]["permissionDecision"], "ask");
    assert!(ask_json(&o)["systemMessage"].as_str().unwrap().contains("olp-hook classify --bash \"cargo publish\""));
    // self-protection and hook errors still block (exit 2) in prompt modes
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "olp-hook classify --bash rm read_only"}), Some("default"), None);
    assert_eq!(o.code, 2);
    assert_eq!(run_hook(b"{oops", Some(&p)).code, 2);
    // reads pass without output
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "ls"}), Some("default"), None);
    assert_eq!((o.code, o.stdout.as_str()), (0, ""));
}

#[test]
fn hook_denies_when_mode_skips_prompt() {
    let p = tmp("deny_mode");
    victim3(&p);
    // auto sends hook "ask" to the permission prompt per Claude Code docs, so it is removed from the block list (see test below).
    for mode in [Some("bypassPermissions"), Some("dontAsk"), None, Some("whatever")] {
        let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), mode, Some("toolu_d"));
        assert_eq!(o.code, 2, "{mode:?}: mode where no prompt may appear = blocked");
        assert!(o.stdout.is_empty(), "{mode:?}: ask JSON 0");
        assert!(o.stderr.contains("before engine check · no receipt"), "{}", o.stderr);
    }
    assert!(p.join("victim/1.txt").exists());
}

#[test]
fn classify_refuses_keys_that_are_not_command_names() {
    // Even if the hint is corrected, a person can type by hand: never store `}`, `"1mb"` or block-execution verbs
    let p = tmp("classify_key");
    // (`"1mb"` is allowed because in Bash rules it may be a tool name starting with a digit, like `7z`; the PowerShell side filters numeric expressions separately)
    for k in ["}", "\"{0} files\"", "foreach-object", "where-object x", "iex x", "source env.sh"] {
        let e = classify(&p, &["--bash", k, "read_only"]).unwrap_err();
        assert!(e.contains("is not a command name"), "{k}: {e}");
    }
    assert!(classify(&p, &["--bash", "git check-ignore", "read_only"]).is_ok());
    // real tools starting with a digit can be stored
    assert!(classify(&p, &["--bash", "7z a", "read_only"]).is_ok());
}

#[test]
fn auto_mode_asks_with_the_card() {
    // Desktop app default = auto. Hook "ask" -> permission prompt (the classifier does not decide instead), so a card replaces a block.
    let p = tmp("auto_mode");
    victim3(&p);
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), Some("auto"), Some("toolu_auto"));
    assert_eq!(o.code, 0, "{}", o.stderr);
    let v: serde_json::Value = serde_json::from_str(o.stdout.trim()).unwrap();
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "ask");
    assert!(v["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().starts_with("[OLP] Before this runs: delete files — "));
    assert!(p.join("victim/1.txt").exists());
}

#[test]
fn post_hook_pairs_card_with_execution() {
    let p = tmp("post_pair");
    victim3(&p);
    let pre = |id: &str| hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), Some("default"), Some(id));
    assert_eq!(pre("toolu_A").code, 0);
    assert_eq!(pre("toolu_B").code, 0);
    let post = |id: &str| hook_full(&p, "PostToolUse", "Bash", json!({"command": "rm -rf victim"}), Some("default"), Some(id));
    let o = post("toolu_A");
    assert_eq!((o.code, o.stdout.as_str(), o.stderr.as_str()), (0, "", ""), "PostToolUse only records quietly");
    assert_eq!(post("toolu_Z").code, 0, "ids without a card are not recorded");
    // Post for read tools also passes quietly
    assert_eq!(hook_full(&p, "PostToolUse", "Read", json!({}), Some("default"), Some("toolu_R")).code, 0);
    let lines = log_lines(&p);
    let shown: Vec<&Value> = lines.iter().filter(|l| l["card"] == "card_shown").collect();
    assert_eq!(shown.len(), 2);
    let ran: Vec<&Value> = lines.iter().filter(|l| l["card"] == "executed_after_card").collect();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0]["tool_use_id"], "toolu_A");
    assert!(!lines.iter().any(|l| l["tool_use_id"] == "toolu_Z"));
    let s = olp_hook::stats::stats(&p).unwrap();
    assert!(s.contains("after a card: ran 1 · not run 1"), "{s}");
}

#[test]
fn stats_reports_counts_only() {
    let p = tmp("stats");
    victim3(&p);
    let home = p.join("_home");
    olp_hook::signal::setup(&p, &home, "y").unwrap();
    let share: Value = serde_json::from_str(&fs::read_to_string(olp_hook::signal::record_path(&p, &home)).unwrap()).unwrap();
    let id = share["id"].as_str().unwrap().to_string();
    assert!(id.len() >= 16);
    hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), Some("default"), Some("toolu_S1"));
    hook_full(&p, "PreToolUse", "Bash", json!({"command": "terraform destroy"}), Some("dontAsk"), Some("toolu_S2"));
    classify(&p, &["--bash", "cargo test", "read_only"]).unwrap();
    let fr = fs::read_to_string(p.join(".olp/first-run")).unwrap();
    assert!(!fr.trim().is_empty() && fr.trim().chars().all(|c| c.is_ascii_digit()), "first run = digits (timestamp) only: {fr}");
    let s = olp_hook::stats::stats(&p).unwrap();
    for want in ["first run:", "decisions: 2", "stops (ask or block): 2", "after a card: ran 0 · not run 1", "classify answers: 1"] {
        assert!(s.contains(want), "{want} in {s}");
    }
    for bad in [id.as_str(), "toolu_S1", "toolu_S2", "victim", "terraform", "rm -rf", "cargo", &p.to_string_lossy()] {
        assert!(!s.contains(bad), "no path, raw text or id in stats: {bad}");
    }
    assert!(!fr.contains(&id));
}

#[test]
fn signal_off_by_default_and_sends_nothing() {
    use olp_hook::signal::{self, Plan};
    let p = tmp("signal_off");
    let home = p.join("_home");
    assert!(matches!(signal::plan(&p, &home).unwrap(), Plan::Off(_)), "installed only = off");
    assert!(signal::send(&p, &home).unwrap().contains("not sent"));
    // setup default (empty answer) = no (record lives under the home folder; no share.json inside the repo)
    signal::setup(&p, &home, "").unwrap();
    let share: Value = serde_json::from_str(&fs::read_to_string(signal::record_path(&p, &home)).unwrap()).unwrap();
    assert_eq!(share["opt_in"], false);
    assert!(share.get("id").is_none());
    assert!(!p.join(".olp/share.json").exists());
    assert!(matches!(signal::plan(&p, &home).unwrap(), Plan::Off(_)));
    // even on yes, if the project config clears the endpoint nothing is sent (the bundled endpoint is used when the field is absent; tests always clear it or use a fake endpoint)
    let q = tmp("signal_noaddr");
    let qh = q.join("_home");
    signal::setup(&q, &qh, "yes").unwrap();
    fs::create_dir_all(q.join(".olp")).unwrap();
    fs::write(q.join(".olp/hook-config.json"), json!({"signal_endpoint": ""}).to_string()).unwrap();
    assert!(matches!(signal::plan(&q, &qh).unwrap(), Plan::Off(_)));
    assert!(signal::send(&q, &qh).unwrap().contains("not sent"));
    assert!(signal::spawn::calls().is_empty(), "no signal wrapper calls");
}

// Value: protects=env var OLP_HOOK_SIGNAL=off always beats opt-in and config; fails_when=sends despite off when consent and endpoint are present; seam=signal::plan_with (env decision injected, no env-var races between tests); no_claim=no real transmission.
#[test]
fn signal_env_off_always_wins() {
    use olp_hook::signal::{self, Plan};
    assert!(signal::env_off(Some("off")) && signal::env_off(Some("OFF")) && !signal::env_off(None) && !signal::env_off(Some("on")));
    let p = tmp("signal_env_off");
    let home = p.join("_home");
    signal::setup(&p, &home, "y").unwrap();
    assert!(matches!(signal::plan_with(&p, &home, false).unwrap(), Plan::Ready { .. }), "not off = ready");
    assert!(matches!(signal::plan_with(&p, &home, true).unwrap(), Plan::Off(_)), "off = never sends");
}

// Value: protects=age_days is an integer 0..31 from the first-run time (31 = 31 days or more); fails_when=negative, 32 or more, or an invented value when no timestamp; seam=signal::age_days pure function; no_claim=no clock-tampering handling.
#[test]
fn signal_age_days_from_first_run_clamped() {
    use olp_hook::signal::age_days;
    assert_eq!(age_days(Some(1_000_000), 1_000_000), 0);
    assert_eq!(age_days(Some(0), 7 * 86_400 + 5), 7);
    assert_eq!(age_days(Some(0), 400 * 86_400), 31, "31 = 31 days or more");
    assert_eq!(age_days(Some(2_000), 1_000), 0, "0 when the clock runs backwards");
    assert_eq!(age_days(None, 1_000), 0, "no first-run record = first day");
}

// Value: protects=Post* events other than PostToolUse (e.g. a user copies the same config line to PostToolUseFailure) pass quietly with no log, so the stats stop count is not inflated; fails_when=Post* is treated as an unknown event = block + error_blocked; seam=run_hook event branch; no_claim=unknown Pre* events are still blocked.
#[test]
fn post_like_unknown_event_is_silent() {
    let p = tmp("post_like");
    let before = log_lines(&p).len();
    let o = hook_full(&p, "PostToolUseFailure", "Bash", json!({"command": "rm -rf victim"}), Some("default"), Some("toolu_pf1"));
    assert_eq!(o.code, 0, "Post* = not blocked");
    assert!(o.stdout.is_empty() && o.stderr.is_empty(), "no output");
    assert_eq!(log_lines(&p).len(), before, "no log entry");
    let o = hook_full(&p, "SomethingElse", "Bash", json!({"command": "ls"}), Some("default"), None);
    assert_eq!(o.code, 2, "unknown events outside Post* are still blocked");
}

#[test]
fn signal_payload_is_id_version_usedtoday_only() {
    use olp_hook::signal::{self, Plan};
    let p = tmp("signal_payload");
    let home = p.join("_home");
    signal::setup(&p, &home, "y").unwrap();
    // a different endpoint goes only into the home-folder record (outside the repo); project config can only turn it off
    let rp = signal::record_path(&p, &home);
    let mut rec: Value = serde_json::from_str(&fs::read_to_string(&rp).unwrap()).unwrap();
    rec["endpoint"] = json!("https://signal.example.invalid/v1");
    fs::write(&rp, rec.to_string()).unwrap();
    hook_full(&p, "PreToolUse", "Bash", json!({"command": "ls"}), Some("default"), None);
    let Plan::Ready { argv, body, .. } = signal::plan(&p, &home).unwrap() else { panic!("must be ready") };
    let v: Value = serde_json::from_str(&body).unwrap();
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    // Closed field list v2 = README table = server = signal::FIELDS.
    let mut want: Vec<&str> = signal::FIELDS.to_vec();
    want.sort();
    assert_eq!(keys, want, "body = closed field list v2 only");
    assert!(body.len() <= 1024, "<= 1 KiB limit");
    assert_eq!(v["v"], 2);
    assert_eq!(v["age_days"], 0, "first run today = day 0");
    let share: Value = serde_json::from_str(&fs::read_to_string(&rp).unwrap()).unwrap();
    assert_eq!(v["install_id"], share["id"]);
    let id = v["install_id"].as_str().unwrap();
    let b = id.as_bytes();
    assert!(
        id.len() == 36
            && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-')
            && b[14] == b'4'
            && matches!(b[19], b'8' | b'9' | b'a' | b'b')
            && id.chars().all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c)),
        "UUIDv4 lowercase 36 chars: {id}"
    );
    assert_eq!(v["hook_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(v["used_today"], true, "hook ran after a previous send (consent given today)");
    let a: Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    assert!(signal::spawn::check_argv(&a), "closed args: {argv:?}");
    assert_eq!(a.last().copied(), Some("https://signal.example.invalid/v1"));
    assert!(!body.contains(&p.to_string_lossy().to_string()));
    // outside closed args is rejected
    assert!(!signal::spawn::check_argv(&["-T", "/etc/passwd", "--url", "x"]));
    let mut bad = a.clone();
    let n = bad.len();
    bad[n - 1] = "-o/tmp/x";
    assert!(!signal::spawn::check_argv(&bad), "endpoint that looks like an option is rejected");
    // once per day: nothing prepared if already sent today
    let mut s = share.clone();
    s["last_sent_day"] = json!(olp_hook::store::now() / 86400);
    fs::write(&rp, s.to_string()).unwrap();
    assert!(matches!(signal::plan(&p, &home).unwrap(), Plan::Off(_)));
}

#[test]
fn hook_paths_never_touch_signal_wrapper() {
    // Compile-unit check: only the executable (main.rs) calls the signal module. Hook, counter, card, log and classify modules contain no signal text.
    for f in all_src_files() {
        let rel = f.strip_prefix(src_dir()).unwrap().to_string_lossy().replace('\\', "/");
        if rel.starts_with("signal/") || rel == "main.rs" || rel == "lib.rs" {
            continue;
        }
        let s = fs::read_to_string(&f).unwrap();
        assert!(!s.contains("signal::") && !s.contains("crate::signal") && !s.contains("curl"), "{rel} reaches the signal path");
    }
    let main = read_src("main.rs");
    assert!(main.contains("\"signal\""), "signal is only a human-invoked subcommand");
    // Run: repeated hook / PostToolUse runs make no signal wrapper calls
    let p = tmp("hook_no_signal");
    olp_hook::signal::setup(&p, &p.join("_home"), "y").unwrap();
    for (ev, cmd) in [("PreToolUse", "ls"), ("PreToolUse", "rm -rf x"), ("PostToolUse", "rm -rf x")] {
        hook_full(&p, ev, "Bash", json!({"command": cmd}), Some("default"), Some("toolu_N"));
    }
    assert!(olp_hook::signal::spawn::calls().is_empty());
}

// ---- `cd <dir> && ...` shapes are counted relative to that directory ----
// Value: protects=the most common agent one-liner (change directory then delete) still shows how many files would be lost; fails_when=counts another directory (after ;, || or |) or writes an uncounted result as 0; seam=classify::assess_bash eff_dir; no_claim=directory changing between counting time and execution time (race).

// the counted number is labelled "preview:" (same text as the card_v1 golden)
const COUNTED3: &str = "— 3 files (preview: counted on this device, read-only)";
const CD_UNKNOWN: &str = "count not checked (the command changes folder first)";

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn cd_card(p: &Path, cmd: &str) -> String {
    bash(p, cmd).stderr
}

#[test]
fn cd_and_rm_counts_in_target_folder() {
    let p = tmp("cd_and_rm");
    victim3(&p.join("work"));
    let s = cd_card(&p, "cd work && rm -rf victim");
    assert!(s.contains(COUNTED3), "{s}");
}

#[test]
fn cd_semicolon_rm_stays_unknown() {
    let p = tmp("cd_semi");
    victim3(&p.join("work"));
    let s = cd_card(&p, "cd work; rm -rf victim");
    assert!(s.contains(CD_UNKNOWN), "{s}");
}

#[test]
fn cd_or_rm_stays_unknown() {
    let p = tmp("cd_or");
    victim3(&p.join("work"));
    let s = cd_card(&p, "cd work || rm -rf victim");
    assert!(s.contains(CD_UNKNOWN), "{s}");
}

#[test]
fn cd_pipe_stays_unknown() {
    let p = tmp("cd_pipe");
    victim3(&p.join("work"));
    let s = cd_card(&p, "cd work | rm -rf victim");
    assert!(s.contains(CD_UNKNOWN), "{s}");
}

#[test]
fn cd_missing_folder_is_unknown_not_zero() {
    let p = tmp("cd_missing");
    let s = cd_card(&p, "cd nowhere && rm -rf victim");
    assert!(s.contains("delete files — count not checked ("), "{s}");
    assert!(!s.contains("0 files"), "{s}");
}

#[test]
fn cd_variable_or_tilde_or_dash_is_unknown() {
    let p = tmp("cd_var");
    victim3(&p.join("work"));
    victim3(&p);
    for cmd in [
        "cd $W && rm -rf victim",
        "cd ~ && rm -rf victim",
        "cd - && rm -rf victim",
        "cd work extra && rm -rf victim",
        "builtin cd work && rm -rf victim",
        "command cd work && rm -rf victim",
    ] {
        let s = cd_card(&p, cmd);
        assert!(s.contains(CD_UNKNOWN), "{cmd}: {s}");
    }
}

#[test]
fn cd_chain_two_levels_counts() {
    let p = tmp("cd_two");
    victim3(&p.join("a").join("b"));
    let s = cd_card(&p, "cd a && cd b && rm -rf victim");
    assert!(s.contains(COUNTED3), "{s}");
}

#[test]
fn cd_absolute_path_counts() {
    let p = tmp("cd_abs");
    let w = p.join("work");
    victim3(&w);
    let s = cd_card(&p, &format!("cd \"{}\" && rm -rf victim", slash(&w)));
    assert!(s.contains(COUNTED3), "{s}");
}

#[test]
fn cd_into_olp_is_self_guard() {
    let p = tmp("cd_olp");
    victim3(&p.join(".olp"));
    let s = cd_card(&p, "cd .olp && rm -rf victim");
    assert!(s.contains("This touches the OLP hook itself"), "{s}");
}

#[test]
fn redirect_after_cd_counts_in_target() {
    let p = tmp("cd_redirect");
    fs::create_dir_all(p.join("work")).unwrap();
    fs::write(p.join("work").join("out.txt"), "x").unwrap();
    let s = cd_card(&p, "cd work && echo hi > out.txt");
    assert!(s.contains("— 1 file (preview: counted on this device, read-only)"), "{s}");
}

#[test]
fn known_then_semicolon_becomes_unknown() {
    let p = tmp("cd_known_semi");
    victim3(&p.join("work"));
    let s = cd_card(&p, "cd work && rm -rf victim; rm -rf victim");
    assert!(s.contains(COUNTED3), "{s}");
    assert!(s.contains(CD_UNKNOWN), "{s}");
    for cmd in ["cd work && ls || rm -rf victim", "cd work && ls | rm -rf victim", "cd work && ls\nrm -rf victim", "cd work && ls & rm -rf victim"] {
        let s = cd_card(&p, cmd);
        assert!(s.contains(CD_UNKNOWN), "{cmd:?}: {s}");
    }
}

#[test]
fn cd_dotdot_counts() {
    let p = tmp("cd_dotdot");
    victim3(&p);
    fs::create_dir_all(p.join("a")).unwrap();
    let s = cd_card(&p, "cd a && cd .. && rm -rf victim");
    assert!(s.contains(COUNTED3), "{s}");
}

#[test]
fn cd_other_drive_or_powershell_is_unknown() {
    let p = tmp("cd_drive_ps");
    victim3(&p.join("work"));
    // the PowerShell tool does not follow cd
    let s = hook(&p, "PowerShell", json!({"command": "cd work && rm -rf victim"})).stderr;
    assert!(s.contains(CD_UNKNOWN), "{s}");
    // PowerShell and cmd style directory changes
    for cmd in ["Set-Location work && rm -rf victim", "sl work && rm -rf victim", "cd /d work && rm -rf victim", "pushd work && rm -rf victim", "chdir work && rm -rf victim"] {
        let s = cd_card(&p, cmd);
        assert!(s.contains(CD_UNKNOWN), "{cmd}: {s}");
    }
    // drive-changing shapes (a drive letter different from the current one) are reported as a directory change, not a missing directory
    let here = slash(&p).chars().next().unwrap_or('/').to_ascii_uppercase();
    let other = if here == 'Q' { 'R' } else { 'Q' };
    let s = cd_card(&p, &format!("cd {other}:/ && rm -rf victim"));
    assert!(s.contains(CD_UNKNOWN), "{s}");
    #[cfg(windows)]
    {
        // `/...` without a drive letter (Git Bash style) cannot be mapped to a Windows path, so it is not counted
        let s = cd_card(&p, "cd /tmp && rm -rf victim");
        assert!(s.contains(CD_UNKNOWN), "{s}");
        // even on the same drive, drive-relative shapes (`D:work`) depend on that drive's current directory, so they are not counted
        let s = cd_card(&p, &format!("cd {here}:work && rm -rf victim"));
        assert!(s.contains(CD_UNKNOWN), "{s}");
    }
}

/// Realistic shape (a command form Claude Code 2.1.316 emitted; folder is synthetic): same-drive absolute path + quotes + a pwd in between.
#[test]
fn cd_real_agent_form_counts() {
    let p = tmp("cd_real_form");
    let rec = p.join("demo").join("rec");
    fs::create_dir_all(rec.join("build").join("cache")).unwrap();
    for f in ["build/report-a.txt", "build/report-b.txt", "build/cache/c.txt"] {
        fs::write(rec.join(f), "x").unwrap();
    }
    let cmd = format!("cd \"{}\" && pwd && rm -rf build && ls -la", slash(&rec));
    let s = cd_card(&p, &cmd);
    assert!(s.contains(COUNTED3), "{s}");
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": cmd}), Some("default"), Some("toolu_cd1"));
    let reason = ask_json(&o)["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string();
    assert!(reason.starts_with("[OLP] Before this runs: delete files — 3 files"), "{reason}");
}

// The stderr card in non-prompting modes and the hook error card use the same `[OLP]` marker exactly once (no overlap with "OLP hook:")
#[test]
fn stderr_cards_carry_the_same_tag_once() {
    let p = tmp("stderr_tag");
    victim3(&p);
    let o = hook_full(&p, "PreToolUse", "Bash", json!({"command": "rm -rf victim"}), Some("bypassPermissions"), None);
    assert_eq!(o.code, 2);
    assert!(o.stderr.starts_with("[OLP] "), "{}", o.stderr);
    assert!(!o.stderr.contains("OLP hook:") && o.stderr.matches("[OLP]").count() == 1, "marker once: {}", o.stderr);
    let e = run_hook(b"{oops", Some(&p));
    assert_eq!(e.code, 2);
    assert!(e.stderr.starts_with("[OLP] blocked"), "{}", e.stderr);
}
