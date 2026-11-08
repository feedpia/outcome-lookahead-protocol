//! Tests for `olp-hook watch`: the text screen and the single local HTML page from `watch --html`.
//! All file and git operations happen only in temporary folders under CARGO_TARGET_TMPDIR.

use olp_hook::stats::{week, Week};
use olp_hook::watch::{self, Tail};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);
const NOW: u64 = 1_793_440_000;
const DAY: u64 = 86_400;

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_watch_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

fn card(ts: u64, kind: &str, n: Option<u64>, id: Option<&str>) -> Value {
    let mut d = json!({"ts": ts, "event": "decision", "verdict": "asked", "effect": "writes_other", "reversible": false, "recovery": "unknown", "unchecked": n.is_none(), "card": "card_shown",
        "findings": [{"kind": kind, "count": {"n": n, "unit": if kind == "file_delete" || kind == "file_overwrite" { "file" } else { "row" }, "source": "counted"}}]});
    if let Some(id) = id {
        d["tool_use_id"] = json!(id);
    }
    d
}

fn post(ts: u64, id: &str) -> Value {
    json!({"ts": ts, "event": "post", "card": "executed_after_card", "tool_use_id": id})
}

fn quiet(ts: u64) -> Value {
    json!({"ts": ts, "event": "decision", "verdict": "quiet_pass", "findings": []})
}

fn write_log(project: &Path, lines: &[Value]) {
    fs::create_dir_all(project.join(".olp")).unwrap();
    let s: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(project.join(".olp").join("hook-log.jsonl"), s).unwrap();
}

fn olp(project: &Path, args: &[&str], stdin: &str) -> (String, String, i32) {
    use std::process::Stdio;
    let mut c = Command::new(env!("CARGO_BIN_EXE_olp-hook"));
    c.args(args).env("CLAUDE_PROJECT_DIR", project).env_remove("NO_COLOR").env_remove("SSH_CONNECTION").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = c.spawn().unwrap();
    feed(&mut ch.stdin.take().unwrap(), stdin.as_bytes());
    let o = ch.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned(), o.status.code().unwrap_or(-1))
}

/// Writes to the child's stdin. Only the child having exited before reading (BrokenPipe; setup does not prompt when there is no terminal) is tolerated; any other error fails.
/// The caller asserts on the child's output and exit code (this function does not change verdicts).
/// Windows closed-pipe shapes: NotConnected and os error 232 (ERROR_NO_DATA) / 233 (ERROR_PIPE_NOT_CONNECTED) count as the same "exited early" case.
fn feed(w: &mut impl std::io::Write, bytes: &[u8]) {
    let closed = |e: &std::io::Error| {
        matches!(e.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::NotConnected) || (cfg!(windows) && matches!(e.raw_os_error(), Some(232 | 233)))
    };
    match w.write_all(bytes) {
        Err(e) if closed(&e) => {}
        Err(e) => panic!("writing child input failed: kind={:?} os={:?} {e}", e.kind(), e.raw_os_error()),
        Ok(()) => {}
    }
}

// Value: protects=a test does not blow up with BrokenPipe when the child exits without reading input (no terminal = setup does not prompt); other write errors still fail;
// fails_when=panic on BrokenPipe, or swallowing other errors; seam=feed; no_claim=child output and exit code (asserted by each test).
#[test]
fn feed_tolerates_a_child_that_closed_stdin_first() {
    use std::process::Stdio;
    let mut ch = Command::new(env!("CARGO_BIN_EXE_olp-hook")).arg("--help").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let mut sin = ch.stdin.take().unwrap();
    while ch.try_wait().unwrap().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    feed(&mut sin, &vec![b'n'; 1 << 20]); // the child has already exited, so the pipe is closed
}

