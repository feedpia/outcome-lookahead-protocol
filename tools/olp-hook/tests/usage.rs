//! Tests for the opt-in daily usage-numbers signal: consent, record location, request body shape, and failure isolation.
// Value: protects=only consenting users are counted (record kept outside the repo), numbers only, at most once a day, sent only to the bundled https endpoint, and failures never affect verdicts or the session; fails_when=sending without consent, via a planted file, or to a foreign address, paths/commands/free text leaking into the body, writing output at session start, or sending twice in one day; seam=signal::plan_with, consent, opt_out, record_path, install::add_start_entry, remove_entries; no_claim=server-side storage and aggregation.
use olp_hook::install::{add_entries, add_start_entry, remove_entries, run, InstallEnv};
use olp_hook::signal::{self, Plan};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// (project, home) pair; both are fake directories.
fn tmp(name: &str) -> (PathBuf, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_usage_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let (p, h) = (root.join("proj"), root.join("home"));
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::create_dir_all(&h).unwrap();
    (p, h)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// Yesterday noon (UTC), a time whose 10-minute window has already closed.
fn yesterday_noon() -> u64 {
    (now() / 86_400 - 1) * 86_400 + 43_200
}

fn record(p: &Path, h: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(signal::record_path(p, h)).unwrap()).unwrap()
}

fn set_record(p: &Path, h: &Path, f: impl FnOnce(&mut Value)) {
    let mut r = record(p, h);
    f(&mut r);
    fs::write(signal::record_path(p, h), r.to_string()).unwrap();
}

/// Opted in, with a target endpoint (record in home, outside the repo) and counting from the start of the log.
fn opt_in_to(p: &Path, h: &Path, endpoint: &str) {
    assert!(signal::consent(p, h, "y", "npx").unwrap(), "y means yes");
    set_record(p, h, |r| {
        r["endpoint"] = json!(endpoint);
        r["counted_to"] = json!(0);
    });
}

fn log(p: &Path, lines: &[Value]) {
    let s: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(p.join(".olp/hook-log.jsonl"), s).unwrap();
}

fn files(n: u64) -> Value {
    json!([{ "kind": "files", "count": { "unit": "file", "source": "counted", "n": n } }])
}

fn ready(p: &Path, h: &Path) -> (Vec<String>, Value) {
    match signal::plan_with(p, h, false).unwrap() {
        Plan::Ready { argv, body, .. } => (argv, serde_json::from_str(&body).unwrap()),
        Plan::Off(why) => panic!("not ready to send: {why}"),
    }
}

fn off(p: &Path, h: &Path) -> &'static str {
    match signal::plan_with(p, h, false).unwrap() {
        Plan::Off(why) => why,
        Plan::Ready { .. } => panic!("must not send"),
    }
}

const FAKE: &str = "https://signal.example.invalid/v1";

// ---- Not opted in -> nothing is sent (no arguments built) ----
#[test]
fn t1_no_consent_sends_nothing() {
    let (p, h) = tmp("t1_none");
    assert_eq!(off(&p, &h), "not opted in", "no record");
    assert!(!signal::consent(&p, &h, "", "npx").unwrap(), "Enter means no");
    assert_eq!(off(&p, &h), "not opted in", "answered no");
    for a in ["n", "no", "ok", "1", "true", " "] {
        let (q, qh) = tmp("t1_word");
        assert!(!signal::consent(&q, &qh, a, "npx").unwrap(), "{a:?} is not a yes");
    }
}

// ---- A file inside the repo is not consent, and project config cannot change the endpoint ----
#[test]
fn c1_planted_project_files_never_opt_in_or_redirect() {
    let (p, h) = tmp("c1_planted");
    // Plant a legacy share.json plus a foreign endpoint
    fs::write(p.join(".olp/share.json"), json!({"opt_in": true, "id": "3f2b9c1e-5a7d-4e0f-9b1c-2d3e4f5a6b7c"}).to_string()).unwrap();
    fs::write(p.join(".olp/hook-config.json"), json!({"signal_endpoint": "https://attacker.example.invalid/x"}).to_string()).unwrap();
    assert_eq!(off(&p, &h), "not opted in", "share.json inside the repo is not consent");
    // Even after the user opts in, a different endpoint in project config is ignored (bundled endpoint wins)
    assert!(signal::consent(&p, &h, "y", "manual").unwrap());
    let (argv, _) = ready(&p, &h);
    let url = argv.last().unwrap();
    assert!(!url.contains("attacker"), "{url}");
    assert_eq!(Some(url.clone()), olp_hook::config::bundled_signal_endpoint());
    // The record lives under the home directory, and the file name contains no project path characters
    let rp = signal::record_path(&p, &h);
    assert!(rp.starts_with(&h) && !rp.to_string_lossy().contains("proj"), "{rp:?}");
}

