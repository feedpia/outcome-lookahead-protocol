//! 0.1.3 ③ `olp-hook replay` (DESIGN_OLP_REPLAY_013 §3·§4·§6·§7, ledger 1223): re-judges recorded Bash/PowerShell commands
//! with the same `decide()` the hook uses. Runs none of the recorded commands, sends nothing, writes nothing in any project; only `<home>/olp/replay-count.json`.
use olp_hook::replay::{replay_cli, ReplayEnv};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);
const NOW: u64 = 1_763_000_000; // fixed clock for the tests (unix seconds)

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_replay_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

fn iso(secs: u64) -> String {
    // test-side helper only: days -> civil date (Howard Hinnant), UTC
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let s = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", s / 3600, s % 3600 / 60, s % 60)
}

static IDS: AtomicUsize = AtomicUsize::new(0);

/// One recorded call; every call gets its own tool_use id, as in real records.
fn line(tool: &str, cmd: &str, cwd: &Path, ts: u64) -> String {
    let id = format!("toolu_{}", IDS.fetch_add(1, Ordering::SeqCst));
    json!({"type": "assistant", "cwd": cwd.to_string_lossy(), "timestamp": iso(ts),
           "message": {"content": [{"type": "tool_use", "id": id, "name": tool, "input": {"command": cmd}}]}}).to_string()
}

struct Setup {
    root: PathBuf,
    dir: PathBuf,
    home: PathBuf,
    proj: PathBuf,
}

fn setup(name: &str, lines: &[String]) -> Setup {
    let root = tmp(name);
    let dir = root.join("projects").join("p1");
    let home = root.join("home");
    let proj = root.join("proj");
    fs::create_dir_all(&dir).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join("build")).unwrap();
    for i in 0..12 {
        fs::write(proj.join("build").join(format!("f{i:02}.txt")), "x").unwrap();
    }
    fs::write(dir.join("s.jsonl"), lines.join("\n") + "\n").unwrap();
    let dir = root.join("projects");
    Setup { root, dir, home, proj }
}

fn env(s: &Setup, claudecode: bool) -> ReplayEnv {
    ReplayEnv { home: s.home.clone(), now: NOW, claudecode, version: "0.1.3".into() }
}

fn run(s: &Setup, extra: &[&str], claudecode: bool) -> String {
    let mut args = vec!["--dir".to_string(), s.dir.to_string_lossy().into_owned()];
    args.extend(extra.iter().map(|a| a.to_string()));
    replay_cli(&args, &env(s, claudecode)).unwrap()
}

fn tree(p: &Path) -> Vec<(String, u64)> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                out.extend(tree(&path));
            } else {
                out.push((path.to_string_lossy().into_owned(), fs::read(&path).map(|b| b.len() as u64).unwrap_or(0)));
            }
        }
    }
    out.sort();
    out
}

#[test]
fn t1_recorded_commands_are_never_run() {
    let s0 = tmp("t1probe");
    let marker = s0.join("REPLAY_MARKER");
    let s = setup("t1", &[line("Bash", &format!("touch {}", marker.display()), &s0, NOW - 60)]);
    let _ = run(&s, &[], false);
    assert!(!marker.exists());
}

#[test]
fn t3_writes_nothing_in_the_project_and_only_the_count_file_at_home() {
    let s = setup("t3", &[line("Bash", "rm -rf build", &PathBuf::new(), NOW - 60)]);
    let s = Setup { root: s.root.clone(), dir: s.dir.clone(), home: s.home.clone(), proj: s.proj.clone() };
    fs::write(s.dir.join("p1").join("s.jsonl"), line("Bash", "rm -rf build", &s.proj, NOW - 60) + "\n").unwrap();
    fs::create_dir_all(s.home.join("olp").join("usage")).unwrap();
    fs::write(s.home.join("olp").join("usage").join("u.json"), "{}").unwrap();
    let before = tree(&s.proj);
    let _ = run(&s, &[], false);
    let _ = run(&s, &[], false);
    assert_eq!(tree(&s.proj), before);
    assert_eq!(fs::read_to_string(s.home.join("olp").join("usage").join("u.json")).unwrap(), "{}");
    let count: Value = serde_json::from_str(&fs::read_to_string(s.home.join("olp").join("replay-count.json")).unwrap()).unwrap();
    assert_eq!(count, json!({"runs": 2}));
}

