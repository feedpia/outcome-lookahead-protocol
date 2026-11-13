//! Hook decision: classify, then pass (exit 0) or block (exit 2 plus a card). Claude Code hook contract: 0 = success, 2 = block (stderr goes to Claude),
//! anything else = non-blocking error, so errors, panics and timeouts are all turned into 2 here (fail closed).
//! No server calls. Writes and money movement have no write domain in the engine, so they are always blocked (human confirmation). One exception: in project opt-in quiet mode, a write whose only reason is a declared recovery.

use crate::card;
use crate::classify::{self, Assessment, Effect, Limits};
use crate::config;
use crate::decision;
use crate::pick;
use crate::store::{self, DeviceMap, Entry, LogLine};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

pub const EXIT_ALLOW: i32 = 0;
pub const EXIT_BLOCK: i32 = 2;

#[derive(Debug, Clone)]
pub struct HookOutcome {
    pub code: i32,
    pub stderr: String,
    /// JSON that Claude Code reads on exit 0 (ask). No output when empty.
    pub stdout: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardFail {
    Panic,
    Timeout,
}

/// Runs `f` on a separate thread; if it does not finish within the limit or panics, returns GuardFail. The caller turns that into a block.
pub fn guarded<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static, budget: Option<Duration>) -> Result<T, GuardFail> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new().spawn(move || {
        let r = f();
        let _ = tx.send(r);
    });
    if spawned.is_err() {
        return Err(GuardFail::Panic);
    }
    match budget {
        Some(b) => rx.recv_timeout(b).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => GuardFail::Timeout,
            mpsc::RecvTimeoutError::Disconnected => GuardFail::Panic,
        }),
        None => rx.recv().map_err(|_| GuardFail::Panic),
    }
}

fn blocked_error(project: Option<&Path>, why: &'static str) -> HookOutcome {
    if let Some(p) = project {
        let _ = store::append_log(
            p,
            &LogLine { ts: store::now(), event: "decision", verdict: Some("error_blocked"), unchecked: Some(true), human_choice: Some("not_chosen"), why: Some(why), ..Default::default() },
        );
    }
    HookOutcome { code: EXIT_BLOCK, stderr: card::error_card(why), stdout: String::new() }
}

fn decision_line(a: &Assessment, verdict: &'static str) -> LogLine {
    let effect = a.effect();
    LogLine {
        ts: store::now(),
        event: "decision",
        verdict: Some(verdict),
        effect,
        findings: a.findings.clone(),
        reversible: Some(a.reversible()),
        recovery: Some(if a.recovery_declared() { "declared" } else { "unknown" }),
        unchecked: Some(a.count_unknown() || effect.is_none()),
        classified: Some(effect.is_some()),
        reasons: card::reasons(a),
        human_choice: if verdict == "blocked" { Some("not_chosen") } else { None },
        ..Default::default()
    }
}

/// Permission modes that show an approval prompt (closed list). Anything else (bypassPermissions, dontAsk, missing field, unknown value) may show no prompt, so block.
/// The field name `permission_mode` is a common Claude Code hook input.
// Per the official docs: modes are default, acceptEdits, plan, auto, dontAsk, bypassPermissions; `manual` is an alias of default.
// Only modes documented to show the prompt get ask; the rest (dontAsk, bypassPermissions, unknown) get deny.
// auto (the desktop app default) also routes a hook "ask" to the approval prompt (a classifier does not decide in its place), so ask.
pub const PROMPT_MODES: &[&str] = &["default", "manual", "plan", "acceptEdits", "auto"];

/// The human's keep choice means this command is not run; the reason goes to the agent so it can retry. Exit 0 + deny.
fn deny_outcome(reason: &str) -> HookOutcome {
    // The prompt-side reason always starts with `[OLP]` and the chat line with "OLP hook:", so drop the tag here to avoid overlap.
    let plain = reason.strip_prefix(card::REASON_TAG).unwrap_or(reason);
    let tagged = format!("{}{plain}", card::REASON_TAG);
    let v = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": tagged,
        },
        "systemMessage": format!("OLP hook: {plain}"),
    });
    HookOutcome { code: EXIT_ALLOW, stderr: String::new(), stdout: v.to_string() }
}

