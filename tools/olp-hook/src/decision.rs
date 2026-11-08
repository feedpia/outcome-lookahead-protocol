//! Decision card experiment (setting `decision`, default off). Conditions:
//! (1) default off; (2) rule 1: only when there is new information (k >= 1 not in git) and a real choice (counted files >= 2); (3) block cap of 1 per 10 minutes; (4) reason = fixed words plus numbers, never echoing input text.
//! What it does: blocks a deletion once (deny) and gives the agent 3 options; the agent asks the human in a question prompt; the answer becomes the next instruction.
//! It never produces a pass; the next deletion normally gets the regular card or permission prompt.

use crate::{counter, pick};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Block-cap window (seconds). If a block happened within it, the next deletion gets the regular card.
pub const ASKED_S: u64 = 600;

fn marker(project: &Path) -> PathBuf {
    project.join(".olp").join("decision-asked")
}

fn recently_asked(project: &Path) -> bool {
    let Ok(t) = std::fs::metadata(marker(project)).and_then(|m| m.modified()) else { return false };
    let lim = Duration::from_secs(ASKED_S);
    match t.elapsed() {
        Ok(age) => age <= lim,
        Err(_) => true, // A marker from the future (clock skew) counts as just blocked (keeps the block cap).
    }
}

/// Fixed words plus numbers only. Option order: cancel, keep some, delete all (the question prompt's default must not be "delete all", so the first slot deletes nothing).
pub fn reason(n: u64, k: usize) -> String {
    format!(
        "decision: this delete would remove {n} files, and {k} of them are not in git (git cannot bring them back). It did not run. \
Ask the person with AskUserQuestion: one question, these three options in this order (the first one is the default), then do only what they choose. \
1) \"Cancel\": nothing removed. 2) \"Keep some\": ask which files to keep, then delete only the others. 3) \"Delete all {n}\": {n} files removed. \
The next delete will show the usual OLP card."
    )
}

/// Reason when blocking. None = regular card (rule 1, cap, unknown). If the marker cannot be written, do not block (so the cap cannot break).
pub fn check(project: &Path, files: &[PathBuf], total: u64, deadline: Instant) -> Option<String> {
    if total < 2 || files.len() as u64 != total || recently_asked(project) {
        return None;
    }
    let names = pick::rel_names(project, files)?;
    let k = counter::not_in_git(project, &names, deadline)?;
    if k == 0 {
        return None;
    }
    std::fs::write(marker(project), b"").ok()?;
    Some(reason(total, k))
}