#[test]
fn t4_numbers_only_by_default_and_show_prints_lines() {
    let s0 = tmp("t4cwd");
    let s = setup("t4", &[line("Bash", "rm -rf build", &s0, NOW - 60), line("Bash", "git push origin main", &s0, NOW - 50)]);
    let card = run(&s, &[], false);
    assert!(!card.contains("rm -rf"), "{card}");
    assert!(!card.contains("git push"), "{card}");
    let shown = run(&s, &["--show", "2"], false);
    assert!(shown.contains("rm -rf build") || shown.contains("git push origin main"), "{shown}");
}

#[test]
fn t5_counts_bash_powershell_and_broken_lines() {
    let c = tmp("t5cwd");
    let mut lines = vec![];
    for cmd in ["ls", "cat a.txt", "git status", "echo hi", "rm -rf build", "ls"] {
        lines.push(line("Bash", cmd, &c, NOW - 60));
    }
    for cmd in ["Get-ChildItem", "Remove-Item -Recurse build", "Get-Content a.txt"] {
        lines.push(line("PowerShell", cmd, &c, NOW - 60));
    }
    lines.push("{ not json".to_string());
    let s = setup("t5", &lines);
    let j: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    assert_eq!(j["occurrences"], 9);
    assert_eq!(j["commands"], 8);
    assert_eq!(j["broken"], 1);
}

#[test]
fn t6_days_and_today_keep_only_recent_lines() {
    let c = tmp("t6cwd");
    let s = setup("t6", &[line("Bash", "ls", &c, NOW - 60), line("Bash", "pwd", &c, NOW - 3 * 86_400), line("Bash", "whoami", &c, NOW - 40 * 86_400)]);
    let all: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    assert_eq!(all["occurrences"], 2); // default 30 days
    let d1: Value = serde_json::from_str(&run(&s, &["--json", "--days", "1"], false)).unwrap();
    assert_eq!(d1["occurrences"], 1);
    let today: Value = serde_json::from_str(&run(&s, &["--json", "--today"], false)).unwrap();
    assert_eq!(today["occurrences"], 1);
    assert!(run(&s, &["--today"], false).lines().next().unwrap().contains("last 24 hours"));
}

#[test]
fn t7_json_report_keys() {
    let c = tmp("t7cwd");
    let s = setup("t7", &[line("Bash", "ls", &c, NOW - 60)]);
    let j: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    let mut keys: Vec<&str> = j.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort();
    assert_eq!(keys, vec!["ask", "ask_unclassified", "block", "broken", "by_kind", "commands", "errors", "notes", "occurrences", "pass", "shown", "skipped", "timeout"]);
}

#[test]
fn t8_inside_claude_code_show_prints_no_command_text() {
    let c = tmp("t8cwd");
    let s = setup("t8", &[line("Bash", "rm -rf build", &c, NOW - 60)]);
    let out = run(&s, &["--show", "2"], true);
    assert!(!out.contains("rm -rf"), "{out}");
    assert!(out.contains("run this in your own terminal to see the commands"), "{out}");
}

#[test]
fn t11_no_records_one_line_and_card_shape() {
    let s = setup("t11", &[]);
    let out = run(&s, &[], false);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.starts_with("No Claude Code records in the last 30 days"), "{out}");
    let c = tmp("t11cwd");
    let s2 = setup("t11b", &[line("Bash", "ls", &c, NOW - 60), line("Bash", "rm -rf build", &s2_proj_placeholder(), NOW - 60)]);
    let card = run(&s2, &[], false);
    assert!(card.lines().count() <= 10, "{card}");
    assert!(card.lines().all(|l| l.chars().count() <= 72), "{card}");
    assert!(card.lines().next().unwrap().contains("OLP 0.1.3"), "{card}");
    assert_eq!(card.lines().last().unwrap(), "OLP sent nothing and ran none of these commands.");
}

