//! `olp-hook stats` — on-device tally only (no server). Output is counts and the first-run time only; no paths, raw commands, tool_use_id or signal id.
//! Card pairing: PreToolUse ask = card_shown, a PostToolUse with the same id = executed_after_card, no pair = not run (a refusal cannot be told from an execution failure, so it is honestly "not run").

use crate::store;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;

pub fn stats(project: &Path) -> Result<String, String> {
    let log = store::read_log(project);
    let decisions = log.iter().filter(|l| l["event"] == "decision").count();
    let stops = log.iter().filter(|l| l["event"] == "decision" && ["blocked", "asked", "error_blocked"].iter().any(|v| l["verdict"] == *v)).count();
    let quiet = log.iter().filter(|l| l["verdict"] == "quiet_pass").count();
    let answers = log.iter().filter(|l| l["event"] == "human_choice").count();
    let shown: HashSet<&str> = log.iter().filter(|l| l["card"] == "card_shown").filter_map(|l| l["tool_use_id"].as_str()).collect();
    let shown_no_id = log.iter().filter(|l| l["card"] == "card_shown" && l["tool_use_id"].as_str().is_none()).count();
    let ran: HashSet<&str> = log.iter().filter(|l| l["card"] == "executed_after_card").filter_map(|l| l["tool_use_id"].as_str()).filter(|id| shown.contains(id)).collect();
    let not_run = shown.len() - ran.len() + shown_no_id;
    let first = match store::first_run(project) {
        Some(t) => format!("{t} (unix seconds), {} days ago", store::now().saturating_sub(t) / 86_400),
        None => "not yet".to_string(),
    };
    Ok([
        format!("first run: {first}"),
        format!("decisions: {decisions}"),
        format!("stops (ask or block): {stops}"),
        format!("after a card: ran {} · not run {not_run}", ran.len()),
        format!("quiet passes: {quiet}"),
        format!("classify answers: {answers}"),
        "Counted on this device only. Nothing is sent.".to_string(),
    ]
    .join("\n"))
}

/// This week (today plus the previous 6 UTC days, same day rule as the README) — only values counted from the record.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Week {
    pub cards: u64,
    pub ran: u64,
    pub not_run: u64,
    pub blocked: u64,
    pub files_ran: u64,
    pub files_not_run: u64,
    pub files_blocked: u64,
    pub not_counted: u64,
}

impl Week {
    pub fn files_on_cards(&self) -> u64 {
        self.files_ran + self.files_not_run + self.files_blocked
    }
}

/// File count counted on the device (unit=file, source=counted) and whether non-counted values are mixed in.
pub(crate) fn counted_files(d: &Value) -> (u64, bool) {
    let mut n = 0;
    let mut not_counted = d["unchecked"] == true;
    for c in d["findings"].as_array().into_iter().flatten().map(|f| &f["count"]) {
        if c["unit"] == "file" && c["source"] == "counted" {
            n += c["n"].as_u64().unwrap_or(0);
        } else {
            not_counted = true;
        }
    }
    (n, not_counted)
}

pub fn week(log: &[Value], now: u64) -> Week {
    let first_day = (now / 86_400).saturating_sub(6);
    let in_week = |l: &Value| l["ts"].as_u64().is_some_and(|t| t / 86_400 >= first_day && t <= now);
    let ran_ids: HashSet<&str> = log.iter().filter(|l| l["card"] == "executed_after_card").filter_map(|l| l["tool_use_id"].as_str()).collect();
    let mut w = Week::default();
    for d in log.iter().filter(|l| l["event"] == "decision" && in_week(l)) {
        let is_block = d["verdict"] == "blocked" || d["verdict"] == "error_blocked";
        if d["card"] != "card_shown" && !is_block {
            continue;
        }
        let (n, not_counted) = counted_files(d);
        w.not_counted += u64::from(not_counted);
        if is_block {
            w.blocked += 1;
            w.files_blocked += n;
        } else if d["tool_use_id"].as_str().is_some_and(|id| ran_ids.contains(id)) {
            w.cards += 1;
            w.ran += 1;
            w.files_ran += n;
        } else {
            w.cards += 1;
            w.not_run += 1;
            w.files_not_run += n;
        }
    }
    w
}

/// The animated view only when stdout is a terminal, NO_COLOR is unset, TERM is not dumb and `--plain` is absent. Otherwise plain text.
pub fn animate_ok(is_terminal: bool, no_color: bool, term: Option<&str>, plain: bool) -> bool {
    is_terminal && !no_color && term != Some("dumb") && !plain
}

const BAR: u64 = 30;

/// One-screen frame (ASCII only, no control characters; drawing is the executable's job). Numbers go from 0 to the real value (integer proportion, no overflow); the last frame is the real value.
pub fn frames(w: &Week, steps: usize) -> Vec<String> {
    let steps = steps.clamp(1, 39) as u64;
    let rows = [("Cards in the prompt", w.cards), ("Ran after a card", w.ran), ("Not run after a card", w.not_run), ("Blocked", w.blocked)];
    let max = rows.iter().map(|r| r.1).max().unwrap_or(0);
    let first = if steps == 1 { 1 } else { 0 };
    (first..=steps)
        .map(|i| {
            let at = |v: u64| v * i / steps;
            let mut out = vec!["This week (last 7 UTC days, counted on this device)".to_string()];
            for (name, v) in rows {
                let bar = (at(v) * BAR).checked_div(max).unwrap_or(0);
                out.push(format!("{name:<22}{:>6}  {}", at(v), "#".repeat(bar as usize)));
            }
            out.push(format!(
                "Files on cards (counted on this device): {} - after ran: {}, after not run: {}, blocked: {}",
                at(w.files_on_cards()),
                at(w.files_ran),
                at(w.files_not_run),
                at(w.files_blocked)
            ));
            let k = at(w.not_counted);
            out.push(format!("Not counted: {k} {}", if k == 1 { "card" } else { "cards" }));
            out.join("\n")
        })
        .collect()
}