#[test]
fn c1_project_config_can_only_turn_it_off() {
    let (p, h) = tmp("c1_off");
    opt_in_to(&p, &h, FAKE);
    fs::write(p.join(".olp/hook-config.json"), json!({"signal_endpoint": ""}).to_string()).unwrap();
    assert_eq!(off(&p, &h), "no signal_endpoint set", "empty string means off");
}

// ---- install_id format ----
#[test]
fn f5_install_id_must_be_uuid_v4() {
    for bad in ["../../etc/passwd", "3F2B9C1E-5A7D-4E0F-9B1C-2D3E4F5A6B7C", "3f2b9c1e-5a7d-3e0f-9b1c-2d3e4f5a6b7c", "x", &"a".repeat(2000)] {
        let (p, h) = tmp("f5_id");
        opt_in_to(&p, &h, FAKE);
        set_record(&p, &h, |r| r["id"] = json!(bad));
        assert_eq!(off(&p, &h), "no id", "{bad}");
    }
    let (p, h) = tmp("f5_good");
    opt_in_to(&p, &h, FAKE);
    let (_, b) = ready(&p, &h);
    assert!(signal::id_ok(b["install_id"].as_str().unwrap()));
}

// ---- Non-interactive install: no prompt, no record, no SessionStart entry; --yes is not an option ----
fn run_bin(project: &Path, home: &Path, args: &[&str], stdin: &[u8], path_env: Option<&str>) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_olp-hook"));
    c.args(args).env("CLAUDE_PROJECT_DIR", project).env("HOME", home).env("USERPROFILE", home).env_remove("OLP_HOOK_SIGNAL");
    if let Some(v) = path_env {
        c.env("PATH", v);
    }
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use std::io::Write;
        let _ = child.stdin.take().unwrap().write_all(stdin);
    }
    child.wait_with_output().unwrap()
}

#[test]
fn t2_d2_piped_install_never_opts_in() {
    let (project, home) = tmp("t2_piped");
    let o = run_bin(&project, &home, &["install"], b"y\n", None);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(!signal::asked(&project, &home), "piped y is not consent (no record)");
    let s: Value = serde_json::from_str(&fs::read_to_string(project.join(".claude/settings.json")).unwrap()).unwrap();
    assert!(s["hooks"]["SessionStart"].is_null(), "no SessionStart entry: {s}");
    assert!(!s["hooks"]["PreToolUse"].is_null(), "hook entries are still added");
    // --yes is not an option (no consent path)
    let (project, home) = tmp("t2_yes");
    let o = run_bin(&project, &home, &["install", "--yes"], b"y\n", None);
    assert_eq!(o.status.code(), Some(1));
    assert!(!signal::asked(&project, &home));
    assert!(!project.join(".claude/settings.json").exists(), "nothing changed");
    // No consent path via environment variable
    let (project, home) = tmp("t2_env");
    let mut c = Command::new(env!("CARGO_BIN_EXE_olp-hook"));
    c.args(["install"]).env("CLAUDE_PROJECT_DIR", &project).env("HOME", &home).env("USERPROFILE", &home).env("OLP_HOOK_SIGNAL", "on").stdin(Stdio::null());
    assert_eq!(c.output().unwrap().status.code(), Some(0));
    assert!(!signal::asked(&project, &home), "OLP_HOOK_SIGNAL=on is not consent");
    // Setup without a terminal neither asks nor records a "no"
    let (project, home) = tmp("t2_setup");
    let o = run_bin(&project, &home, &["setup"], b"y\n", None);
    assert_eq!(o.status.code(), Some(0));
    assert!(!signal::asked(&project, &home), "no record");
    assert!(String::from_utf8_lossy(&o.stdout).contains("Not asked"));
}