// Value: protects=--json notes carry the design §3 differences (① working folder, ② pick, ③ decision) and the broken-settings
// case (hook blocks, replay cannot judge); fails_when=a project with pick/decision on or a broken hook-config.json gives no
// notes line; seam=replay_cli --json; no_claim=the numbers themselves (other tests).
#[test]
fn second_review_c2_notes_for_known_differences() {
    let s = setup("c2", &[]);
    fs::create_dir_all(s.proj.join(".olp")).unwrap();
    fs::write(s.proj.join(".olp").join("hook-config.json"), r#"{"pick": true, "decision": true}"#).unwrap();
    let broken = tmp("c2broken");
    fs::create_dir_all(broken.join(".olp")).unwrap();
    fs::write(broken.join(".olp").join("hook-config.json"), "{not json").unwrap();
    let lines = [line("Bash", "rm -rf build", &s.proj, NOW - 60), line("Bash", "rm -rf build", &broken, NOW - 60)];
    fs::write(s.dir.join("p1").join("s.jsonl"), lines.join("\n") + "\n").unwrap();
    let j: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    let notes: Vec<String> = j["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect();
    let has = |w: &str| notes.iter().any(|n| n.contains(w));
    assert!(has("CLAUDE_PROJECT_DIR"), "{notes:?}");
    assert!(has("1 project has pick on"), "{notes:?}");
    assert!(has("1 project has decision on"), "{notes:?}");
    assert!(has("1 command was in a project whose OLP settings could not be read"), "{notes:?}");
    assert_eq!(j["skipped"], 1, "{j}");
}

// Value: protects=replay never sends (design §7 T2) even when the person opted in to the daily signal and an endpoint is
// set; fails_when=the judge path ever calls the signal module; seam=replay_cli; no_claim=the hook's own SessionStart send.
#[test]
fn second_review_c3_t2_no_network_even_when_opted_in() {
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let s = setup("t2", &[]);
    fs::write(s.dir.join("p1").join("s.jsonl"), line("Bash", "rm -rf build", &s.proj, NOW - 60) + "\n").unwrap();
    let rec = olp_hook::signal::record_path(&s.proj, &s.home);
    fs::create_dir_all(rec.parent().unwrap()).unwrap();
    fs::write(&rec, json!({"opt_in": true, "id": "01234567-89ab-4def-8123-456789abcdef", "endpoint": format!("https://127.0.0.1:{port}/v")}).to_string()).unwrap();
    // the fixture is a real "would send" state for the signal module, so a stray call from replay would reach the port
    assert!(matches!(olp_hook::signal::plan_with(&s.proj, &s.home, false), Ok(olp_hook::signal::Plan::Ready { .. })));
    let _ = run(&s, &[], false);
    let _ = run(&s, &["--json"], false);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    assert!(listener.accept().is_err_and(|e| e.kind() == std::io::ErrorKind::WouldBlock), "replay opened a connection");
}

fn tree_mtime(p: &Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut out = vec![];
    if let Ok(rd) = fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                out.extend(tree_mtime(&path));
            } else {
                out.push((path.to_string_lossy().into_owned(), fs::read(&path).unwrap(), fs::metadata(&path).unwrap().modified().unwrap()));
            }
        }
    }
    out.sort();
    out
}

