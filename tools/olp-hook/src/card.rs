//! Card text (English, sent to Claude on stderr). The first line is always "before engine check · no receipt" — no receipt number or "OLP check receipt" tag.
//! Numbers are always labelled as counted, user-given, or count not checked.

use crate::classify::{Assessment, Count, Effect, Kind, Source, Unit};

pub const HEADER: &str = "OLP hook: blocked before it ran — before engine check · no receipt";
pub const RERUN: &str = "Checked it? Run it again yourself, outside the agent.";

fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::FileDelete => "delete files",
        Kind::FileOverwrite => "overwrite files",
        Kind::GitForcePush => "force-push over git history",
        Kind::GitResetHard => "discard uncommitted git changes",
        Kind::DbDelete => "delete database rows",
        Kind::DbUpdate => "change database rows",
    }
}

fn unit_word(u: Unit, n: u64) -> String {
    let w = match u {
        Unit::File => "file",
        Unit::Commit => "commit",
        Unit::Change => "change",
        Unit::Row => "row",
    };
    if n == 1 {
        w.to_string()
    } else {
        format!("{w}s")
    }
}

pub fn count_text(c: &Count) -> String {
    match (c.n, c.source) {
        (Some(n), Some(Source::UserDeclared)) => format!("{n} {} (user-given count · not checked by the guard)", unit_word(c.unit, n)),
        // Mark as a preview value: a counted number was seen before execution and is not a promise.
        (Some(n), _) => format!("{n} {} (preview: counted on this device, read-only)", unit_word(c.unit, n)),
        (None, _) => format!("count not checked ({})", c.why_unknown.unwrap_or("unknown")),
    }
}

pub fn reasons(a: &Assessment) -> Vec<&'static str> {
    if a.effect().is_none() {
        return vec![];
    }
    let mut r = vec![];
    if !a.reversible() {
        r.push("WRITE_IRREVERSIBLE");
    }
    r.push(if a.recovery_declared() { "WRITE_RECOVERY_DECLARED" } else { "WRITE_RECOVERY_UNKNOWN" });
    if a.count_unknown() {
        r.push("WRITE_COUNT_UNKNOWN");
    }
    r
}

/// Header of the permission-prompt (ask) card: a check before running, not a block.
pub const ASK_HEADER: &str = "OLP hook: check before it runs — before engine check · no receipt";

/// One line for payment/transfer (classified as money movement): the hook does not read payment arguments, so it says "confirm in the input" instead of an amount (README promise).
pub const AMOUNT_LINE: &str = "Amount: check it in the tool input before you answer (the hook does not read amounts)";

/// The "what it does" lines and the attached "amount" line (also included in the permission prompt reason).
fn what_lines(lines: &[String]) -> Vec<String> {
    lines.iter().skip(1).take_while(|l| l.starts_with("What it does:") || l.starts_with("Amount:")).cloned().collect()
}

/// Permission-prompt card (full text): the block card minus only the "the agent cannot get past this / run again" lines.
pub fn ask_card(a: &Assessment) -> String {
    ask_card_with(a, &[])
}

/// Same as above, with extra lines (file names, picker hints) placed right after the "what it does" lines and before Undo.
pub fn ask_card_with(a: &Assessment, extras: &[String]) -> String {
    let mut lines = card_lines(a);
    lines[0] = ASK_HEADER.to_string();
    let at = 1 + what_lines(&lines).len();
    for (i, e) in extras.iter().enumerate() {
        lines.insert(at + i, e.clone());
    }
    lines.join("\n")
}

/// One-line permission-prompt reason: what it does first, then the tag (Undo goes only in systemMessage).
pub fn ask_reason(a: &Assessment) -> String {
    ask_reason_with(a, &[])
}

/// One-line note for the working-folder delete case.
pub const CWD_GUIDE: &str = "Claude Code shows its own warning first for this shape, so OLP cannot ask here. Run it with a relative path (for example `rm -rf build`) or from your own terminal.";

/// Tag at the very start of the permission-prompt reason, so it is clear at a glance who wrote it. ASCII brackets look the same in any font or window.
pub const REASON_TAG: &str = "[OLP] ";
/// Lead-in words before the first "what it does" (so a first-time reader sees "OLP shows this before it runs").
const BEFORE: &str = "Before this runs: ";
const WHAT: &str = "What it does: ";