// ---- Opt-in -> one SessionStart entry, PreToolUse/PostToolUse unchanged ----
const CMD: &str = "C:/Users/me/olp/olp-hook.exe";

#[test]
fn t3_consent_adds_one_session_start_entry_only() {
    let base = add_entries(None, CMD, false).unwrap().text;
    let c = add_start_entry(Some(&base), CMD).unwrap();
    assert!(c.changed);
    let (before, after): (Value, Value) = (serde_json::from_str(&base).unwrap(), serde_json::from_str(&c.text).unwrap());
    assert_eq!(after["hooks"]["PreToolUse"], before["hooks"]["PreToolUse"]);
    assert_eq!(after["hooks"]["PostToolUse"], before["hooks"]["PostToolUse"]);
    assert_eq!(
        after["hooks"]["SessionStart"],
        json!([{ "matcher": "startup", "hooks": [{ "type": "command", "command": format!("{CMD} signal --session-start"), "timeout": 3 }] }])
    );
    assert!(after["hooks"]["SessionStart"][0]["hooks"][0].get("onFailure").is_none(), "failures must not affect the session");
    let again = add_start_entry(Some(&c.text), CMD).unwrap();
    assert!(!again.changed, "second run is a no-op (one entry)");
    // uninstall also removes our SessionStart entry
    let u = remove_entries(&c.text).unwrap();
    let v: Value = serde_json::from_str(&u.text).unwrap();
    assert_eq!(v["hooks"]["SessionStart"], json!([]), "{v}");
}

// A SessionStart entry holding someone else's hook: only our hook is changed
#[test]
fn f2_shared_session_start_entry_keeps_other_hooks() {
    let shared = json!({ "hooks": { "SessionStart": [ { "matcher": "startup", "hooks": [
        { "type": "command", "command": "load-env.sh" },
        { "type": "command", "command": "D:/old/olp/olp-hook.exe signal --session-start", "timeout": 3 }
    ] } ] } })
    .to_string();
    let c = add_start_entry(Some(&shared), CMD).unwrap();
    let v: Value = serde_json::from_str(&c.text).unwrap();
    let hs = &v["hooks"]["SessionStart"][0]["hooks"];
    assert_eq!(hs[0]["command"], "load-env.sh", "foreign hook untouched: {v}");
    assert_eq!(hs[1]["command"], format!("{CMD} signal --session-start"));
    assert_eq!(hs.as_array().unwrap().len(), 2);
}

#[test]
fn t3_install_path_is_npx_only_inside_node_modules() {
    assert_eq!(olp_hook::install::install_path(Path::new("/x/node_modules/@feedpia/olp-win32-x64/bin/olp-hook.exe")), "npx");
    assert_eq!(olp_hook::install::install_path(Path::new("C:/Users/me/olp/olp-hook.exe")), "manual");
}

// ---- Body contains closed fields only, no canary path or command text; values are counted from the log ----
#[test]
fn t4_body_is_closed_list_with_counts_only() {
    let (p, h) = tmp("t4_body");
    opt_in_to(&p, &h, FAKE);
    let t = yesterday_noon();
    log(
        &p,
        &[
            json!({"ts": t, "event": "decision", "verdict": "asked", "card": "card_shown", "findings": files(2), "why": "/canary/secret-path.txt", "cmd": "rm -rf /canary"}),
            json!({"ts": t + 1, "event": "decision", "verdict": "blocked", "findings": files(1)}),
            json!({"ts": t + 2, "event": "decision", "verdict": "kept_by_choice", "findings": files(1)}),
            json!({"ts": t + 3, "event": "decision", "verdict": "error_blocked"}),
            json!({"ts": t + 4, "event": "decision", "verdict": "quiet_pass"}),
            json!({"ts": now() + 5, "event": "decision", "verdict": "asked", "card": "card_shown"}),
        ],
    );
    let (_, b) = ready(&p, &h);
    let mut got: Vec<&str> = b.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    got.sort();
    let mut want: Vec<&str> = signal::FIELDS.to_vec();
    want.sort();
    assert_eq!(got, want, "closed field list v2");
    let text = b.to_string();
    assert!(!text.contains("canary") && !text.contains("rm "), "no path or command text: {text}");
    assert_eq!(b["v"], 2);
    assert_eq!(b["install_path"], "npx");
    assert_eq!(b["cards_shown"], 1, "an entry whose 10-minute window is still open is sent next time");
    assert_eq!(b["blocked"], 3, "blocked·kept_by_choice·error_blocked");
    assert_eq!(b["decision_on"], false);
    assert_eq!(b["decision_asked"], 0);
    assert_eq!(b["after_decision"], json!({"all": 0, "fewer": 0, "none": 0}));
}