/// On Windows a closed pipe can surface as NotConnected or os error 232 (ERROR_NO_DATA) / 233 (ERROR_PIPE_NOT_CONNECTED) as well as BrokenPipe; this was seen intermittently.
#[test]
fn feed_tolerates_every_closed_pipe_form() {
    struct Closed(std::io::ErrorKind, Option<i32>);
    impl std::io::Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(match self.1 {
                Some(n) => std::io::Error::from_raw_os_error(n),
                None => std::io::Error::new(self.0, "closed"),
            })
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut forms = vec![(std::io::ErrorKind::BrokenPipe, None), (std::io::ErrorKind::NotConnected, None)];
    if cfg!(windows) {
        forms.push((std::io::ErrorKind::Other, Some(232)));
        forms.push((std::io::ErrorKind::Other, Some(233)));
    }
    for (kind, os) in forms {
        assert!(std::panic::catch_unwind(move || feed(&mut Closed(kind, os), b"n")).is_ok(), "closed-pipe shape should be tolerated: kind={kind:?} os={os:?}");
    }
}

#[test]
fn feed_still_fails_on_other_write_errors() {
    struct Bad;
    impl std::io::Write for Bad {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(std::panic::catch_unwind(|| feed(&mut Bad, b"n")).is_err(), "errors other than BrokenPipe must still fail");
}

fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            m.insert(e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap_or_default());
        }
    }
    m
}

// Value: protects=the current-card field shows the last card's kind, count and undo info unchanged (no stale first-card values); fails_when=the first card's values show or undo info is inferred; seam=render (pure); no_claim=whether it matches the permission prompt moment is a manual check.
#[test]
fn watch_shows_latest_card_kind_count_undo() {
    let log = vec![card(NOW - 100, "file_overwrite", Some(9), Some("a")), post(NOW - 99, "a"), card(NOW - 5, "file_delete", Some(3), Some("b"))];
    let s = watch::render(&log, NOW);
    for want in ["delete files", "3 files", "Undo: not confirmed", "asked"] {
        assert!(s.contains(want), "{want} in {s}");
    }
    assert!(!s.contains("9 files") && !s.contains("overwrite files"), "{s}");
}

// Value: protects=an uncounted number shows as "count not checked" and is never written as 0; fails_when=null or a missing n renders as `0 files`; seam=render; no_claim=none beyond protects.
#[test]
fn watch_not_counted_is_never_zero() {
    let mut null_n = card(NOW - 5, "file_delete", None, Some("a"));
    null_n["findings"][0]["count"].as_object_mut().unwrap().remove("source");
    let missing_n = json!({"ts": NOW - 5, "event": "decision", "verdict": "asked", "card": "card_shown", "unchecked": true, "findings": [{"kind": "file_delete", "count": {"unit": "file"}}]});
    for log in [vec![null_n], vec![missing_n]] {
        let s = watch::render(&log, NOW);
        assert!(s.contains("count not checked"), "{s}");
        assert!(!s.contains("0 files"), "{s}");
    }
}

// Value: protects=pairing: a PostToolUse with the same id = ran, a later decision with no pair = not run, in between = waiting (stays waiting after 60 s or 10 min); fails_when=not run appears from elapsed time alone or an unpaired card shows as ran; seam=render (now injected); no_claim=cannot tell rejection from failure; with parallel tool calls a read tool's quiet_pass recorded first while a permission is pending may briefly show not run; Claude Code's parallel execution and hook ordering are not verified.
#[test]
fn watch_pairs_ran_and_not_run() {
    let ran = vec![card(NOW - 10, "file_delete", Some(3), Some("a")), post(NOW - 5, "a")];
    assert!(watch::render(&ran, NOW).lines().any(|l| l == "Result: ran"), "{}", watch::render(&ran, NOW));
    let other_id = vec![card(NOW - 10, "file_delete", Some(3), Some("a")), post(NOW - 5, "zzz")];
    assert!(watch::render(&other_id, NOW).contains("waiting for your answer"), "a different id is not a pairing");
    let moved_on = vec![card(NOW - 10, "file_delete", Some(3), Some("a")), quiet(NOW - 5)];
    assert!(watch::render(&moved_on, NOW).lines().any(|l| l == "Result: not run"), "{}", watch::render(&moved_on, NOW));
    let next_card = vec![card(NOW - 10, "file_delete", Some(3), Some("a")), card(NOW - 5, "db_delete", Some(2), Some("b"))];
    let s = watch::render(&next_card, NOW);
    assert!(s.contains("delete database rows") && s.contains("waiting for your answer (5s)"), "{s}");
    let one = vec![card(NOW, "file_delete", Some(3), Some("a"))];
    for (later, want) in [(0, "(0s)"), (59, "(59s)"), (61, "(61s)"), (600, "(600s)"), (3_600, "(3600s)")] {
        let s = watch::render(&one, NOW + later);
        assert!(s.contains("Result: waiting for your answer") && s.contains(want) && !s.contains("Result: not run"), "{later}: {s}");
    }
}

