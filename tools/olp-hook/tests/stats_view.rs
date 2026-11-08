//! `olp-hook stats` weekly one-page view: animates only on a terminal and shows only real counts.

use olp_hook::stats::{animate_ok, frames, week, Week};
use serde_json::{json, Value};

const DAY: u64 = 86_400;
const NOW: u64 = 1_793_440_000; // test time (same rule on any UTC day)

fn decision(ts: u64, verdict: &str, id: Option<&str>, n: Option<u64>, source: &str) -> Value {
    let mut d = json!({"ts": ts, "event": "decision", "verdict": verdict, "findings": []});
    if let Some(n) = n {
        d["findings"] = json!([{"kind": "file_delete", "count": {"n": n, "unit": "file", "source": source}}]);
    }
    if verdict == "asked" {
        d["card"] = json!("card_shown");
        if let Some(id) = id {
            d["tool_use_id"] = json!(id);
        }
    }
    d
}

fn ran(ts: u64, id: &str) -> Value {
    json!({"ts": ts, "event": "post", "card": "executed_after_card", "tool_use_id": id})
}

fn sample() -> Vec<Value> {
    vec![
        decision(NOW - DAY, "asked", Some("a"), Some(3), "counted"),
        ran(NOW - DAY + 5, "a"),
        decision(NOW - 2 * DAY, "asked", Some("b"), Some(5), "counted"),
        decision(NOW - 3 * DAY, "blocked", None, Some(2), "counted"),
        decision(NOW - 6 * DAY, "asked", Some("c"), Some(7), "user_declared"),
        // Outside the 7 UTC days (8 days ago): not counted
        decision(NOW - 8 * DAY, "asked", Some("old"), Some(100), "counted"),
        ran(NOW - 8 * DAY + 5, "old"),
        decision(NOW - 8 * DAY, "blocked", None, Some(50), "counted"),
    ]
}

// Value: protects=this week's (last 7 UTC days) columns hold only values counted from the log, and 8 days ago counts as 0 (real counts only); fails_when=out-of-window records leak in or the ran/not-run pairing after a card is wrong; seam=week (pure); no_claim=no distinction between a refusal and an execution failure ("not run" means exactly that).
#[test]
fn stats_week_counts_from_log_only() {
    let w = week(&sample(), NOW);
    assert_eq!((w.cards, w.ran, w.not_run, w.blocked), (3, 1, 2, 1));
}

// Value: protects=the file total is only what the device counted (source=counted); user-given numbers are shown separately as "not counted"; fails_when=user_given leaks into the total and inflates it; seam=week; no_claim=no claim that the counted files were really deleted ("after ran" is only the report of running after a card).
#[test]
fn stats_files_sum_counted_only() {
    let w = week(&sample(), NOW);
    assert_eq!((w.files_ran, w.files_not_run, w.files_blocked), (3, 5, 2));
    assert_eq!(w.files_on_cards(), 10);
    assert_eq!(w.not_counted, 1, "one user_given card");
}

fn full() -> Week {
    week(&sample(), NOW)
}

// Value: protects=the animation starts at 0 and rises only to the real values, the last frame equals the real values, no intermediate overshoot, and the frame count is capped; fails_when=a number larger than the real one shows mid-animation or the final number differs; seam=frames (pure); no_claim=no claim about perceived animation speed.
#[test]
fn stats_frames_end_at_real_values() {
    let w = full();
    let f = frames(&w, 20);
    assert!(f.len() >= 2 && f.len() <= 40, "{}", f.len());
    let last = f.last().unwrap();
    assert_eq!(last, &frames(&w, 1)[0], "last frame equals the single real-value frame");
    for want in ["Cards in the prompt", "Ran after a card", "Not run after a card", "Blocked", "Files on cards (counted on this device): 10", "Not counted: 1 card"] {
        assert!(last.contains(want), "{want} in {last}");
    }
    let nums = |s: &str| -> Vec<u64> { s.lines().take(5).skip(1).filter_map(|l| l.split_whitespace().find_map(|t| t.parse().ok())).collect() };
    let end = nums(last);
    assert_eq!(end, vec![3, 1, 2, 1]);
    for fr in &f {
        for (a, b) in nums(fr).iter().zip(&end) {
            assert!(a <= b, "no intermediate overshoot: {fr}");
        }
    }
    assert_eq!(nums(&f[0]), vec![0, 0, 0, 0], "starts at 0");
}

// Value: protects=no percentages, probabilities or effect-claim wording (saved, prevented, would have been); fails_when=effect-claim wording appears in the output; seam=frames; no_claim=none beyond protects.
#[test]
fn stats_no_percent_or_probability() {
    for fr in frames(&full(), 20).iter().chain(frames(&Week::default(), 20).iter()) {
        let low = fr.to_lowercase();
        for bad in ["%", "probability", "saved", "prevented", "would have", "almost"] {
            assert!(!low.contains(bad), "{bad}: {fr}");
        }
    }
}

// Value: protects=bars and text are ASCII only (no garbling of block or dash characters on cp949 consoles); control characters live only in the screen-drawing code; fails_when=a frame contains non-ASCII or control characters; seam=frames; no_claim=no claim about VT support in old conhost (one README line).
#[test]
fn stats_frames_are_ascii_only() {
    for fr in frames(&full(), 20) {
        assert!(fr.chars().all(|c| c == '\n' || (c.is_ascii() && !c.is_ascii_control())), "{fr:?}");
    }
}

// Value: protects=animation only on a terminal with no NO_COLOR, TERM not dumb and no --plain; otherwise the plain text output stays as is; fails_when=control characters appear in piped or CI output; seam=animate_ok (pure); no_claim=none beyond protects.
#[test]
fn stats_no_color_env_plain() {
    assert!(animate_ok(true, false, Some("xterm-256color"), false));
    assert!(animate_ok(true, false, None, false));
    assert!(!animate_ok(false, false, Some("xterm"), false), "not a terminal");
    assert!(!animate_ok(true, true, Some("xterm"), false), "NO_COLOR");
    assert!(!animate_ok(true, false, Some("dumb"), false), "TERM=dumb");
    assert!(!animate_ok(true, false, Some("xterm"), true), "--plain");
}

// Value: protects=plain output (pipe, --plain) is byte-identical to the pre-animation output (old output pinned); fails_when=this-week columns or control characters leak into plain output and break scripts; seam=stats(); no_claim=none beyond protects.
#[test]
fn stats_plain_when_not_terminal() {
    let dir = std::env::temp_dir().join(format!("olp-stats-plain-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = olp_hook::stats::stats(&dir).unwrap();
    assert_eq!(
        s,
        "first run: not yet\ndecisions: 0\nstops (ask or block): 0\nafter a card: ran 0 · not run 0\nquiet passes: 0\nclassify answers: 0\nCounted on this device only. Nothing is sent."
    );
    let _ = std::fs::remove_dir_all(&dir);
}