#[test]
fn t4_after_decision_counts_the_next_delete_within_ten_minutes() {
    let (p, h) = tmp("t4_after");
    opt_in_to(&p, &h, FAKE);
    fs::write(p.join(".olp/hook-config.json"), json!({"decision": true}).to_string()).unwrap();
    let t = yesterday_noon() - 20_000;
    log(
        &p,
        &[
            // 3 questions -> exactly 600 seconds later 1 = fewer (boundary inclusive, same as decision.rs)
            json!({"ts": t, "event": "decision", "verdict": "decision_asked", "findings": files(3)}),
            json!({"ts": t + 600, "event": "decision", "verdict": "asked", "card": "card_shown", "findings": files(1)}),
            // 2 questions -> the next 2 after a zero-count line = all (first line with count > 0)
            json!({"ts": t + 2000, "event": "decision", "verdict": "decision_asked", "findings": files(2)}),
            json!({"ts": t + 2010, "event": "decision", "verdict": "quiet_pass"}),
            json!({"ts": t + 2030, "event": "decision", "verdict": "asked", "card": "card_shown", "findings": files(2)}),
            // 601 seconds later = none
            json!({"ts": t + 5000, "event": "decision", "verdict": "decision_asked", "findings": files(2)}),
            json!({"ts": t + 5601, "event": "decision", "verdict": "asked", "card": "card_shown", "findings": files(2)}),
        ],
    );
    let (_, b) = ready(&p, &h);
    assert_eq!(b["decision_on"], true);
    assert_eq!(b["decision_asked"], 3);
    assert_eq!(b["after_decision"], json!({"all": 1, "fewer": 1, "none": 1}));
}

// A decision card whose 10-minute window is still open is not frozen as "none" now; it is deferred to the next send
#[test]
fn f6_open_decision_window_waits_for_next_send() {
    let (p, h) = tmp("f6_open");
    opt_in_to(&p, &h, "https://127.0.0.1:9/f6");
    let t = now() - 300;
    log(&p, &[json!({"ts": t, "event": "decision", "verdict": "decision_asked", "findings": files(2)})]);
    let Plan::Ready { body, counted_to, .. } = signal::plan_with(&p, &h, false).unwrap() else { panic!() };
    let b: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(b["decision_asked"], 0, "not counted yet");
    assert!(counted_to <= t, "counted-to = now minus 10 minutes");
    // The next send counts from that point, so this card is included
    let _ = signal::send(&p, &h).unwrap();
    assert_eq!(record(&p, &h)["counted_to"], json!(counted_to));
}

#[test]
fn t4_counts_are_capped_at_10000() {
    let (p, h) = tmp("t4_cap");
    opt_in_to(&p, &h, FAKE);
    let t = yesterday_noon();
    let lines: Vec<Value> = (0..10_001).map(|i| json!({"ts": t + i % 3600, "event": "decision", "verdict": "blocked", "card": "card_shown"})).collect();
    log(&p, &lines);
    let (_, b) = ready(&p, &h);
    assert_eq!(b["cards_shown"], 10_000);
    assert_eq!(b["blocked"], 10_000);
}

// used_today: whether the hook ran after the last send (including today)
#[test]
fn f3_used_today_looks_back_to_the_last_send() {
    let (p, h) = tmp("f3_used");
    opt_in_to(&p, &h, FAKE);
    let yday = now() / 86_400 - 1;
    set_record(&p, &h, |r| r["counted_to"] = json!(yday * 86_400));
    fs::write(p.join(".olp/used-day"), yday.to_string()).unwrap();
    assert_eq!(ready(&p, &h).1["used_today"], true, "ran yesterday and last send was yesterday = true");
    fs::write(p.join(".olp/used-day"), (yday - 3).to_string()).unwrap();
    assert_eq!(ready(&p, &h).1["used_today"], false, "ran only before the last send = false");
}