// Value: protects=only allowed fields are drawn: path/command canaries placed in disallowed fields (why, reasons, unknown fields, tool_use_id, unfamiliar text in the kind field) never appear in the text or HTML screen; fails_when=a canary shows on any screen; seam=render, render_html; no_claim=no validation of the meaning of values inside allowed fields.
#[test]
fn watch_never_prints_unlisted_fields() {
    let mut c = card(NOW - 5, "file_delete", Some(3), Some("C:\\secret\\x"));
    c["why"] = json!("C:\\secret\\x");
    c["reasons"] = json!(["rm -rf /tmp/canary", "C:\\secret\\x"]);
    c["cmd"] = json!("rm -rf /tmp/canary");
    c["path"] = json!("C:\\secret\\x");
    c["human_choice"] = json!("rm -rf /tmp/canary");
    c["findings"][0]["path"] = json!("C:\\secret\\x");
    c["findings"][0]["count"]["note"] = json!("rm -rf /tmp/canary");
    let mut weird = card(NOW - 4, "rm -rf /tmp/canary", Some(1), Some("w"));
    weird["findings"][0]["count"]["unit"] = json!("C:\\secret\\x");
    for log in [vec![c.clone()], vec![c, weird]] {
        let outs = [watch::render(&log, NOW), watch::render_color(&log, NOW), watch::render_html(&log, NOW, "en", true), watch::render_html(&log, NOW, "ko", false)];
        for o in outs {
            for bad in ["secret", "canary", "rm -rf", "C:\\"] {
                assert!(!o.contains(bad), "{bad} in {o}");
            }
        }
    }
}

// Value: protects=watch is read-only: the .olp file list and bytes are identical before and after, and with no log it neither creates .olp nor prints more than one line; fails_when=watch creates or changes files; seam=binary --once; no_claim=--html writes watch.html, so it is outside this test.
#[test]
fn watch_is_read_only() {
    let p = tmp("ro");
    write_log(&p, &[card(NOW - 5, "file_delete", Some(3), Some("a"))]);
    fs::write(p.join(".olp").join("hook-map.json"), "{\"version\":1,\"entries\":{}}").unwrap();
    let before = snapshot(&p.join(".olp"));
    let (out, _, code) = olp(&p, &["watch", "--once", "--plain"], "");
    assert_eq!(code, 0);
    assert!(out.contains("delete files") && out.contains("3 files"), "{out}");
    assert_eq!(before, snapshot(&p.join(".olp")), "bytes unchanged");
    let e = tmp("ro_empty");
    let (out, _, code) = olp(&e, &["watch", "--once", "--plain"], "");
    assert_eq!(code, 0);
    assert_eq!(out.trim_end(), "waiting for the first card");
    assert!(!e.join(".olp").exists(), "nothing is created when there is no log");
}

// Value: protects=the watch module source has no network or process execution (including opening a browser); fails_when=std::net, std::process or Command enters the source; seam=source scan (same shape as hook_makes_no_network_calls in hook.rs); no_claim=the dependency list check lives in hook.rs.
#[test]
fn watch_makes_no_network_or_process_calls() {
    let s = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("watch.rs")).unwrap();
    for b in ["std::net", "std::process", "Command", "TcpStream", "TcpListener", "UdpSocket", "ToSocketAddrs", "reqwest", "ureq", "hyper", "tokio", "http://", "https://", "xdg-open"] {
        assert!(!s.contains(b), "watch.rs must not contain {b}");
    }
}