/// Same as above, with extra lines (file names, picker hints) placed after the "what it does" (+ amount) lines and before the tag.
/// Always starts with `REASON_TAG`; the first "What it does:" becomes "Before this runs:" and the later "What it does:" words are dropped (no repetition in one line). The chat line (full text) is unchanged.
pub fn ask_reason_with(a: &Assessment, extras: &[String]) -> String {
    let mut first = true;
    let mut parts: Vec<String> = what_lines(&card_lines(a))
        .into_iter()
        .map(|l| match l.strip_prefix(WHAT) {
            Some(rest) if first => {
                first = false;
                format!("{BEFORE}{rest}")
            }
            Some(rest) => rest.to_string(),
            None => l,
        })
        .collect();
    parts.extend(extras.iter().cloned());
    parts.push("before engine check · no receipt".to_string());
    format!("{REASON_TAG}{}", parts.join(" | "))
}

/// When blocking in a non-asking mode: do not hand execution to the person; tell them switching to an asking mode lets them choose in the permission prompt.
/// `mode` = only mode names the hook knows (anything else = unknown; input text is never echoed).
pub fn mode_stop_lines(mode: &str) -> [String; 2] {
    [
        format!("OLP stopped this because this session runs commands without asking (permission mode: {mode})."),
        "Switch to a mode that asks (Shift+Tab in Claude Code), then try again: you answer the permission prompt. Agent: do not ask the person to run this command themselves.".to_string(),
    ]
}

/// `no_prompt_mode` = Some(mode name) means the block is due to a non-asking mode; unless it is self-protection, show the mode hint instead of "run it yourself".
pub fn block_card(a: &Assessment, no_prompt_mode: Option<&str>) -> String {
    let mut out = card_lines(a);
    // The stderr card carries the same tag once: the name slot in the first line "OLP hook: …" is replaced by the tag (no overlap). The chat line (systemMessage) header is unchanged.
    if let Some(first) = out.first_mut() {
        *first = format!("{REASON_TAG}{}", first.strip_prefix("OLP hook: ").unwrap_or(first));
    }
    if !a.findings.is_empty() || a.self_guard || a.unanswerable || !a.classified.is_empty() {
        out.push("The agent cannot pass this card.".to_string());
        match no_prompt_mode {
            Some(m) if !a.self_guard => out.extend(mode_stop_lines(m)),
            _ => out.push(RERUN.to_string()),
        }
    }
    out.join("\n")
}

fn card_lines(a: &Assessment) -> Vec<String> {
    let mut out = vec![HEADER.to_string()];
    for f in &a.findings {
        out.push(format!("What it does: {} — {}", kind_label(f.kind), count_text(&f.count)));
    }
    for c in &a.classified {
        let what = match c.effect {
            Effect::MovesMoney => "moves money",
            Effect::WritesOther => "writes",
            Effect::ReadOnly => "reads",
        };
        out.push(format!("What it does: {what} (classified by you) — count not checked"));
        if c.effect == Effect::MovesMoney {
            out.push(AMOUNT_LINE.to_string());
        }
    }
    if a.self_guard {
        out.push("This touches the OLP hook itself or its files. The agent cannot change them.".to_string());
    }
    if a.effect().is_some() {
        let undo = if a.reversible() {
            "Undo: recovery location declared by you (not checked by the guard)"
        } else if a.recovery_declared() {
            "Undo: not confirmed — recovery location declared by you, but not marked reversible"
        } else {
            "Undo: not confirmed — no recovery location declared"
        };
        out.push(undo.to_string());
        out.push(format!("Why: {}", reasons(a).join(", ")));
    }
    if a.unanswerable {
        out.push("This command builds parts of itself at run time, so it cannot be classified.".to_string());
    }
    if !a.to_classify.is_empty() || a.unnamed {
        out.push("Not classified yet for this project, so it is treated as a write.".to_string());
    }
    if !a.not_recognized.is_empty() {
        // Contains a piece of the command text, so only visible ASCII and within 80 chars (no control characters or long text).
        let shown: String = a.not_recognized.join(", ").chars().map(|c| if (' '..='~').contains(&c) { c } else { '?' }).collect();
        let shown = if shown.chars().count() > 80 { format!("{}...", shown.chars().take(77).collect::<String>()) } else { shown };
        out.push(format!("Not recognized: {shown}"));
    }
    if !a.to_classify.is_empty() {
        out.push("Classify it once in your own terminal (not through the agent), then run it again:".to_string());
        for k in &a.to_classify {
            if let Some(t) = k.strip_prefix("--tool ") {
                out.push(format!("  olp-hook classify --tool {t} read_only"));
            } else {
                out.push(format!("  olp-hook classify --bash \"{k}\" read_only"));
            }
        }
        out.push("  (if it writes: writes_other or moves_money, add --reversible --recovery-declared only if true)".to_string());
    }
    out
}

pub fn error_card(why: &str) -> String {
    format!("{REASON_TAG}blocked — the hook could not decide ({why}). Fail closed: nothing ran.\nFix the cause, or remove the olp-hook line from your Claude Code settings to turn it off.")
}