// ---- Server failure -> exit 0, verdict bytes identical ----
#[test]
fn t5_server_failure_changes_no_verdict() {
    let (p, h) = tmp("t5_fail");
    opt_in_to(&p, &h, "https://127.0.0.1:9/v1/olp/signal");
    let call = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}, "permission_mode": "default"}).to_string();
    let before = olp_hook::run_hook(call.as_bytes(), Some(&p));
    let msg = signal::send(&p, &h).unwrap();
    // Session start no longer waits for curl, so it reports only that the send was started (or why not).
    assert!(msg.starts_with("started") || msg.starts_with("not sent"), "{msg}");
    let after = olp_hook::run_hook(call.as_bytes(), Some(&p));
    assert_eq!((before.code, before.stdout, before.stderr), (after.code, after.stdout, after.stderr));
}

// ---- One-line opt-out, OLP_HOOK_SIGNAL=off, and re-enabling counts from that point ----
#[test]
fn t6_opt_out_and_env_off() {
    let (p, h) = tmp("t6_off");
    opt_in_to(&p, &h, FAKE);
    assert!(matches!(signal::plan_with(&p, &h, true).unwrap(), Plan::Off("OLP_HOOK_SIGNAL=off")));
    signal::opt_out(&p, &h).unwrap();
    assert_eq!(off(&p, &h), "not opted in");
    assert!(record(&p, &h).get("id").is_none(), "turning off also removes the id");
    // Re-enabling (setup) counts from now; records made while off are not sent
    log(&p, &[json!({"ts": yesterday_noon(), "event": "decision", "verdict": "asked", "card": "card_shown"})]);
    signal::setup(&p, &h, "y").unwrap();
    set_record(&p, &h, |r| r["endpoint"] = json!(FAKE));
    assert_eq!(ready(&p, &h).1["cards_shown"], 0);
    // telemetry off binary: consent false, and no SessionStart entry in project settings
    let (project, home) = tmp("t6_cli");
    opt_in_to(&project, &home, FAKE);
    let base = add_entries(None, CMD, false).unwrap().text;
    fs::create_dir_all(project.join(".claude")).unwrap();
    fs::write(project.join(".claude/settings.json"), add_start_entry(Some(&base), CMD).unwrap().text).unwrap();
    let o = run_bin(&project, &home, &["telemetry", "off"], b"", None);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    let v: Value = serde_json::from_str(&fs::read_to_string(project.join(".claude/settings.json")).unwrap()).unwrap();
    assert_eq!(v["hooks"]["SessionStart"], json!([]));
    assert!(!v["hooks"]["PreToolUse"].as_array().unwrap().is_empty(), "hook entries unchanged");
    assert_eq!(off(&project, &home), "not opted in");
}

// Running telemetry off from a subfolder still turns off the record of the installed project
#[test]
fn r3_telemetry_off_from_a_subfolder_finds_the_project() {
    let (project, home) = tmp("r3_sub");
    opt_in_to(&project, &home, FAKE);
    let sub = project.join("src").join("deep");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(signal::find_project(&sub, &home), project);
    let o = run_bin(&sub, &home, &["telemetry", "off"], b"", None);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(off(&project, &home), "not opted in", "installed project record is turned off");
    let (lone, lh) = tmp("r3_none");
    assert_eq!(signal::find_project(&lone, &lh), lone, "with no record, the folder itself");
}

// On opt-in during setup, the SessionStart entry goes where the hook is installed (user settings if it is absent from the project and present there)
#[test]
fn k1_start_entry_goes_where_the_hook_is_installed() {
    let (project, home) = tmp("k1_scope");
    let env = InstallEnv { project: project.clone(), home: home.clone(), exe: project.join("olp-hook"), now: 1 };
    assert!(!olp_hook::install::hook_in_user_settings(&env), "absent everywhere = project");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude/settings.json"), add_entries(None, CMD, false).unwrap().text).unwrap();
    assert!(olp_hook::install::hook_in_user_settings(&env), "only in user settings = user scope");
    fs::create_dir_all(project.join(".claude")).unwrap();
    fs::write(project.join(".claude/settings.json"), add_entries(None, CMD, false).unwrap().text).unwrap();
    assert!(!olp_hook::install::hook_in_user_settings(&env), "present in project = project");
}