// Value: protects=the this-week field equals the stats::week result (same function, same numbers) in both text and HTML; fails_when=watch counts separately and drifts from stats; seam=render, slots; no_claim=none beyond protects.
#[test]
fn watch_week_matches_stats() {
    let log = vec![
        card(NOW - DAY, "file_delete", Some(3), Some("a")),
        post(NOW - DAY + 5, "a"),
        card(NOW - 2 * DAY, "file_delete", Some(5), Some("b")),
        json!({"ts": NOW - 3 * DAY, "event": "decision", "verdict": "blocked", "findings": [{"kind": "file_delete", "count": {"n": 2, "unit": "file", "source": "counted"}}]}),
        card(NOW - 8 * DAY, "file_delete", Some(100), Some("old")),
        card(NOW - 3, "file_overwrite", Some(4), Some("z")),
    ];
    let w: Week = week(&log, NOW);
    let s = watch::render(&log, NOW);
    assert!(s.contains(&format!("This week: cards {} · ran {} · not run {} · blocked {}", w.cards, w.ran, w.not_run, w.blocked)), "{s}");
    assert!(s.contains(&format!("Files on cards: ran {} · not run {} · blocked {} · not counted {}", w.files_ran, w.files_not_run, w.files_blocked, w.not_counted)), "{s}");
    let m = watch::slots(&log, NOW, "en");
    for (k, v) in [("cards", w.cards), ("ran", w.ran), ("not_run", w.not_run), ("blocked", w.blocked), ("files_ran", w.files_ran), ("files_not_run", w.files_not_run), ("files_blocked", w.files_blocked), ("not_counted", w.not_counted)] {
        assert_eq!(m[&format!("week.{k}")], v.to_string(), "week.{k}");
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            for d in it.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// Value: protects=the no-color paths (NO_COLOR, --plain, non-terminal) emit zero ESC bytes and keep the same words (meaning never depends on color alone); fails_when=control characters leak into plain text or words exist only in the colored variant; seam=render, render_color, binary (piped); no_claim=how color looks in a terminal is a manual check.
#[test]
fn watch_plain_has_no_ansi() {
    let log = vec![card(NOW - 5, "file_delete", None, Some("a")), quiet(NOW - 1)];
    let plain = watch::render(&log, NOW);
    assert!(!plain.contains('\x1b'), "{plain:?}");
    let colored = watch::render_color(&log, NOW);
    assert!(colored.contains('\x1b'), "the colored variant must contain color codes");
    assert_eq!(strip_ansi(&colored), plain, "stripping color leaves the same words");
    let p = tmp("plain");
    write_log(&p, &log);
    let (out, _, _) = olp(&p, &["watch", "--once"], "");
    assert!(!out.contains('\x1b') && out.contains("count not checked"), "{out:?}");
}

// Value: protects=tail reading: reads only the grown part (one added line in a 100k-line log = only that line's bytes) and does not count a partial line until it ends; fails_when=re-reading from the start every time; seam=Tail; no_claim=the 50ms figure is a single local measurement (eprintln) with no assertion; not a CI number.
#[test]
fn watch_tail_reads_only_new_bytes() {
    use std::io::Write;
    let p = tmp("tail");
    fs::create_dir_all(p.join(".olp")).unwrap();
    let path = p.join(".olp").join("hook-log.jsonl");
    let mut big = String::new();
    for i in 0..100_000u64 {
        big.push_str(&format!("{}\n", json!({"ts": NOW - 1_000_000 + i, "event": "decision", "verdict": "quiet_pass", "findings": []})));
    }
    fs::write(&path, &big).unwrap();
    let mut t = Tail::new(&path);
    assert!(t.poll());
    assert_eq!(t.bytes_read, big.len() as u64);
    assert_eq!(t.records.len(), 100_000);
    assert!(!t.poll(), "no read when the file did not grow");
    assert_eq!(t.bytes_read, big.len() as u64);
    let line = format!("{}\n", card(NOW - 2, "file_delete", Some(3), Some("a")));
    let half = line.len() / 2;
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(&line.as_bytes()[..half]).unwrap();
    t.poll();
    assert_eq!(t.records.len(), 100_000, "a partial line is not counted yet");
    f.write_all(&line.as_bytes()[half..]).unwrap();
    assert!(t.poll());
    assert_eq!(t.records.len(), 100_001);
    assert_eq!(t.bytes_read, (big.len() + line.len()) as u64, "only new bytes are read");
    let t0 = std::time::Instant::now();
    let s = watch::render(&t.records, NOW);
    eprintln!("render of 100001 records: {:?} (local, debug build, not asserted)", t0.elapsed());
    assert!(s.contains("3 files"));
}

// Value: protects=when the log shrinks (deleted, replaced, rotated) it is re-read from the start and old cards are not shown; fails_when=an old card remains after the file shrinks; seam=Tail; no_claim=a replacement of the same size is not detected because only size is compared (no claim).
#[test]
fn watch_rereads_after_truncate() {
    let p = tmp("trunc");
    write_log(&p, &[card(NOW - 50, "file_delete", Some(7), Some("a")), card(NOW - 40, "file_delete", Some(8), Some("b")), card(NOW - 30, "file_delete", Some(9), Some("c"))]);
    let path = p.join(".olp").join("hook-log.jsonl");
    let mut t = Tail::new(&path);
    t.poll();
    assert!(watch::render(&t.records, NOW).contains("9 files"));
    write_log(&p, &[card(NOW - 5, "db_delete", Some(1), Some("n"))]);
    assert!(t.poll());
    let s = watch::render(&t.records, NOW);
    assert!(s.contains("delete database rows") && s.contains("1 row") && !s.contains("9 files"), "{s}");
    fs::remove_file(&path).unwrap();
    assert!(t.poll());
    assert_eq!(watch::render(&t.records, NOW), "waiting for the first card");
}

fn sample_log() -> Vec<Value> {
    vec![card(NOW - 10, "file_delete", Some(3), Some("a")), post(NOW - 5, "a"), card(NOW - 2, "git_reset_hard", Some(7), Some("b"))]
}

// Value: protects=the HTML has no external resources or scripts (no http://, https:// or //-prefixed src, no <script) and has a CSP line; fails_when=an outside address or script enters the template or fill; seam=render_html; no_claim=actual per-browser blocking is checked only by a manual Chromium run.
#[test]
fn html_has_no_external_resource_and_no_script() {
    for lang in ["en", "ko"] {
        let h = watch::render_html(&sample_log(), NOW, lang, true);
        for bad in ["http://", "https://", "src=\"//", "src='//", "<script", "href=\"//", "url(//", "@import"] {
            assert!(!h.contains(bad), "{bad}");
        }
        assert!(h.contains("http-equiv=\"Content-Security-Policy\"") && h.contains("default-src 'none'"), "CSP line");
        assert!(h.contains(&format!("<html lang=\"{lang}\">")) && h.contains("<meta charset=\"utf-8\">"));
    }
}

// Value: protects=write to a temp file then rename (no half-written file) and write only when content changes (same content keeps the file time); fails_when=a temp file is left behind or identical content is rewritten; seam=write_if_changed; no_claim=the retry on a Windows sharing violation belongs to the loop (manual check).
#[test]
fn html_written_atomically_and_only_on_change() {
    let p = tmp("atomic");
    let path = p.join("watch.html");
    let mut last = None;
    assert!(watch::write_if_changed(&path, "one", &mut last).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "one");
    let m1 = fs::metadata(&path).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(30));
    assert!(!watch::write_if_changed(&path, "one", &mut last).unwrap(), "same content means no write");
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), m1);
    assert!(watch::write_if_changed(&path, "two", &mut last).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "two");
    let names: Vec<String> = fs::read_dir(&p).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["watch.html".to_string()], "no temp file left behind: {names:?}");
    fs::remove_file(&path).unwrap();
    assert!(watch::write_if_changed(&path, "two", &mut last).unwrap(), "rewritten if it was deleted");
    let src = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src").join("watch.rs")).unwrap();
    assert!(src.contains("fs::rename("), "replaced via rename");
}