/// `reason_extras` = additions for the permission prompt reason (file names only); `extras` = additions for the full systemMessage (names plus pick hints).
/// The desktop app shows the reason in a small scroll box, so file names come first and picker hints go only to systemMessage.
fn ask_outcome(a: &Assessment, reason_extras: &[String], extras: &[String]) -> HookOutcome {
    let v = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "ask",
            "permissionDecisionReason": card::ask_reason_with(a, reason_extras),
        },
        "systemMessage": card::ask_card_with(a, extras),
    });
    HookOutcome { code: EXIT_ALLOW, stderr: String::new(), stdout: v.to_string() }
}

/// PostToolUse: only records that a call whose card was shown (same tool_use_id) has run. Never blocks, no output, errors are silent.
fn post_tool_use(v: &Value, project: Option<&Path>) -> HookOutcome {
    let quiet_ok = HookOutcome { code: EXIT_ALLOW, stderr: String::new(), stdout: String::new() };
    let Some(id) = store::clean_tool_use_id(v.get("tool_use_id").and_then(|x| x.as_str())) else { return quiet_ok };
    let project: PathBuf = match project {
        Some(p) => p.to_path_buf(),
        None => match std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from).or_else(|| v.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from)) {
            Some(p) => p,
            None => return quiet_ok,
        },
    };
    let log = store::read_log(&project);
    let shown = log.iter().any(|l| l["card"] == "card_shown" && l["tool_use_id"] == id.as_str());
    let already = log.iter().any(|l| l["card"] == "executed_after_card" && l["tool_use_id"] == id.as_str());
    if shown && !already {
        let _ = store::append_log(&project, &LogLine { ts: store::now(), event: "post", card: Some("executed_after_card"), tool_use_id: Some(id), ..Default::default() });
    }
    quiet_ok
}

/// Only when there is a file-deletion risk shape and all its counts are numeric (counted paths exist): (first 200 paths, total count). Otherwise None (no picker).
fn delete_files(a: &Assessment) -> Option<(Vec<PathBuf>, u64)> {
    let dels: Vec<&classify::Finding> = a.findings.iter().filter(|f| f.kind == classify::Kind::FileDelete).collect();
    if dels.is_empty() {
        return None;
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    for f in dels {
        total += f.count.n?;
        files.extend(f.count.files.clone()?);
    }
    files.truncate(classify::PICK_CAP);
    Some((files, total))
}

/// Quiet pass = project opted in + no risky shape + not money + the only reason is "declared recovery location present".
fn quiet_passes(a: &Assessment, quiet: bool) -> bool {
    quiet && a.findings.is_empty() && !a.self_guard && a.effect() == Some(Effect::WritesOther) && card::reasons(a) == ["WRITE_RECOVERY_DECLARED"]
}

/// One hook call as `decide` sees it (no tool_use_id, no event name: those only matter to the shell that writes the record).
pub struct HookRequest {
    pub tool: String,
    pub input: Value,
    pub cwd: PathBuf,
    pub mode: Option<String>,
}

/// Which way the hook goes. The shell (`run_hook`) does the matching write and card for each route, in the same order as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Read,
    QuietPass,
    CwdDeny,
    Ask,
    Block { no_prompt_mode: Option<&'static str> },
}

#[derive(Debug, Clone)]
pub struct Decided {
    pub route: Route,
    /// The opt-in decision card may be tried (after CwdDeny, before Ask/Block); if it gives no card the route stands.
    pub decision_candidate: bool,
    pub a: Assessment,
}

/// The verdict with no writes, deletes or network: it only reads the disk to count. Same input and same disk -> same `Decided`.
/// Limits and budget come from the caller (the hook shell passes the project config values).
pub fn decide(req: HookRequest, project: &Path, cfg: &config::Config, map: Arc<DeviceMap>, limits: Limits, budget: Option<Duration>) -> Result<Decided, GuardFail> {
    let HookRequest { tool, input, cwd, mode } = req;
    // Also include the project folder in the comparison set, so deletes of the project folder (settings location) or above are blocked even when the shell is outside the project (inside the decision thread only).
    let anchor = project.to_path_buf();
    let a = guarded(
        move || {
            classify::set_project_anchor(Some(anchor));
            classify::assess(&tool, &input, &map, &cwd, limits)
        },
        budget,
    )?;
    let prompts = mode.as_deref().map(|m| PROMPT_MODES.contains(&m)).unwrap_or(false);
    let only_delete = !a.findings.is_empty() && a.findings.iter().all(|f| f.kind == classify::Kind::FileDelete);
    let decision_candidate = cfg.decision && only_delete && !a.self_guard && !a.unanswerable && a.classified.is_empty() && a.to_classify.is_empty();
    let route = if a.is_read() {
        Route::Read
    } else if quiet_passes(&a, cfg.quiet) {
        Route::QuietPass
    } else if prompts && a.cwd_critical && !a.self_guard {
        Route::CwdDeny
    } else if prompts && !a.self_guard {
        Route::Ask
    } else {
        // If blocked because the mode does not prompt, add a mode hint (known mode names only, else unknown).
        let known: &'static str = match mode.as_deref() {
            Some("bypassPermissions") => "bypassPermissions",
            Some("dontAsk") => "dontAsk",
            _ => "unknown",
        };
        Route::Block { no_prompt_mode: (!prompts).then_some(known) }
    };
    Ok(Decided { route, decision_candidate, a })
}

