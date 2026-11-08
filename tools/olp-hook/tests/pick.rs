//! Hook-side tests for the pick flow: the user picks on screen which files to keep, and the hook compares the agent's next delete against that choice.
//! All file work happens in temp folders under CARGO_TARGET_TMPDIR. The GUI server is not started; only the shapes of its files (gui.alive, choice) are written by hand.
// Value: protects=after the user picks which files to keep on screen, the agent's next delete is compared with that pick and shown in one line in the permission dialog, and the verdict always stays with the user (ask); fails_when=choice replaces the permission (allow), names leak into the persistent log or outside the project, or a stale pick or dead server yields a "matches" line; why_new=pick flow in the GUI; seam=hook::run_hook pick branch; no_claim=no guarantee that a human wrote choice/alive (an agent on the same PC could write them).
use olp_hook::{run_hook, HookOutcome};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_pick_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// Pick mode on (`pick: true`) plus the count limit. `alive` = Some(seconds ago) sets gui.alive to that time.
fn setup(p: &Path, pick: bool, alive_ago_s: Option<u64>) {
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::write(p.join(".olp/hook-config.json"), json!({"counter_timeout_ms": 3000, "hook_budget_ms": 8000, "pick": pick}).to_string()).unwrap();
    if let Some(ago) = alive_ago_s {
        let f = p.join(".olp/gui.alive");
        fs::write(&f, json!({"v": 1, "at": now() - ago, "port": 1}).to_string()).unwrap();
        set_mtime(&f, ago);
    }
}

fn set_mtime(f: &Path, ago_s: u64) {
    let t = SystemTime::now() - Duration::from_secs(ago_s);
    fs::OpenOptions::new().write(true).open(f).unwrap().set_modified(t).unwrap();
}

/// Files to delete: build/report-a.txt, build/report-b.txt, build/cache/c.txt
fn build3(p: &Path) {
    fs::create_dir_all(p.join("build/cache")).unwrap();
    for f in ["build/report-a.txt", "build/report-b.txt", "build/cache/c.txt"] {
        fs::write(p.join(f), "x").unwrap();
    }
}

fn pre(p: &Path, cmd: &str, mode: &str, id: &str) -> HookOutcome {
    let v = json!({
        "session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(),
        "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": cmd},
        "permission_mode": mode, "tool_use_id": id,
    });
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn out_json(o: &HookOutcome) -> Value {
    serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null)
}