// Value: protects=the file URL is correct per OS and spaces, non-ASCII characters and # are percent-encoded; fails_when=# is taken as a fragment marker or backslashes remain; seam=file_url (pure string); no_claim=whether a real browser opens it is a manual check.
#[test]
fn file_url_is_percent_encoded_per_os() {
    assert_eq!(watch::file_url("C:\\Users\\홍 길동\\a#b\\.olp\\watch.html"), "file:///C:/Users/%ED%99%8D%20%EA%B8%B8%EB%8F%99/a%23b/.olp/watch.html");
    assert_eq!(watch::file_url("\\\\?\\C:\\p q\\.olp\\watch.html"), "file:///C:/p%20q/.olp/watch.html");
    assert_eq!(watch::file_url("/Users/x y/프로젝트#1/.olp/watch.html"), "file:///Users/x%20y/%ED%94%84%EB%A1%9C%EC%A0%9D%ED%8A%B8%231/.olp/watch.html");
    assert_eq!(watch::file_url("/home/a%b/.olp/watch.html"), "file:///home/a%25b/.olp/watch.html");
}

// Value: protects=in a display-less environment (SSH, or Linux with neither DISPLAY nor WAYLAND) it switches to a one-line notice, still writes the file, and exits 0; fails_when=only a link is printed with no notice, or the file is not written; seam=headless, start_message, binary; no_claim=does not open a browser itself (no process spawned).
#[test]
fn headless_prints_note_and_still_writes() {
    assert!(watch::headless(true, true, true, false), "SSH");
    assert!(watch::headless(false, false, false, true), "Linux, no display variables");
    assert!(!watch::headless(false, true, false, true) && !watch::headless(false, false, true, true));
    assert!(!watch::headless(false, false, false, false), "not Linux and not SSH = display available");
    assert!(watch::start_message("file:///x/.olp/watch.html", false).starts_with("Open this file in your browser: file:///x/.olp/watch.html"));
    assert_eq!(
        watch::start_message("file:///x", true),
        "No screen here — open .olp/watch.html on the machine that has the browser, or use `olp-hook watch` (text)."
    );
    let p = tmp("headless");
    write_log(&p, &sample_log());
    let mut c = Command::new(env!("CARGO_BIN_EXE_olp-hook"));
    c.args(["watch", "--html", "--once"]).env("CLAUDE_PROJECT_DIR", &p).env("SSH_CONNECTION", "1.2.3.4 1 5.6.7.8 22");
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(out.starts_with("No screen here"), "{out}");
    assert!(p.join(".olp").join("watch.html").exists());
}