// Value: protects=design §7 T3′ — project bytes·mtime·count unchanged, a pending file older than 10 minutes is NOT swept
// (the hook shell's pick::sweep must not leak into replay), and a home that cannot be written still gives the card and Ok;
// fails_when=replay writes, touches or sweeps anything in a project, or fails when the count file cannot be written;
// seam=replay_cli; no_claim=the hook path (hook tests).
#[test]
fn second_review_c3_t3_bytes_mtime_old_pending_and_unwritable_home() {
    let s = setup("t3b", &[]);
    fs::write(s.dir.join("p1").join("s.jsonl"), line("Bash", "rm -rf build", &s.proj, NOW - 60) + "\n").unwrap();
    let pending = s.proj.join(".olp").join("pending");
    fs::create_dir_all(&pending).unwrap();
    let old = pending.join("old.json");
    fs::write(&old, "{}").unwrap();
    let f = fs::File::options().write(true).open(&old).unwrap();
    f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
    drop(f);
    let before = tree_mtime(&s.proj);
    std::thread::sleep(std::time::Duration::from_millis(20));
    let _ = run(&s, &[], false);
    assert_eq!(tree_mtime(&s.proj), before);
    assert!(old.exists(), "old pending file was swept");
    // home that cannot be written: <home>/olp is a file, so the count file cannot be created
    let s2 = setup("t3c", &[line("Bash", "ls", &tmp("t3ccwd"), NOW - 60)]);
    fs::write(s2.home.join("olp"), "not a folder").unwrap();
    let mut args = vec!["--dir".to_string(), s2.dir.to_string_lossy().into_owned()];
    let card = replay_cli(&args, &env(&s2, false)).unwrap();
    assert_eq!(card.lines().last().unwrap(), "OLP sent nothing and ran none of these commands.");
    args.push("--json".into());
    assert!(replay_cli(&args, &env(&s2, false)).is_ok());
    assert_eq!(fs::read_to_string(s2.home.join("olp")).unwrap(), "not a folder");
}

// Value: protects=a record file that is not UTF-8 is counted in `broken`, not silently skipped (second review memo);
// fails_when=a non-UTF-8 file disappears from every number; seam=replay_cli --json.
#[test]
fn second_review_memo_non_utf8_file_counts_as_broken() {
    let c = tmp("u8cwd");
    let s = setup("u8", &[line("Bash", "ls", &c, NOW - 60)]);
    fs::write(s.dir.join("p1").join("bad.jsonl"), [0xffu8, 0xfe, 0x00, b'\n']).unwrap();
    let j: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    assert_eq!(j["broken"], 1, "{j}");
    assert_eq!(j["occurrences"], 1, "{j}");
}

fn s2_proj_placeholder() -> PathBuf {
    tmp("t11proj")
}

#[test]
fn ledger_1223_wording_stopped_by_a_rule() {
    let s = setup("w", &[]);
    fs::write(s.dir.join("p1").join("s.jsonl"), line("Bash", "rm -rf build", &s.proj, NOW - 60) + "\n").unwrap();
    // a mode that never prompts makes the recorded delete a block
    let mut l: Value = serde_json::from_str(&line("Bash", "rm -rf build", &s.proj, NOW - 60)).unwrap();
    l["permissionMode"] = json!("bypassPermissions");
    fs::write(s.dir.join("p1").join("s.jsonl"), l.to_string() + "\n").unwrap();
    let card = run(&s, &[], false);
    assert!(card.contains("would have been stopped by a rule"), "{card}");
    assert!(!card.contains("would have blocked"), "{card}");
    let j: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    assert_eq!(j["block"], 1);
}

#[test]
fn review_b_c1_request_copy_lines_and_repeated_ids_are_not_counted_again() {
    // real records also carry `api-request-blob` lines that repeat earlier tool_use blocks with the same id and no cwd;
    // resumed or copied records can repeat the same assistant line. Count each recorded call once.
    let c = tmp("c1cwd");
    let mut v: Value = serde_json::from_str(&line("Bash", "ls", &c, NOW - 60)).unwrap();
    v["message"]["content"][0]["id"] = json!("toolu_A");
    let original = v.to_string();
    let blob = json!({"type": "api-request-blob", "message": {"content": [{"type": "tool_use", "id": "toolu_A", "name": "Bash", "input": {"command": "ls"}}]}}).to_string();
    let s = setup("c1", &[original.clone(), blob.clone(), blob, original]);
    let j: Value = serde_json::from_str(&run(&s, &["--json"], false)).unwrap();
    assert_eq!(j["occurrences"], 1, "{j}");
    assert_eq!(j["skipped"], 0, "{j}");
}