fn reason(o: &HookOutcome) -> String {
    out_json(o)["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap_or("").to_string()
}

/// Pick guidance goes only in systemMessage; the permission-dialog reason carries only count, names and comparison.
fn message(o: &HookOutcome) -> String {
    out_json(o)["systemMessage"].as_str().unwrap_or("").to_string()
}

fn decision(o: &HookOutcome) -> String {
    out_json(o)["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("").to_string()
}

fn pending(p: &Path, id: &str) -> Option<Value> {
    fs::read_to_string(p.join(format!(".olp/pending/{id}.json"))).ok().map(|s| serde_json::from_str(&s).unwrap())
}

fn pending_names(v: &Value) -> Vec<String> {
    v["files"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap().to_string()).collect()
}

/// Same shape the server writes: keep = list of indexes to keep.
fn write_choice(p: &Path, id: &str, keep: &[usize], at: u64) {
    fs::write(p.join(format!(".olp/pending/{id}.choice.json")), json!({"v": 1, "id": id, "keep": keep, "at": at}).to_string()).unwrap();
}

/// Pick which files to keep by index-to-name comparison (does not rely on file order).
fn keep_idx(v: &Value, keep_names: &[&str]) -> Vec<usize> {
    v["files"].as_array().unwrap().iter().filter(|f| keep_names.contains(&f["name"].as_str().unwrap())).map(|f| f["index"].as_u64().unwrap() as usize).collect()
}

const PICK_LINE: &str = "Mark Keep or Delete in the GUI window";

// ---- pick flow: basic cases ----

#[test]
fn pick_off_or_gui_dead_is_v1() {
    for (pick, alive) in [(false, Some(0)), (true, None)] {
        let p = tmp("pick_off");
        setup(&p, pick, alive);
        build3(&p);
        let o = pre(&p, "rm -rf build", "default", "toolu_off1");
        assert_eq!(decision(&o), "ask");
        assert!(!reason(&o).contains(PICK_LINE), "{}", reason(&o));
        assert!(pending(&p, "toolu_off1").is_none());
    }
}

#[test]
fn pending_written_with_repo_relative_names() {
    let p = tmp("pending_written");
    setup(&p, true, Some(0));
    build3(&p);
    let o = pre(&p, "rm -rf build", "default", "toolu_pw1");
    assert_eq!(decision(&o), "ask");
    assert!(message(&o).contains(PICK_LINE), "{}", message(&o));
    assert!(!reason(&o).contains(PICK_LINE), "{}", reason(&o));
    let v = pending(&p, "toolu_pw1").expect("pending file");
    assert_eq!(v["v"], 1);
    assert_eq!(v["id"], "toolu_pw1");
    assert!(v["created"].as_u64().unwrap() + 5 >= now());
    assert_eq!(v["more_n"], 0);
    let mut names = pending_names(&v);
    names.sort();
    assert_eq!(names, ["build/cache/c.txt", "build/report-a.txt", "build/report-b.txt"]);
    for (i, f) in v["files"].as_array().unwrap().iter().enumerate() {
        assert_eq!(f["index"], i);
        assert!(["tracked", "ignored", "recent", "untracked", "unreadable"].contains(&f["nature"].as_str().unwrap()));
    }
}

#[test]
fn narrowed_command_ask_shows_match_n() {
    let p = tmp("narrowed");
    setup(&p, true, Some(0));
    build3(&p);
    pre(&p, "rm -rf build", "default", "toolu_n1");
    let v = pending(&p, "toolu_n1").unwrap();
    write_choice(&p, "toolu_n1", &keep_idx(&v, &["build/report-a.txt"]), now());
    // The user kept report-a, then the agent narrows the command (different tool_use_id); the comparison uses the file set, not the id.
    let o = pre(&p, "rm -rf build/report-b.txt build/cache", "default", "toolu_n2");
    assert_eq!(decision(&o), "ask");
    assert!(reason(&o).contains("matches your pick (2 files)"), "{}", reason(&o));
}

#[test]
fn widened_command_ask_shows_differs() {
    let p = tmp("widened");
    setup(&p, true, Some(0));
    build3(&p);
    pre(&p, "rm -rf build", "default", "toolu_w1");
    let v = pending(&p, "toolu_w1").unwrap();
    write_choice(&p, "toolu_w1", &keep_idx(&v, &["build/report-a.txt"]), now());
    // A command that includes a kept file (e.g. widened to `rm -rf build`) is no longer ask but a deny asking to retry without it; see the keep_* tests below.
    // A partial narrowing that drops a kept file (subset) still yields a "differs" ask.
    let o = pre(&p, "rm -rf build/report-b.txt", "default", "toolu_w2");
    assert_eq!(decision(&o), "ask");
    assert!(reason(&o).contains("differs from your pick"), "{}", reason(&o));
    // Even while choice is alive, this card's pending file is still written so this card can also be picked on screen.
    assert!(pending(&p, "toolu_w2").is_some(), "no pending for toolu_w2");
}

#[test]
fn choice_file_never_bypasses_final_ask() {
    let p = tmp("never_bypass");
    setup(&p, true, Some(0));
    build3(&p);
    pre(&p, "rm -rf build", "default", "toolu_b1");
    let v = pending(&p, "toolu_b1").unwrap();
    write_choice(&p, "toolu_b1", &keep_idx(&v, &["build/report-a.txt"]), now());
    let narrowed = "rm -rf build/report-b.txt build/cache";
    // Even on a match, the permission-dialog mode stays ask (never allow).
    let o = pre(&p, narrowed, "default", "toolu_b2");
    assert_eq!(o.code, 0);
    assert_eq!(decision(&o), "ask");
    // Mode without a dialog stays blocked.
    let o = pre(&p, narrowed, "bypassPermissions", "toolu_b3");
    assert_eq!(o.code, 2);
    assert!(o.stdout.is_empty());
    // A corrupt, oversized or malformed choice is treated as absent (first-edition text); the verdict stays ask.
    let bad: [String; 4] = [
        "{not json".to_string(),
        json!({"v": 2, "id": "toolu_b1", "keep": [0], "at": now()}).to_string(),
        json!({"v": 1, "id": "other_id", "keep": [0], "at": now()}).to_string(),
        format!("{{\"v\":1,\"id\":\"toolu_b1\",\"keep\":[0],\"at\":{},\"pad\":\"{}\"}}", now(), "x".repeat(1_100_000)),
    ];
    for b in bad {
        fs::write(p.join(".olp/pending/toolu_b1.choice.json"), b).unwrap();
        let o = pre(&p, narrowed, "default", "toolu_b4");
        assert_eq!(decision(&o), "ask");
        assert!(!reason(&o).contains("matches your pick") && !reason(&o).contains("differs from your pick"), "{}", reason(&o));
    }
}

// ---- Keep protection: a kept file in the command target gives no execution + a retry-without deny reason ----
// Value: protects=a later delete command that contains a file the user picked to keep is not executed, and the agent receives the kept names and retries without them; no keep, an expired pick (10 min), a pick for other files, and no-dialog mode behave as before; fails_when=a command containing a kept file goes out as ask and is deleted by one "yes", blocks when there is no pick, blocks because of another card's pick, or names land in the persistent log; why_new=keep protection; seam=pending and choice files (same shape as the server); no_claim=no guarantee that a human wrote the choice file, so this path only blocks (deny) and never allows.

/// Result of the same delete command arriving again after a pick.
fn rerun_after_pick(name: &str, keep: &[&str], ago_s: u64, again: &str) -> (PathBuf, HookOutcome) {
    let p = tmp(name);
    setup(&p, true, Some(0));
    build3(&p);
    pre(&p, "rm -rf build", "default", "toolu_k1");
    let v = pending(&p, "toolu_k1").unwrap();
    write_choice(&p, "toolu_k1", &keep_idx(&v, keep), now() - ago_s);
    let o = pre(&p, again, "default", "toolu_k2");
    (p, o)
}

#[test]
fn keep_none_stays_ask() {
    let (_, o) = rerun_after_pick("keep_none", &[], 0, "rm -rf build");
    assert_eq!(decision(&o), "ask", "zero kept = all chosen for deletion -> permission dialog");
    assert!(reason(&o).contains("matches your pick (3 files)"), "{}", reason(&o));
}

#[test]
fn keep_some_denies_with_rerun_reason() {
    let (p, o) = rerun_after_pick("keep_some", &["build/report-a.txt"], 0, "rm -rf build");
    assert_eq!(o.code, 0);
    assert_eq!(decision(&o), "deny", "{}", o.stdout);
    let r = reason(&o);
    assert!(r.contains("build/report-a.txt"), "{r}");
    assert!(r.contains("chose to keep") && r.contains("without"), "{r}");
    assert!(r.starts_with("[OLP] "), "deny reasons carry the same marker: {r}");
    assert!(!r.contains("report-b") && !r.contains("cache/c.txt"), "names picked for deletion must not appear in the reason: {r}");
    // The persistent log holds no names; the decision is recorded.
    let log = fs::read_to_string(p.join(".olp/hook-log.jsonl")).unwrap();
    assert!(!log.contains("report-a"), "{log}");
    assert!(log.contains("kept_by_choice"), "{log}");
}

#[test]
fn keep_reason_lists_at_most_three_names() {
    let p = tmp("keep_many");
    setup(&p, true, Some(0));
    fs::create_dir_all(p.join("out")).unwrap();
    for i in 1..=5 {
        fs::write(p.join(format!("out/f{i}.txt")), "x").unwrap();
    }
    pre(&p, "rm -rf out", "default", "toolu_m1");
    let v = pending(&p, "toolu_m1").unwrap();
    write_choice(&p, "toolu_m1", &keep_idx(&v, &["out/f1.txt", "out/f2.txt", "out/f3.txt", "out/f4.txt", "out/f5.txt"]), now());
    let o = pre(&p, "rm -rf out", "default", "toolu_m2");
    assert_eq!(decision(&o), "deny");
    let r = reason(&o);
    assert_eq!(r.matches("out/f").count(), 3, "{r}");
    assert!(r.contains("and 2 more"), "{r}");
}

#[test]
fn keep_choice_older_than_10min_stays_ask() {
    let (_, o) = rerun_after_pick("keep_stale", &["build/report-a.txt"], 601, "rm -rf build");
    assert_eq!(decision(&o), "ask");
}

#[test]
fn keep_choice_for_other_files_is_ignored() {
    // The kept file (report-a) is not a target of this command: it is another card's pick, so no block.
    let (_, o) = rerun_after_pick("keep_other", &["build/report-a.txt"], 0, "rm -rf build/cache");
    assert_eq!(decision(&o), "ask");
    assert!(!reason(&o).contains("chose to keep"), "{}", reason(&o));
}

#[test]
fn keep_in_no_prompt_mode_still_blocks_as_before() {
    let p = tmp("keep_bypass");
    setup(&p, true, Some(0));
    build3(&p);
    pre(&p, "rm -rf build", "default", "toolu_kb1");
    let v = pending(&p, "toolu_kb1").unwrap();
    write_choice(&p, "toolu_kb1", &keep_idx(&v, &["build/report-a.txt"]), now());
    let o = pre(&p, "rm -rf build", "bypassPermissions", "toolu_kb2");
    assert_eq!(o.code, 2, "mode without a dialog blocks as before");
}

#[test]
fn stale_choice_ignored_after_10min() {
    let p = tmp("stale_choice");
    setup(&p, true, Some(0));
    build3(&p);
    pre(&p, "rm -rf build", "default", "toolu_s1");
    let v = pending(&p, "toolu_s1").unwrap();
    write_choice(&p, "toolu_s1", &keep_idx(&v, &["build/report-a.txt"]), now() - 601);
    let o = pre(&p, "rm -rf build/report-b.txt build/cache", "default", "toolu_s2");
    assert!(!reason(&o).contains("matches your pick"), "{}", reason(&o));
}

#[test]
fn pending_swept_at_hook_start_after_10min() {
    let p = tmp("sweep");
    setup(&p, false, None);
    let d = p.join(".olp/pending");
    fs::create_dir_all(&d).unwrap();
    for f in ["old_1.json", "old_1.choice.json", "fresh_1.json", "old_1.json.tmp", "notes.txt"] {
        fs::write(d.join(f), "{}").unwrap();
    }
    for f in ["old_1.json", "old_1.choice.json", "old_1.json.tmp", "notes.txt"] {
        set_mtime(&d.join(f), 601);
    }
    // Pick off + read-only command: the sweep still runs at hook start.
    pre(&p, "ls", "default", "toolu_sw1");
    assert!(!d.join("old_1.json").exists());
    assert!(!d.join("old_1.choice.json").exists());
    assert!(d.join("fresh_1.json").exists());
    assert!(d.join("old_1.json.tmp").exists(), "files outside the name pattern are left alone");
    assert!(d.join("notes.txt").exists());
}

// ---- pick flow: liveness and path handling ----

#[test]
fn server_down_means_v1_behavior() {
    let p = tmp("server_down");
    setup(&p, true, Some(20)); // older than 15 s counts as dead
    build3(&p);
    let o = pre(&p, "rm -rf build", "default", "toolu_d1");
    assert_eq!(decision(&o), "ask");
    assert!(!reason(&o).contains(PICK_LINE));
    assert!(pending(&p, "toolu_d1").is_none());
}

#[test]
fn pending_paths_are_repo_relative() {
    let root = tmp("repo_rel");
    let p = root.join("proj");
    setup(&p, true, Some(0));
    fs::create_dir_all(p.join("sub/x")).unwrap();
    fs::write(p.join("sub/x/a.txt"), "x").unwrap();
    // A count that followed cd is also project-relative and uses `/` separators.
    pre(&p, "cd sub && rm -rf x", "default", "toolu_r1");
    let v = pending(&p, "toolu_r1").expect("pending");
    assert_eq!(pending_names(&v), ["sub/x/a.txt"]);
    let raw = fs::read_to_string(p.join(".olp/pending/toolu_r1.json")).unwrap();
    assert!(!raw.contains(&*root.to_string_lossy()) && !raw.contains(":\\") && !raw.contains(".."), "{raw}");
    // A file outside the project yields no pending.
    fs::create_dir_all(root.join("outside")).unwrap();
    fs::write(root.join("outside/o.txt"), "x").unwrap();
    let o = pre(&p, "rm -rf ../outside", "default", "toolu_r2");
    assert_eq!(decision(&o), "ask");
    assert!(pending(&p, "toolu_r2").is_none());
    // Windows treats cwd and project paths that differ only by case as the same folder (prefix comparison ignores case).
    #[cfg(windows)]
    {
        let v = json!({
            "session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy().to_uppercase(),
            "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "rm -rf sub"},
            "permission_mode": "default", "tool_use_id": "toolu_r3",
        });
        run_hook(v.to_string().as_bytes(), Some(&p));
        let v = pending(&p, "toolu_r3").expect("pending exists for a cwd that differs by case");
        assert_eq!(pending_names(&v), ["sub/x/a.txt"]);
    }
}

// ---- pick flow: truncated lists ----

#[test]
fn pending_list_capped_200_with_note() {
    let p = tmp("cap200");
    setup(&p, true, Some(0));
    fs::create_dir_all(p.join("many")).unwrap();
    for i in 0..201 {
        fs::write(p.join(format!("many/f{i:03}.txt")), "x").unwrap();
    }
    let o = pre(&p, "rm -rf many", "default", "toolu_c1");
    assert!(message(&o).contains("first 200 of 201 files listed"), "{}", message(&o));
    let v = pending(&p, "toolu_c1").unwrap();
    assert_eq!(v["files"].as_array().unwrap().len(), 200);
    assert_eq!(v["more_n"], 1);
    // more_n > 0 means the total is unknown, so always differs.
    write_choice(&p, "toolu_c1", &[0], now());
    let first = v["files"][0]["name"].as_str().unwrap().to_string();
    let rest: Vec<String> = (0..201).map(|i| format!("many/f{i:03}.txt")).filter(|n| *n != first).collect();
    let o = pre(&p, &format!("rm -f {}", rest.join(" ")), "default", "toolu_c2");
    assert!(reason(&o).contains("differs from your pick"), "{}", reason(&o));
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

#[test]
fn nature_two_spawns_within_3000ms() {
    let p = tmp("nature");
    git(&p, &["init", "-q", "-b", "main"]);
    setup(&p, true, Some(0));
    fs::create_dir_all(p.join("w")).unwrap();
    fs::write(p.join(".gitignore"), "w/gen_kxnat.log\n").unwrap();
    fs::write(p.join("w/old_kxnat.txt"), "t").unwrap();
    git(&p, &["add", ".gitignore", "w/old_kxnat.txt"]);
    git(&p, &["commit", "-q", "-m", "one"]);
    set_mtime(&p.join("w/old_kxnat.txt"), 3 * 86400);
    fs::write(p.join("w/new_kxnat.txt"), "t").unwrap(); // tracked + recent
    git(&p, &["add", "w/new_kxnat.txt"]);
    fs::write(p.join("w/gen_kxnat.log"), "g").unwrap(); // ignored
    fs::write(p.join("w/loose_kxnat.txt"), "u").unwrap(); // untracked + recent
    set_mtime(&p.join("w/loose_kxnat.txt"), 3 * 86400);
    let t0 = Instant::now();
    pre(&p, "rm -rf w", "default", "toolu_nat1");
    assert!(t0.elapsed() < Duration::from_millis(3000 + 2000), "{:?}", t0.elapsed());
    let v = pending(&p, "toolu_nat1").unwrap();
    let nat = |n: &str| v["files"].as_array().unwrap().iter().find(|f| f["name"] == n).unwrap()["nature"].as_str().unwrap().to_string();
    assert_eq!(nat("w/old_kxnat.txt"), "tracked");
    assert_eq!(nat("w/new_kxnat.txt"), "recent");
    assert_eq!(nat("w/gen_kxnat.log"), "ignored");
    assert_eq!(nat("w/loose_kxnat.txt"), "untracked");
    // Classification reads are a closed set of 2 git calls (1 ls-files + 1 check-ignore); ls-files runs exactly once for this card's names.
    let calls = olp_hook::counter::spawn::calls();
    assert_eq!(calls.iter().filter(|c| c.first().map(|s| s == "ls-files").unwrap_or(false) && c.iter().any(|a| a == "w/old_kxnat.txt")).count(), 1, "{calls:?}");
    assert!(calls.iter().any(|c| c.first().map(|s| s == "check-ignore").unwrap_or(false)), "{calls:?}");
}

// ---- walk consistency ----

#[test]
fn pending_file_has_names_hook_log_has_none() {
    let p = tmp("palref");
    setup(&p, true, Some(0));
    fs::create_dir_all(p.join("kxcanary_dir")).unwrap();
    fs::write(p.join("kxcanary_dir/kxcanary_name.txt"), "x").unwrap();
    pre(&p, "rm -rf kxcanary_dir", "default", "toolu_pal1");
    let v = pending(&p, "toolu_pal1").unwrap();
    assert_eq!(pending_names(&v), ["kxcanary_dir/kxcanary_name.txt"]);
    let log = fs::read_to_string(p.join(".olp/hook-log.jsonl")).unwrap();
    assert!(!log.contains("kxcanary"), "{log}");
}

#[test]
fn config_pick_field_parses_and_unknown_still_rejected() {
    let p = tmp("cfg_pick");
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::write(p.join(".olp/hook-config.json"), r#"{"pick": true}"#).unwrap();
    assert!(olp_hook::config::load(&p).unwrap().pick);
    fs::write(p.join(".olp/hook-config.json"), r#"{}"#).unwrap();
    assert!(!olp_hook::config::load(&p).unwrap().pick);
    fs::write(p.join(".olp/hook-config.json"), r#"{"pick": true, "pikc": true}"#).unwrap();
    assert!(olp_hook::config::load(&p).is_err());
}

/// Count and list come from a single walk: in a tree mixing a missing path, a link and an empty folder, the counts agree.
fn m4_tree(name: &str) -> (PathBuf, bool) {
    let p = tmp(name);
    fs::create_dir_all(p.join("a/b")).unwrap();
    fs::create_dir_all(p.join("e")).unwrap();
    fs::write(p.join("a/1.txt"), "1").unwrap();
    fs::write(p.join("a/b/2.txt"), "2").unwrap();
    fs::write(p.join("top.txt"), "t").unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(p.join("a"), p.join("a/b/ln")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(p.join("a"), p.join("a/b/ln")).is_ok();
    (p, linked)
}

#[test]
fn count_and_list_agree_on_same_tree() {
    let (p, linked) = m4_tree("m4_agree");
    if !linked {
        eprintln!("m4: link not made on this machine (needs symlink right) — link row skipped");
    }
    let args = ["a", "e", "nope", "top.txt"];
    let far = Instant::now() + Duration::from_secs(5);
    let n = olp_hook::counter::count_paths(&p, &args, far).unwrap();
    let (paths, total) = olp_hook::counter::list_paths(&p, &args, far, usize::MAX).unwrap();
    // 1.txt, 2.txt, top.txt + the link (counted once, not followed); empty folder 0; missing path 0
    assert_eq!(n, if linked { 4 } else { 3 });
    assert_eq!(total, n);
    assert_eq!(paths.len() as u64, n);
}

#[test]
fn list_cap_zero_collects_no_paths() {
    let (p, _) = m4_tree("m4_cap0");
    let far = Instant::now() + Duration::from_secs(5);
    let (paths, total) = olp_hook::counter::list_paths(&p, &["a", "e", "nope", "top.txt"], far, 0).unwrap();
    assert!(paths.is_empty());
    assert_eq!(total, olp_hook::counter::count_paths(&p, &["a", "e", "nope", "top.txt"], far).unwrap());
    // Only the sort is skipped; the time limit still applies: a past deadline gives Err.
    assert!(olp_hook::counter::list_paths(&p, &["a"], Instant::now(), 0).is_err());
}