// ---- README field table matches the code key list ----
#[test]
fn t8_readme_table_matches_fields() {
    for (readme, head) in [(include_str!("../README.md"), "## Optional daily usage numbers (off by default)"), (include_str!("../README.ko.md"), "## 따로 켜는 하루 사용 숫자 (기본 꺼짐)")] {
        let start = readme.find(head).expect("README section");
        let section = &readme[start..];
        let section = &section[..section[3..].find("\n## ").map(|i| i + 3).unwrap_or(section.len())];
        let mut rows: Vec<&str> = section.lines().filter(|l| l.starts_with("| `")).map(|l| l[3..].split('`').next().unwrap()).collect();
        rows.sort();
        let mut want = signal::FIELDS.to_vec();
        want.sort();
        assert_eq!(rows, want, "README table matches the sent fields");
        for must in ["OLP_HOOK_SIGNAL=off", "telemetry off", "UTC", "after_decision", "not the answer the person chose", "Enter"] {
            assert!(section.contains(must), "README must contain {must}");
        }
    }
}

// ---- Endpoint must be https:// only, no redirects, user curl config ignored, at most 3 seconds ----
#[test]
fn d1_only_https_endpoints_build_arguments() {
    for e in ["http://signal.example.invalid/v1", "ftp://x/y", "HTTPS://x/y", "signal.example.invalid", "//x/y"] {
        let (p, h) = tmp("d1_bad");
        opt_in_to(&p, &h, e);
        assert_eq!(off(&p, &h), "signal_endpoint not accepted", "{e}");
    }
}

#[test]
fn d1_arguments_ignore_curlrc_and_redirects_with_a_short_cap() {
    let (p, h) = tmp("d1_args");
    opt_in_to(&p, &h, FAKE);
    let (argv, _) = ready(&p, &h);
    assert_eq!(argv[0], "-q", "first argument -q means the user .curlrc is not read");
    let a = argv.join(" ");
    assert!(a.contains("--max-redirs 0") && a.contains("--max-time 5") && a.contains("--proto =https"), "{a}");
    assert!(!argv.iter().any(|x| x == "-L" || x.starts_with("--location")), "{a}");
    let r = |t: u64| {
        let v = signal::spawn::argv("https://x/y", t);
        signal::spawn::check_argv(&v.iter().map(|s| s.as_str()).collect::<Vec<_>>())
    };
    assert!((1..=5).all(r) && !r(6) && !r(0), "time limit allowed only at 1-5 seconds (SessionStart itself uses the bundled 5 s, checked above)");
    // A time limit from project config is not used
    fs::write(p.join(".olp/hook-config.json"), json!({"signal_timeout_s": 3}).to_string()).unwrap();
    assert!(ready(&p, &h).0.join(" ").contains("--max-time 5"));
    let mut no_q = signal::spawn::argv("https://x/y", 2);
    no_q.remove(0);
    assert!(!signal::spawn::check_argv(&no_q.iter().map(|s| s.as_str()).collect::<Vec<_>>()), "rejected without -q");
}

#[test]
fn d1_bundled_endpoint_is_https() {
    let (p, _) = tmp("d1_bundled");
    let cfg = olp_hook::config::load(&p).unwrap();
    assert!(olp_hook::config::bundled_signal_endpoint().is_some_and(|e| e.starts_with("https://")), "bundled endpoint");
    // Session start does not wait for curl (tests/signal_detach.rs), so curl may use the full 5 s.
    assert_eq!(cfg.signal_timeout_s, Some(5), "curl 5s, session start does not wait for it");
}

// ---- Never holds up the session: second run on the same day = no spawn, no binary output, exit 0 ----
#[test]
fn d3_second_run_same_day_spawns_nothing() {
    let (p, h) = tmp("d3_twice");
    let ep = format!("https://127.0.0.1:9/d3/{}", SEQ.fetch_add(1, Ordering::SeqCst));
    opt_in_to(&p, &h, &ep);
    let mine = || signal::spawn::calls().iter().filter(|c| c.contains(&ep)).count();
    let _ = signal::send(&p, &h).unwrap();
    assert_eq!(mine(), 1, "first run attempts once (even if it fails)");
    assert_eq!(off(&p, &h), "already sent today", "no retry");
    let _ = signal::send(&p, &h).unwrap();
    assert_eq!(mine(), 1, "second run does not spawn");
}