/// One hook run. If `project` is None, CLAUDE_PROJECT_DIR is the project folder, else the input's cwd.
pub fn run_hook(stdin: &[u8], project: Option<&Path>) -> HookOutcome {
    let start = std::time::Instant::now();
    let v: Value = match serde_json::from_slice(stdin) {
        Ok(v) => v,
        Err(_) => return blocked_error(project, "input is not valid JSON"),
    };
    match v.get("hook_event_name").and_then(|e| e.as_str()) {
        Some("PostToolUse") => return post_tool_use(&v, project),
        Some("PreToolUse") | None => {}
        // Post* events after the tool already ran (PostToolUseFailure etc.) have nothing to block: pass quietly, record nothing (no stats inflation).
        Some(e) if e.starts_with("Post") => return HookOutcome { code: 0, stderr: String::new(), stdout: String::new() },
        Some(_) => return blocked_error(project, "unknown hook event"),
    }
    let mode = v.get("permission_mode").and_then(|m| m.as_str()).map(String::from);
    let tool_use_id = store::clean_tool_use_id(v.get("tool_use_id").and_then(|x| x.as_str()));
    let Some(tool) = v.get("tool_name").and_then(|t| t.as_str()).map(String::from) else {
        return blocked_error(project, "input has no tool_name");
    };
    let input = v.get("tool_input").cloned().unwrap_or(Value::Null);
    let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from) else {
        return blocked_error(project, "input has no cwd");
    };
    let project: PathBuf = match project {
        Some(p) => p.to_path_buf(),
        None => std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from).unwrap_or_else(|| cwd.clone()),
    };
    // Regardless of the pick setting, remove pending/choice files older than 10 minutes (only those with a matching name shape).
    pick::sweep(&project);
    let cfg = match config::load(&project) {
        Ok(c) => c,
        Err(why) => return blocked_error(Some(&project), why),
    };
    let map: DeviceMap = match store::load_map(&project) {
        Ok(m) => m,
        Err(why) => return blocked_error(Some(&project), why),
    };
    store::mark_run(&project);
    let limits = Limits { counter: cfg.counter_limit(), ps_parser: true };
    let decided = decide(HookRequest { tool, input, cwd, mode }, &project, &cfg, Arc::new(map), limits, cfg.hook_budget());
    let Decided { route, decision_candidate, a } = match decided {
        Ok(d) => d,
        Err(GuardFail::Timeout) => return blocked_error(Some(&project), "the hook took longer than hook_budget_ms"),
        Err(GuardFail::Panic) => return blocked_error(Some(&project), "internal error"),
    };
    if route == Route::Read {
        return HookOutcome { code: EXIT_ALLOW, stderr: String::new(), stdout: String::new() };
    }
    if route == Route::QuietPass {
        // Record even when passing. If the record cannot be written, do not pass.
        return match store::append_log(&project, &decision_line(&a, "quiet_pass")) {
            Ok(()) => HookOutcome { code: EXIT_ALLOW, stderr: String::new(), stdout: String::new() },
            Err(_) => HookOutcome { code: EXIT_BLOCK, stderr: card::error_card("quiet pass could not be recorded"), stdout: String::new() },
        };
    }
    // Deleting the working folder or above: the Claude Code built-in prompt discards this card and one Yes deletes it (measured 18/18), so block even in asking modes, with the card and a way to retry.
    // Non-asking modes and self-protection stay as below (exit 2).
    if route == Route::CwdDeny {
        let names: Vec<String> = delete_files(&a).and_then(|(files, total)| pick::names_line(&project, &files, total)).into_iter().collect();
        let reason = format!("{} | {}", card::ask_reason_with(&a, &names), card::CWD_GUIDE);
        let mut line = decision_line(&a, "blocked");
        line.tool_use_id = tool_use_id;
        let _ = store::append_log(&project, &line);
        return deny_outcome(&reason);
    }
    // Decision card (default off): a call containing only deletions with rule 1 is blocked once regardless of mode, and the options go to the agent.
    if decision_candidate {
        if let Some(r) = delete_files(&a).and_then(|(files, total)| decision::check(&project, &files, total, pick::nature_deadline(start, cfg.hook_budget()))) {
            let mut line = decision_line(&a, "decision_asked");
            line.tool_use_id = tool_use_id;
            let _ = store::append_log(&project, &line);
            return deny_outcome(&r);
        }
    }
    // Writes, money, unclassified = Claude Code's own prompt (ask). Modes that may show no prompt, and self-protection = block (exit 2).
    if route == Route::Ask {
        // If a file the human chose to keep is among this delete's targets, do not run, and give a "retry without it" reason (blocking side only; no names in the record).
        if cfg.pick {
            if let Some(kept) = delete_files(&a).and_then(|(files, _)| pick::kept_in_target(&project, &files)) {
                let mut line = decision_line(&a, "kept_by_choice");
                line.tool_use_id = tool_use_id;
                let _ = store::append_log(&project, &line);
                return deny_outcome(&pick::keep_reason(&kept));
            }
        }
        // The decision stays ask; the picker only adds one card line and a pending temp file (when the pick setting is on and olp-gui is running).
        let dels = delete_files(&a);
        let pick_line = match (cfg.pick, pick::gui_alive(&project), &dels) {
            (true, true, Some((files, total))) => pick::card_line(&project, tool_use_id.as_deref(), files, *total, pick::nature_deadline(start, cfg.hook_budget())),
            // Picker enabled but the window is off: show only the command to start it (no address or key).
            (true, false, Some(_)) => Some(pick::GUI_HINT.to_string()),
            _ => None,
        };
        // Up to 3 file names to delete plus "and N more" (card only, not in the record).
        let names = dels.as_ref().and_then(|(files, total)| pick::names_line(&project, files, *total));
        // The reason carries only names plus a "matches/differs your pick" comparison line. Picker hints (PICK_LINE, ANSWER_NO, GUI_HINT) go only to systemMessage.
        let guide = |l: &String| l.starts_with(pick::PICK_LINE) || l == pick::GUI_HINT;
        let reason_extras: Vec<String> = names.iter().chain(pick_line.iter().filter(|l| !guide(l))).cloned().collect();
        let extras: Vec<String> = names.into_iter().chain(pick_line).collect();
        let mut line = decision_line(&a, "asked");
        line.card = Some("card_shown");
        line.tool_use_id = tool_use_id;
        if store::append_log(&project, &line).is_err() {
            // Show the prompt even if the paired record cannot be written (the human decides); a record failure adds one line to the card.
            let mut o = ask_outcome(&a, &reason_extras, &extras);
            o.stderr = "(note: this card could not be recorded on this device)".to_string();
            return o;
        }
        return ask_outcome(&a, &reason_extras, &extras);
    }
    let Route::Block { no_prompt_mode } = route else { unreachable!("every other route returned above") };
    let mut stderr = card::block_card(&a, no_prompt_mode);
    if store::append_log(&project, &decision_line(&a, "blocked")).is_err() {
        stderr.push_str("\n(note: this block could not be recorded on this device)");
    }
    HookOutcome { code: EXIT_BLOCK, stderr, stdout: String::new() }
}