// Value: protects=file names (text others can choose) are HTML-escaped; fails_when=<script>, "onload= or & go in unescaped; seam=fill(files); no_claim=in this unit files[] is always empty so it does not flow from a real log; only the fill function is checked.
#[test]
fn file_names_are_html_escaped() {
    let files = vec![("<script>x</script>".to_string(), "tracked".to_string()), ("\"onload=alert(1)".to_string(), "recent".to_string()), ("a&b<img src=x onerror=1>".to_string(), "ignored".to_string())];
    let vals = watch::slots(&sample_log(), NOW, "en");
    let h = watch::fill(watch::TEMPLATE, &vals, &files);
    assert!(!h.contains("<script") && !h.contains("<img") && !h.contains("\"onload="), "raw text was inserted unescaped");
    for want in ["&lt;script&gt;x&lt;/script&gt;", "&quot;onload=alert(1)", "a&amp;b&lt;img src=x onerror=1&gt;"] {
        assert!(h.contains(want), "{want}");
    }
    assert_eq!(h.matches("class=\"tile n-").count(), 3);
}

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut a = h;
        for i in 0..64 {
            let s1 = a[4].rotate_right(6) ^ a[4].rotate_right(11) ^ a[4].rotate_right(25);
            let ch = (a[4] & a[5]) ^ (!a[4] & a[6]);
            let t1 = a[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a[0].rotate_right(2) ^ a[0].rotate_right(13) ^ a[0].rotate_right(22);
            let maj = (a[0] & a[1]) ^ (a[0] & a[2]) ^ (a[1] & a[2]);
            let t2 = s0.wrapping_add(maj);
            a = [t1.wrapping_add(t2), a[0], a[1], a[2], a[3].wrapping_add(t1), a[4], a[5], a[6]];
        }
        for i in 0..8 {
            h[i] = h[i].wrapping_add(a[i]);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

// Value: protects=the {{...}} list in the template equals the 20 contract slots (none extra, none missing) and the template text matches the locked sha; fails_when=the template changes or slots drift from the contract; seam=TEMPLATE, SLOTS; no_claim=visuals, color and accessibility belong to front-end tests.
#[test]
fn template_slots_match_contract() {
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "sanity check of the sha implementation");
    let h = sha256_hex(watch::TEMPLATE.as_bytes());
    // The wording for the unreadable kind was changed from the old text to "git info missing" (values and slots unchanged); the old sha12 was 0c28d7bfe4d2.
    assert!(h.starts_with("1aa442c59e2f"), "template sha12 is pinned: {h}");
    let mut found = BTreeSet::new();
    let mut rest = watch::TEMPLATE;
    while let Some(i) = rest.find("{{") {
        let j = rest[i..].find("}}").unwrap() + i;
        found.insert(rest[i + 2..j].to_string());
        rest = &rest[j + 2..];
    }
    let want: BTreeSet<String> = [
        "lang", "updated_ago_s", "card.state", "card.kind", "card.count_n", "card.count_unit", "card.count_state", "card.undo", "card.waiting_s", "files[].name", "files[].nature", "files_more_n", "week.cards", "week.ran", "week.not_run", "week.blocked",
        "week.files_ran", "week.files_not_run", "week.files_blocked", "week.not_counted",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(want.len(), 20);
    assert_eq!(found, want);
    assert_eq!(watch::SLOTS.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(), want);
}

// Value: protects=code fields blank out text outside the closed list (they are used as class names, so no injection); fails_when=text outside the list enters the body class or the screen; seam=fill, slots; no_claim=none beyond protects.
#[test]
fn codes_outside_closed_list_are_blanked() {
    let mut vals = watch::slots(&sample_log(), NOW, "en");
    for (k, v) in [("card.state", "evil\" onload=\""), ("card.kind", "rm -rf"), ("card.count_unit", "files"), ("card.count_state", "x y"), ("card.undo", "maybe"), ("lang", "fr")] {
        vals.insert(k.to_string(), v.to_string());
    }
    let h = watch::fill(watch::TEMPLATE, &vals, &[("f".to_string(), "evil".to_string())]);
    assert!(h.contains("<body class=\"s- k- c- u- un-\">"), "{}", h.lines().find(|l| l.contains("<body")).unwrap_or(""));
    assert!(h.contains("<html lang=\"\">") && h.contains("class=\"tile n-\""));
    for bad in ["evil", "rm -rf", "maybe", "onload"] {
        assert!(!h.contains(bad), "{bad}");
    }
    let log = vec![card(NOW - 5, "rm -rf /tmp/canary", Some(3), Some("a"))];
    assert_eq!(watch::slots(&log, NOW, "en")["card.kind"], "");
    assert_eq!(watch::slots(&log, NOW, "fr")["lang"], "en", "unknown language falls back to en");
}

// Value: protects=setup adds one `.olp/` line to the repo's .git/info/exclude (still one line after running twice) and skips non-git folders; fails_when=duplicate lines, or .git appears in a non-git folder; seam=binary setup; no_claim=cannot remove files already committed (only prevents future wholesale adds).
#[test]
fn setup_adds_olp_to_git_info_exclude_once() {
    let p = tmp("setup_git");
    fs::create_dir_all(p.join(".git")).unwrap();
    let ex = p.join(".git").join("info").join("exclude");
    for _ in 0..2 {
        let (_, _, code) = olp(&p, &["setup"], "n\n");
        assert_eq!(code, 0);
        let s = fs::read_to_string(&ex).unwrap();
        assert_eq!(s.lines().filter(|l| l.trim() == ".olp/").count(), 1, "{s:?}");
    }
    fs::write(&ex, "# existing\n*.log").unwrap();
    olp(&p, &["setup"], "n\n");
    let s = fs::read_to_string(&ex).unwrap();
    assert_eq!(s, "# existing\n*.log\n.olp/\n", "appended on a new line even when the file lacks a trailing newline");
    let q = tmp("setup_nogit");
    let (out, _, code) = olp(&q, &["setup"], "n\n");
    assert_eq!(code, 0);
    assert!(!q.join(".git").exists() && out.contains("not a git repository"), "{out}");
}

// Value: protects=every {{slot}} value goes through one escape function (number and code fields too, not only file names); fails_when=`<`, `"` or `&` is inserted raw in any slot; seam=fill (all 20 slots one by one); no_claim=code fields blank out text outside their closed list, so "escaped text is visible" is checked only for number and name fields.
#[test]
fn all_slot_values_are_html_escaped() {
    const CANARY: &str = "<u>\"&'x";
    const ESCAPED: &str = "&lt;u&gt;&quot;&amp;&#39;x";
    let code_slots = ["lang", "card.state", "card.kind", "card.count_unit", "card.count_state", "card.undo", "files[].nature"];
    for slot in watch::SLOTS {
        let mut vals: BTreeMap<String, String> = watch::SLOTS.iter().map(|s| (s.to_string(), "1".to_string())).collect();
        vals.insert("lang".into(), "en".into());
        vals.insert("card.state".into(), "waiting".into());
        vals.insert("card.kind".into(), "file_delete".into());
        vals.insert("card.count_unit".into(), "file".into());
        vals.insert("card.count_state".into(), "counted".into());
        vals.insert("card.undo".into(), "not_confirmed".into());
        vals.insert(slot.to_string(), CANARY.to_string());
        let files = [(if slot == "files[].name" { CANARY } else { "n" }.to_string(), if slot == "files[].nature" { CANARY } else { "tracked" }.to_string())];
        let h = watch::fill(watch::TEMPLATE, &vals, &files);
        assert!(!h.contains(CANARY) && !h.contains("<u>"), "{slot}: raw text was inserted");
        if !code_slots.contains(&slot) {
            assert!(h.contains(ESCAPED), "{slot}: escaped text is missing");
        }
    }
}

// Value: protects=--once (single render, for recordings) output has no refresh line and repeated output has exactly one; fails_when=a recorded file reloads itself and changes; seam=render_html (refresh), binary --html --once; no_claim=none beyond protects.
#[test]
fn once_output_has_no_meta_refresh() {
    let on = watch::render_html(&sample_log(), NOW, "en", true);
    let off = watch::render_html(&sample_log(), NOW, "en", false);
    assert_eq!(on.matches("http-equiv=\"refresh\"").count(), 1);
    assert_eq!(off.matches("http-equiv=\"refresh\"").count(), 0);
    assert!(off.contains("Content-Security-Policy") && off.contains("<meta charset=\"utf-8\">"), "other lines unchanged");
    let p = tmp("once_html");
    write_log(&p, &sample_log());
    let (_, _, code) = olp(&p, &["watch", "--html", "--once"], "");
    assert_eq!(code, 0);
    let h = fs::read_to_string(p.join(".olp").join("watch.html")).unwrap();
    assert!(!h.contains("http-equiv=\"refresh\""));
}
