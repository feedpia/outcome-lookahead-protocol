//! Concurrent hook calls each leave one whole log line: no two records glued onto one line, so `stats` counts every call.
// Value: protects=hook-log.jsonl stays one JSON record per line when several hook processes append at the same time;
//   fails_when=a record and its newline are written separately, so parallel writers interleave and stats undercounts;
//   why_new=chaos run (20 calls at once) glued 2 line pairs and stats counted 16 of 20; seam=store::append_log, stats::stats, the olp-hook binary; no_claim=network filesystems.
use olp_hook::store::{self, LogLine};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier};

fn tmp(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("olp_log_append_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("build")).unwrap();
    for n in ["a.log", "b.tmp", "c.txt"] {
        std::fs::write(p.join("build").join(n), "x").unwrap();
    }
    p
}

/// Raw lines of the log file: (non-empty lines, lines that are not one JSON object).
fn raw_lines(project: &Path) -> (usize, usize) {
    let s = std::fs::read_to_string(project.join(".olp").join("hook-log.jsonl")).unwrap_or_default();
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let bad = lines.iter().filter(|l| serde_json::from_str::<serde_json::Value>(l).is_err()).count();
    (lines.len(), bad)
}

#[test]
fn parallel_appends_keep_one_record_per_line() {
    let project = tmp("threads");
    let (threads, each) = (16, 100);
    let gate = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let (p, g) = (project.clone(), gate.clone());
            std::thread::spawn(move || {
                g.wait();
                for _ in 0..each {
                    store::append_log(&p, &LogLine { ts: store::now(), event: "decision", verdict: Some("asked"), ..Default::default() }).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(raw_lines(&project), (threads * each, 0), "every append is one whole line");
}

/// Starts 20 hook calls (rm and Write, both ask), then hands each its input so they decide and append together.
fn twenty_at_once(project: &Path, round: usize) {
    let call = |i: usize| {
        let input = if i.is_multiple_of(2) {
            json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}})
        } else {
            json!({"tool_name": "Write", "tool_input": {"file_path": project.join("keep.txt").to_string_lossy(), "content": "x"}})
        };
        let mut v = json!({"session_id": "s", "transcript_path": "t", "cwd": project.to_string_lossy(), "hook_event_name": "PreToolUse",
            "permission_mode": "default", "tool_use_id": format!("toolu_par{round}x{i}")});
        v.as_object_mut().unwrap().extend(input.as_object().unwrap().clone());
        let mut c = Command::new(env!("CARGO_BIN_EXE_olp-hook"));
        c.env("CLAUDE_PROJECT_DIR", project).env("HOME", project).env("USERPROFILE", project).env_remove("OLP_HOOK_SIGNAL");
        c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        (child.stdin.take().unwrap(), child, v.to_string())
    };
    let mut started: Vec<_> = (0..20).map(call).collect();
    for (stdin, _, body) in started.iter_mut() {
        use std::io::Write;
        stdin.write_all(body.as_bytes()).unwrap();
    }
    for (stdin, child, _) in started {
        drop(stdin);
        let o = child.wait_with_output().unwrap();
        assert_eq!(o.status.code(), Some(0), "each call asks");
        assert!(String::from_utf8_lossy(&o.stdout).contains("\"ask\""), "each call asks");
    }
}

#[test]
fn twenty_hook_calls_at_once_leave_twenty_lines_and_stats_counts_twenty() {
    let project = tmp("procs");
    let rounds = 5;
    for round in 0..rounds {
        twenty_at_once(&project, round);
        assert_eq!(raw_lines(&project), (20 * (round + 1), 0), "round {round}: every call = one whole line");
    }
    let s = olp_hook::stats::stats(&project).unwrap();
    assert!(s.contains(&format!("decisions: {}", 20 * rounds)), "stats counts every call: {s}");
}