#[test]
fn d3_session_start_prints_nothing_and_exits_zero() {
    let (project, home) = tmp("d3_bin");
    opt_in_to(&project, &home, "https://127.0.0.1:9/v1/olp/signal");
    for _ in 0..2 {
        let t0 = std::time::Instant::now();
        let o = run_bin(&project, &home, &["signal", "--session-start"], b"{\"hook_event_name\":\"SessionStart\"}", None);
        assert_eq!(o.status.code(), Some(0));
        assert!(o.stdout.is_empty() && o.stderr.is_empty(), "no output (SessionStart output enters the model context)");
        assert!(t0.elapsed() < std::time::Duration::from_secs(3), "at most 3 seconds");
    }
    // No output and exit 0 even without consent, with broken config, or without curl (empty PATH)
    let (project, home) = tmp("d3_broken");
    fs::write(project.join(".olp/hook-config.json"), "{broken").unwrap();
    let o = run_bin(&project, &home, &["signal", "--session-start"], b"", None);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    let (project, home) = tmp("d3_nocurl");
    opt_in_to(&project, &home, "https://127.0.0.1:9/v1/olp/signal");
    let o = run_bin(&project, &home, &["signal", "--session-start"], b"", Some(""));
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
}

#[test]
fn install_dry_run_still_works_with_user_flag() {
    // Regression: run's option list is unchanged (no new options)
    let (project, home) = tmp("dry");
    let env = InstallEnv { project: project.clone(), home, exe: project.join("olp-hook"), now: 1 };
    assert!(run("install", &["--dry-run".into(), "--user".into()], &env).is_ok());
    assert!(run("install", &["--yes".into()], &env).is_err());
}

// ---- B2 (0.1.1): a slow server is not reported as "not sent"; session start stays within its slot ----
/// A local server that accepts the connection and never answers (a cold server that is still waking up).
fn silent_server() -> (std::net::TcpListener, String) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let ep = format!("https://127.0.0.1:{}/v1/olp/signal", l.local_addr().unwrap().port());
    (l, ep)
}

#[test]
fn b2_signal_by_hand_without_a_finished_connection_says_not_sent() {
    // The local server accepts TCP but never finishes the TLS handshake, so the request body is never sent.
    // curl stops at its time limit (exit 28), yet "it may still have arrived" would be false here.
    let (_l, ep) = silent_server();
    let (project, home) = tmp("b2_hand");
    opt_in_to(&project, &home, &ep);
    let o = run_bin(&project, &home, &["signal"], b"", None);
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(out.starts_with("not sent: no connection to the server within 5 s"), "{out}");
    assert!(!out.contains("may still have arrived"), "{out}");
}

#[test]
fn b2_session_start_on_a_slow_server_stays_quiet_and_short() {
    let (_l, ep) = silent_server();
    let (project, home) = tmp("b2_start");
    opt_in_to(&project, &home, &ep);
    let t0 = std::time::Instant::now();
    let o = run_bin(&project, &home, &["signal", "--session-start"], b"", None);
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty() && o.stderr.is_empty());
    assert!(t0.elapsed() < std::time::Duration::from_secs(3), "SessionStart slot is 3 s: {:?}", t0.elapsed());
}

#[test]
fn b2_failure_text_follows_the_curl_exit_code() {
    use signal::spawn::failure_text;
    // Timeout after the connection was set up (the request went out): it may still have arrived.
    assert_eq!(failure_text(Some(28), 5, true), "no answer from the server within 5 s. It may still have arrived; no retry today.");
    // Timeout before the connection was set up (Git for Windows curl takes about 2 s to give up on a closed port): nothing went out.
    assert_eq!(failure_text(Some(28), 2, false), "not sent: no connection to the server within 2 s");
    for reached in [true, false] {
        assert_eq!(failure_text(Some(6), 5, reached), "not sent: could not reach the server");
        assert_eq!(failure_text(Some(7), 5, reached), "not sent: could not reach the server");
        assert_eq!(failure_text(Some(22), 5, reached), "not sent: the server answered with an error");
        assert_eq!(failure_text(Some(35), 5, reached), "not sent: curl returned an error");
        assert_eq!(failure_text(None, 5, reached), "not sent: curl returned an error");
    }
}