/// Classification command a person runs from their own terminal:
/// `olp-hook classify (--bash "<command>" | --tool <name>) <read_only|writes_other|moves_money> [--reversible] [--recovery-declared] [--db-count <int>]`
/// The mapping table keeps only the grouped key (first word + subcommand); other argument values are discarded.
pub fn classify_cli(project: &Path, args: &[String]) -> Result<String, String> {
    const USAGE: &str = "usage: olp-hook classify (--bash \"<command>\" | --tool <name>) <read_only|writes_other|moves_money> [--reversible] [--recovery-declared] [--db-count <n>]";
    let mut key: Option<String> = None;
    let mut effect: Option<Effect> = None;
    let mut reversible = false;
    let mut declared = false;
    let mut db: Option<u64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--bash" => {
                let cmd = args.get(i + 1).ok_or(USAGE)?;
                let words: Vec<String> = crate::shell::parse(cmd).segments.first().map(|s| s.words.iter().map(|w| w.text.clone()).collect()).unwrap_or_default();
                let fam = classify::family_of(&words).ok_or("empty command")?;
                if classify::family_refused(&fam) {
                    return Err(format!("refused: \"{fam}\" deletes or overwrites (or is olp-hook itself); these forms always ask"));
                }
                // Do not store keys that are not command-name shaped, or block/execution verbs (`}`, `"1mb"`, foreach-object, etc.).
                if !classify::family_is_name(&fam) {
                    return Err(format!("refused: \"{fam}\" is not a command name"));
                }
                key = Some(format!("bash:{fam}"));
                i += 2;
            }
            "--tool" => {
                let t = args.get(i + 1).ok_or(USAGE)?;
                if classify::SHELL_TOOLS.contains(&t.as_str()) || classify::BUILTIN_READ_TOOLS.contains(&t.as_str()) || classify::BUILTIN_OVERWRITE_TOOLS.contains(&t.as_str()) {
                    return Err(format!("refused: {t} is a built-in tool and is handled by the hook itself"));
                }
                key = Some(format!("tool:{t}"));
                i += 2;
            }
            "--reversible" => {
                reversible = true;
                i += 1;
            }
            "--recovery-declared" => {
                declared = true;
                i += 1;
            }
            "--db-count" => {
                let n = args.get(i + 1).ok_or(USAGE)?;
                db = Some(n.parse::<u64>().map_err(|_| "refused: --db-count must be a whole number".to_string())?);
                i += 2;
            }
            other => {
                effect = Some(Effect::parse(other).ok_or(USAGE)?);
                i += 1;
            }
        }
    }
    let (Some(key), Some(effect)) = (key, effect) else { return Err(USAGE.to_string()) };
    let mut map = store::load_map(project).map_err(|e| e.to_string())?;
    let at = store::now();
    let entry = Entry {
        effect,
        reversible,
        recovery: if declared { "declared".into() } else { "unknown".into() },
        declared_at: at,
        db_count: db.map(|n| store::DbCount { n: Value::from(n), declared_at: at }),
    };
    map.version = 1;
    map.entries.insert(key.clone(), entry);
    store::save_map(project, &map).map_err(|_| "could not write .olp/hook-map.json".to_string())?;
    let _ = store::append_log(
        project,
        &LogLine {
            ts: at,
            event: "human_choice",
            effect: Some(effect),
            reversible: Some(reversible && declared),
            recovery: Some(if declared { "declared" } else { "unknown" }),
            human_choice: Some("classified"),
            ..Default::default()
        },
    );
    Ok(format!("classified {key} as {effect:?} for this project"))
}
