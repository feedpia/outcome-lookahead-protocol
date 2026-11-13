//! Shared classification table (used by the hook and a future MCP wrapper). Tool call -> effect (moves_money/writes_other/read_only) + danger kind + count + undo/recovery declaration.
//! Bash has three tiers: (a) six danger kinds (file delete, overwrite, git force push, reset --hard, DB drop, DB update) = write, never passed by a group answer;
//! (b) a closed read-allow list passes; (c) anything else is treated as a write, classified once on first use (group = first word + subcommand).
//! Tools missing from the table are treated as writes, classified once on first use. A verdict never yields "ok": only reads or quiet mode pass (hook.rs).

use crate::counter;
use crate::shell::{self, base, Join, Segment};
use crate::store::{DeviceMap, Entry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    ReadOnly,
    WritesOther,
    MovesMoney,
}

impl Effect {
    pub fn parse(s: &str) -> Option<Effect> {
        match s {
            "read_only" => Some(Effect::ReadOnly),
            "writes_other" => Some(Effect::WritesOther),
            "moves_money" => Some(Effect::MovesMoney),
            _ => None,
        }
    }
}

/// First write kind (only known incident kinds; register new ones before adding).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    FileDelete,
    FileOverwrite,
    GitForcePush,
    GitResetHard,
    DbDelete,
    DbUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    File,
    Commit,
    Change,
    Row,
}

impl Kind {
    pub fn unit(self) -> Unit {
        match self {
            Kind::FileDelete | Kind::FileOverwrite => Unit::File,
            Kind::GitForcePush => Unit::Commit,
            Kind::GitResetHard => Unit::Change,
            Kind::DbDelete | Kind::DbUpdate => Unit::Row,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Count obtained by read-only inspection on this machine
    Counted,
    /// Count the user declared for that table entry (the wrapper does not verify it)
    UserDeclared,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Count {
    /// None = cannot be counted ("unverified")
    pub n: Option<u64>,
    pub unit: Unit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_at: Option<u64>,
    /// Reason the count was not possible (fixed text, shown on the card only, not recorded)
    #[serde(skip)]
    pub why_unknown: Option<&'static str>,
    /// Paths of counted files (first PICK_CAP, direct file-delete form only). Material for a per-file selection UI; not recorded.
    #[serde(skip)]
    pub files: Option<Vec<PathBuf>>,
}

impl Count {
    fn unknown(unit: Unit, why: &'static str) -> Count {
        Count { n: None, unit, source: None, declared_at: None, why_unknown: Some(why), files: None }
    }
    fn counted(unit: Unit, n: u64) -> Count {
        Count { n: Some(n), unit, source: Some(Source::Counted), declared_at: None, why_unknown: None, files: None }
    }
}

/// Upper bound on the pending list.
pub const PICK_CAP: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub kind: Kind,
    pub count: Count,
}

/// Classification result (verdict input). Verdicts and cards live in hook.rs and card.rs.
#[derive(Debug, Clone, Default)]
pub struct Assessment {
    /// Danger kind (if present, always a write and not undoable)
    pub findings: Vec<Finding>,
    /// Entries found in the table (reads excluded)
    pub classified: Vec<Entry>,
    /// Group keys that need classification (something a user can answer once)
    pub to_classify: Vec<String>,
    /// A piece cannot be offered as a classification candidate (not shaped like a command name, or a block/exec verb): no candidate line, still treated as a write like to_classify
    pub unnamed: bool,
    /// Shapes a group answer cannot resolve (command substitution, heredoc, etc.)
    pub unanswerable: bool,
    /// Shapes touching olp-hook itself or its device files (always blocked)
    pub self_guard: bool,
    /// Bash `rm`/`rmdir` target is the working directory itself or an ancestor: the Claude Code built-in critical-path prompt would swallow the OLP card
    pub cwd_critical: bool,
    /// Info line: names that could not be resolved ("Not recognized: ...", never an empty card). Not used in the verdict.
    pub not_recognized: Vec<String>,
}

impl Assessment {
    pub fn is_read(&self) -> bool {
        self.findings.is_empty() && self.classified.is_empty() && self.to_classify.is_empty() && !self.unnamed && !self.unanswerable && !self.self_guard
    }
    pub fn effect(&self) -> Option<Effect> {
        if !self.to_classify.is_empty() || self.unnamed || self.unanswerable || self.self_guard {
            return None;
        }
        let mut e = Effect::ReadOnly;
        if !self.findings.is_empty() {
            e = Effect::WritesOther;
        }
        for c in &self.classified {
            e = e.max(c.effect);
        }
        Some(e)
    }
    pub fn recovery_declared(&self) -> bool {
        !self.classified.is_empty() && self.classified.iter().all(|c| c.recovery_declared())
    }
    /// Undoable = no danger kind + every entry answered "undoable" + a recovery location declared. Without a declaration it is always false.
    pub fn reversible(&self) -> bool {
        self.findings.is_empty() && self.recovery_declared() && self.classified.iter().all(|c| c.reversible)
    }
    pub fn count_unknown(&self) -> bool {
        self.findings.iter().any(|f| f.count.n.is_none())
    }
}

/// Claude Code built-in read tools (closed list). Other tools go through the table or one-time classification.
pub const BUILTIN_READ_TOOLS: &[&str] = &["Read", "Glob", "Grep", "LS", "NotebookRead", "TodoWrite"];
/// Built-in overwrite tools (danger kind = overwrite; cannot be classified per tool).
pub const BUILTIN_OVERWRITE_TOOLS: &[&str] = &["Write", "Edit", "MultiEdit", "NotebookEdit"];
/// Single-command-line tools (cannot be classified per tool, only per command group).
pub const SHELL_TOOLS: &[&str] = &["Bash", "PowerShell"];

const DELETE_VERBS: &[&str] = &["rm", "rmdir", "unlink", "shred", "srm", "del", "erase", "rd", "ri", "remove-item", "trash", "trash-put"];
/// Names whose definition is a delete but that take no file list (0.1.3, DESIGN_OLP_013_RULES §4): a drive format and a recycle-bin clear. Not in DELETE_VERBS, so `count_for` keeps "this form is not counted".
const DELETE_BY_DEFINITION: &[&str] = &["format-volume", "clear-recyclebin"];
const OVERWRITE_VERBS: &[&str] = &["cp", "mv", "dd", "truncate", "tee", "install", "rsync", "ln", "copy-item", "cpi", "move-item", "mi", "set-content", "out-file", "clear-content", "rename-item"];
const WRAPPERS: &[&str] = &["sudo", "doas", "nohup", "time", "nice", "command", "exec", "env"];
const FIND_WRITE_OPTS: &[&str] = &["-delete", "-exec", "-execdir", "-ok", "-okdir", "-fprint", "-fprint0", "-fprintf", "-fls"];
const GIT_READ_SUBS: &[&str] = &["status", "log", "diff", "show", "ls-files", "rev-parse", "blame", "check-ignore", "ls-tree", "cat-file", "merge-base"];
const READ_CMDS: &[&str] = &["ls", "dir", "cat", "head", "tail", "grep", "egrep", "fgrep", "pwd", "echo", "printf", "wc", "which", "type", "stat", "file", "du", "df", "cut", "diff", "cd", "true", "false", "whoami"];
/// Names treated as reads in Bash only, and only as a bare word with no path (READ_CMDS is shared with PowerShell).
/// Shell builtins `[`, test, read, plus sha256sum, md5sum, tr, cmp: no option of these writes files or changes the system (checked against all GNU `--help` options).
/// sleep: only waits, no writes or execution.
const BASH_READ_CMDS: &[&str] = &["[", "test", "read", "sha256sum", "md5sum", "tr", "cmp", "sleep"];
/// Leading Bash grammar words of a piece; the word that follows is the real command.
const BASH_LEAD: &[&str] = &["do", "then", "else", "elif", "if", "while", "until", "!", "{"];
const NULL_SINKS: &[&str] = &["/dev/null", "nul", "/dev/stdout", "/dev/stderr"];
/// Extra read cmdlets for the PowerShell tool only. ForEach-Object and Where-Object run script blocks and are excluded.
const PS_READ_CMDS: &[&str] = &[
    "get-childitem", "gci", "get-item", "gi", "get-content", "gc", "test-path", "measure-object", "measure", "select-object", "select", "sort-object", "sort",
    "format-table", "ft", "format-list", "fl", "out-string", "get-location", "gl", "resolve-path", "rvpa", "split-path", "join-path", "get-filehash",
    "select-string", "sls", "write-output", "write-host", "get-date",
];
/// Names that lead to Remove-Item in PowerShell (including aliases).
const PS_DELETE_VERBS: &[&str] = &["remove-item", "ri", "rm", "rmdir", "rd", "del", "erase"];

/// Count cap (config file value). None = do not count.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub counter: Option<Duration>,
    /// Whether the PowerShell syntax parser may be started (hook: true). The install-time rule check passes false for command-name rules (one powershell.exe per rule was 82 s on the owner PC).
    pub ps_parser: bool,
}

/// Group key: first word (path and .exe stripped, lowercase) + subcommand (letters, digits, `-_:` only, not a flag).
pub fn family_of(words: &[String]) -> Option<String> {
    let w = strip_prefix_words(words);
    let w0 = base(w.first()?);
    if let Some(w1) = w.get(1) {
        if !w1.starts_with('-') && w1.chars().all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c)) {
            return Some(format!("{w0} {}", w1.to_lowercase()));
        }
    }
    Some(w0)
}

/// The word left after stripping leading env assignments (`A=b`), wrappers (sudo, env, etc.) and their flags.
fn strip_prefix_words(words: &[String]) -> &[String] {
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let is_assign = w
            .split_once('=')
            .map(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()))
            .unwrap_or(false);
        // Wrappers (sudo, env, time ...) count only as bare names too: `./sudo ls` runs the `./sudo` file
        let wrapper = |x: &String| !pathed(x) && WRAPPERS.contains(&base(x).as_str());
        if w == "timeout" {
            match timeout_cmd_start(words, i) {
                Some(j) => {
                    i = j;
                    continue;
                }
                None => break,
            }
        }
        if is_assign || wrapper(w) || (i > 0 && w.starts_with('-') && wrapper(&words[i - 1])) {
            i += 1;
        } else {
            break;
        }
    }
    &words[i..]
}

/// Names that run a block or a string: not eligible for one-time classification (`source` behaves like `.`).
const EXEC_VERBS: &[&str] = &["foreach-object", "%", "where-object", "?", "invoke-expression", "iex", "invoke-command", "icm", "start-job", "&", ".", "source"];

/// Whether the group key is shaped like a command name (first word `^[a-z0-9][a-z0-9._+-]*$` with at least one letter, not a block/exec verb). If not, no classification candidate and nothing is stored.
/// A leading digit (`7z`, `2to3`) is allowed because these are real tools. PowerShell words that start with a digit or are not literal are filtered by the caller (assess_shell).
/// Motivation: a PowerShell `{ ... }` block was split into pieces, so `}`, `"1mb"` and `"{0} files=..."` showed up as candidates.
pub fn family_is_name(family: &str) -> bool {
    let w0 = family.split(' ').next().unwrap_or("");
    let mut cs = w0.chars();
    cs.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._+-".contains(c))
        && w0.chars().any(|c| c.is_ascii_lowercase())
        && !EXEC_VERBS.contains(&w0)
}

/// PowerShell numeric literal shape (`10`, `2.5`, `1mb`, `3GB`): `^\d+(\.\d+)?(kb|mb|gb|tb|pb)?$`, case-insensitive.
/// Tools such as `7z` or `2to3` do not match this shape and remain candidates.
fn ps_number_literal(w: &str) -> bool {
    let w = w.to_ascii_lowercase();
    let num = ["kb", "mb", "gb", "tb", "pb"].iter().find_map(|u| w.strip_suffix(u)).unwrap_or(&w);
    let (int, frac) = num.split_once('.').unwrap_or((num, "0"));
    !int.is_empty() && !frac.is_empty() && int.chars().all(|c| c.is_ascii_digit()) && frac.chars().all(|c| c.is_ascii_digit())
}

/// First words that cannot take a group answer (danger-kind verbs and olp-hook itself).
pub fn family_refused(family: &str) -> bool {
    let w0 = family.split(' ').next().unwrap_or("");
    DELETE_VERBS.contains(&w0) || OVERWRITE_VERBS.contains(&w0) || w0 == "olp-hook"
}

/// `safe[i]` = word i cannot expand into other text (no unquoted `$`, glob, `~` or brace). Same length as `words`; missing = not safe.
fn is_read_segment(words: &[String], safe: &[bool], ps: bool) -> bool {
    let Some(w0) = words.first() else { return true };
    // A name with a path (`./ls`, `bin/cat`, `.\echo`, `C:\x\ls.exe`) runs that file, not a PATH tool, so it is never a read name (it cannot be verified as the real tool).
    if pathed(w0) {
        return false;
    }
    let c = base(w0);
    // Shell builtins match only as the bare word (`./test` and `tests/read` would collapse to the same name via base(), but they are real programs).
    if (ps && PS_READ_CMDS.contains(&c.as_str())) || (!ps && BASH_READ_CMDS.contains(&w0.as_str())) {
        return true;
    }
    let rest = &words[1..];
    let arg_safe = |i: usize| plain_arg(&rest[i]) || safe.get(i + 1).copied().unwrap_or(false);
    // Bash names whose read status depends on options: bare word only, and only when every argument is text that cannot expand
    // (no variable, glob, `~` or brace outside quotes; inside single quotes `$` is plain text, so `'$p'` and `'{print $1}'` qualify).
    if !ps && (0..rest.len()).all(arg_safe) {
        match w0.as_str() {
            "sed" => return sed_print_only(rest),
            "awk" | "gawk" | "mawk" => return awk_no_output(rest),
            "sort" => return sort_no_output(rest),
            "uniq" => return uniq_no_output(rest),
            "date" => return date_no_set(rest),
            _ => {}
        }
    }
    match c.as_str() {
        "rg" => !rest.iter().any(|a| a.starts_with("--pre")),
        "find" => !rest.iter().any(|a| FIND_WRITE_OPTS.contains(&a.as_str())),
        "git" => git_read(rest, arg_safe),
        _ => READ_CMDS.contains(&c.as_str()),
    }
}

/// `git [global options] <sub> <args>`: a read only for a read subcommand after these global options:
/// `--no-pager` (only turns the pager off), `-C <folder>` (only changes folder), `-c <key>=<value>` for output-format keys only
/// (`color.*`, `core.quotepath`). Every other global option and config key can name a program (`core.pager`, `alias.*`,
/// `diff.external`, `core.fsmonitor`, `--exec-path` ...), so it asks. `safe(i)` = argument i cannot expand.
fn git_read(rest: &[String], safe: impl Fn(usize) -> bool) -> bool {
    if rest.len() == 1 && rest[0] == "--version" {
        return true;
    }
    let mut j = 0;
    loop {
        match rest.get(j).map(String::as_str) {
            Some("--no-pager") => j += 1,
            Some("-C") => match rest.get(j + 1) {
                Some(d) if safe(j + 1) && !d.starts_with('-') => j += 2,
                _ => return false,
            },
            Some("-c") => match rest.get(j + 1) {
                Some(kv) if safe(j + 1) => {
                    let key = kv.split('=').next().unwrap_or("").to_ascii_lowercase();
                    if !(key.starts_with("color.") || key == "core.quotepath") {
                        return false;
                    }
                    j += 2;
                }
                _ => return false,
            },
            _ => break,
        }
    }
    let Some(sub) = rest.get(j).filter(|s| !s.starts_with('-')) else { return false };
    let args = &rest[j + 1..];
    if sub == "grep" {
        return git_grep_no_program(args);
    }
    GIT_READ_SUBS.contains(&sub.as_str())
        && !args.iter().any(|a| a.starts_with("--output"))
        // cat-file --textconv and --filters launch a program named in repository config.
        && !(sub == "cat-file" && args.iter().any(|a| a.starts_with("--textconv") || a.starts_with("--filters")))
}

/// `git grep` runs a program only through `-O`/`--open-files-in-pager` (any bundle such as `-nO`, `-Ovim`) or `--textconv`
/// (and the removed `--ext-grep`). git accepts unique prefixes of long options, so any prefix of those names asks too
/// (that also catches `--text`; fails closed).
fn git_grep_no_program(args: &[String]) -> bool {
    args.iter().all(|a| match a.strip_prefix("--") {
        Some(long) => {
            let name = long.split('=').next().unwrap_or("");
            name.is_empty() || !["open-files-in-pager", "textconv", "ext-grep"].iter().any(|n| n.starts_with(name))
        }
        None => !(a.starts_with('-') && a.len() > 1 && a[1..].contains('O')),
    })
}

/// Where the command starts in `timeout [options] <duration> command...`. None if the shape does not match (timeout is not stripped, so not a read).
fn timeout_cmd_start(words: &[String], at: usize) -> Option<usize> {
    let mut j = at + 1;
    while let Some(w) = words.get(j).filter(|w| w.starts_with('-')) {
        j += if ["-s", "-k", "--signal", "--kill-after"].contains(&w.as_str()) { 2 } else { 1 };
    }
    let d = words.get(j)?;
    let num = d.trim_end_matches(['s', 'm', 'h', 'd']);
    let ok = !num.is_empty() && num.chars().all(|c| c.is_ascii_digit() || c == '.') && num.chars().filter(|&c| c == '.').count() <= 1;
    (ok && j + 1 < words.len()).then_some(j + 1)
}

/// A path separator (`/` or `\`) in the name means it runs that file in place, not a PATH tool.
fn pathed(name: &str) -> bool {
    name.contains(['/', '\\'])
}

/// Argument has no shell-expanded text (variables, glob, `~`, backtick, braces), so expansion cannot yield an option such as `-i` or `-o`.
fn plain_arg(a: &str) -> bool {
    !a.contains(['$', '*', '?', '[', '`', '~', '{'])
}

/// Only `sed -n '<line number>p|q[;...]' [files...]` is a read. The script contains only digits, `,`, `;`, p and q, so w, W, e, r cannot appear.
/// A single-quoted `$` is the plain last-line address; an unquoted or double-quoted `$` can expand, so it asks (the caller's safe check).
/// GNU sed also reads options after the file (`sed -n 1p f -i`), so any `-`-prefixed argument other than `-n` anywhere is a no.
/// Also `sed -n -e '<script>' [-e '<script>' ...] [files...]`, and the last-line address `$` when it cannot expand (single-quoted).
fn sed_print_only(rest: &[String]) -> bool {
    let cmd_ok = |c: &str| {
        let Some(addr) = c.strip_suffix(['p', 'q']) else { return false };
        let one = |x: &str| x == "$" || (!x.is_empty() && x.chars().all(|d| d.is_ascii_digit()));
        match addr.split_once(',') {
            Some((a, b)) => one(a) && one(b),
            None => one(addr),
        }
    };
    let script_ok = |s: &str| s.trim_end_matches(';').split(';').all(cmd_ok);
    let Some((n, after)) = rest.split_first() else { return false };
    if n != "-n" {
        return false;
    }
    let mut i = 0;
    while after.get(i).is_some_and(|a| a == "-e") {
        match after.get(i + 1) {
            Some(s) if script_ok(s) => i += 2,
            _ => return false,
        }
    }
    if i == 0 {
        match after.first() {
            Some(s) if script_ok(s) => i = 1,
            _ => return false,
        }
    }
    !after[i..].iter().any(|f| f.starts_with('-'))
}

/// `awk [-F sep] [-v name=value] '<program>' [files or name=value ...]` is a read only when the program text has no
/// `system`, `>`, `|` (output redirection, pipes, `cmd | getline`) or `@` (gawk `@include`/`@load`). Every other option
/// asks: `-f`/`-e`/`-i`/`-l` load code, `-d`/`-o`/`-p` write files, `-W`/`-E`/`-M`/`-D` and all long options are outside the list.
/// `||` and comparisons like `NR>2` also ask (fails closed). Operands that start with `-` (other than `-` = stdin) ask.
/// Sources: logs/design/OLP-0.1.2/RESEARCH_read_rules.md (gawk/mawk/BusyBox/POSIX manuals).
fn awk_no_output(rest: &[String]) -> bool {
    let mut i = 0;
    while let Some(a) = rest.get(i).filter(|a| a.starts_with('-')) {
        match a.as_str() {
            "--" => {
                i += 1;
                break;
            }
            "-F" | "-v" if rest.get(i + 1).is_some() => i += 2,
            _ if a.len() > 2 && (a.starts_with("-F") || a.starts_with("-v")) => i += 1,
            _ => return false,
        }
    }
    let Some(program) = rest.get(i) else { return false };
    // gawk `/inet/...` special files open network connections even through `getline <`.
    !program.contains("system") && !program.contains("/inet") && !program.contains(['>', '|', '@']) && !rest[i + 1..].iter().any(|f| f.starts_with('-') && f != "-")
}

/// `sort` only when there is no output file (`-o`, `--output`, including bundled `-ro` and abbreviated `--out`) and no compress program (`--compress-program`, abbreviated `--co`).
/// An `o` inside bundled short options can be a value character (`-to`), but it is treated as a no (fails closed).
fn sort_no_output(rest: &[String]) -> bool {
    rest.iter().all(|a| match a.strip_prefix("--") {
        Some(long) => {
            let name = long.split('=').next().unwrap_or("");
            name.is_empty() || !("output".starts_with(name) || "compress-program".starts_with(name))
        }
        None => !(a.starts_with('-') && a.len() > 1 && a.contains('o')),
    })
}

/// `uniq [options] [input [output]]`: the second positional argument is the output file. Read only with at most one positional (including `-`) and no `--`.
/// Values of value-taking options (`-f N`) also count as positionals, so errors only lean toward asking.
fn uniq_no_output(rest: &[String]) -> bool {
    !rest.iter().any(|a| a == "--") && rest.iter().filter(|a| a.as_str() == "-" || !a.starts_with('-')).count() <= 1
}

/// `date` is a read only without clock setting (`-s`, `s` in bundled short options, `--set`, abbreviated `--s`/`--se`) and without a numeric positional (`MMDDhhmm[[CC]YY][.ss]`).
fn date_no_set(rest: &[String]) -> bool {
    rest.iter().all(|a| match a.strip_prefix("--") {
        Some(long) => {
            let name = long.split('=').next().unwrap_or("");
            name.is_empty() || !"set".starts_with(name)
        }
        None if a.starts_with('-') => !a.contains('s'),
        None => !(a.chars().all(|c| c.is_ascii_digit() || c == '.')),
    })
}

/// Position of the git subcommand (skipping global options `-C x` and `-c x`).
fn git_sub(w: &[String], i: usize) -> Option<usize> {
    let mut j = i + 1;
    while j < w.len() {
        if w[j] == "-c" || w[j] == "-C" {
            j += 2;
        } else if w[j].starts_with('-') {
            j += 1;
        } else {
            return Some(j);
        }
    }
    None
}

/// Find the six danger kinds in deep words (lowercase). Leans toward over-matching (blocking).
pub fn detect_dangers(w: &[String]) -> Vec<Kind> {
    let mut k = vec![];
    let b: Vec<String> = w.iter().map(|x| base(x)).collect();
    for i in 0..w.len() {
        let bi = b[i].as_str();
        if DELETE_VERBS.contains(&bi) {
            k.push(Kind::FileDelete);
        }
        if OVERWRITE_VERBS.contains(&bi) {
            k.push(Kind::FileOverwrite);
        }
        if bi == "find" && w[i..].iter().any(|a| a == "-delete") {
            k.push(Kind::FileDelete);
        }
        if DELETE_BY_DEFINITION.contains(&bi) || (bi == "robocopy" && w[i..].iter().any(|a| a == "/mir" || a == "/purge")) {
            k.push(Kind::FileDelete);
        }
        // `% Delete`, `ForEach-Object Delete`, `ForEach-Object -MemberName Delete`: the method called on each item is Delete.
        if (bi == "%" || bi == "foreach-object") && w[i + 1..].iter().find(|a| *a != "-membername").is_some_and(|a| a == "delete") {
            k.push(Kind::FileDelete);
        }
        if bi == "sed" && w[i..].iter().any(|a| a.starts_with("-i") || a == "--in-place") {
            k.push(Kind::FileOverwrite);
        }
        if bi == "perl" && w[i..].iter().any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains('i')) {
            k.push(Kind::FileOverwrite);
        }
        if bi == "git" {
            if let Some(s) = git_sub(w, i) {
                let rest = &w[s + 1..];
                match w[s].as_str() {
                    "push" => {
                        let force = rest.iter().any(|a| {
                            a == "--force" || a.starts_with("--force") || a == "--mirror" || (a.starts_with('-') && !a.starts_with("--") && a.contains('f')) || (a.starts_with('+') && a.len() > 1)
                        });
                        if force || push_deletes(rest) {
                            k.push(Kind::GitForcePush);
                        }
                    }
                    "reset" if rest.iter().any(|a| a == "--hard") => k.push(Kind::GitResetHard),
                    "clean" if !rest.iter().any(|a| a == "-n" || a == "--dry-run") => k.push(Kind::FileDelete),
                    // Switching branches (`git checkout dev`) is not a danger kind because git refuses to overwrite. Only restoring files or forcing counts as overwrite.
                    "checkout" if rest.iter().any(|a| ["--", ".", "-f", "--force", "-p", "--patch", "--theirs", "--ours"].contains(&a.as_str())) => k.push(Kind::FileOverwrite),
                    "switch" if rest.iter().any(|a| ["-f", "--force", "--discard-changes"].contains(&a.as_str())) => k.push(Kind::FileOverwrite),
                    "restore" => k.push(Kind::FileOverwrite),
                    _ => {}
                }
            }
        }
        // SQL shapes
        if bi == "delete" && w[i + 1..].iter().any(|a| a == "from") {
            k.push(Kind::DbDelete);
        }
        if bi == "drop" && w.get(i + 1).map(|a| ["table", "database", "schema", "view"].contains(&a.as_str())).unwrap_or(false) {
            k.push(Kind::DbDelete);
        }
        if bi == "truncate" && w.get(i + 1).map(|a| a == "table").unwrap_or(false) {
            k.push(Kind::DbDelete);
        }
        if bi == "update" && w[i + 1..].iter().any(|a| a == "set") {
            k.push(Kind::DbUpdate);
        }
    }
    k.sort_by_key(|x| *x as u8);
    k.dedup();
    k
}

/// If a deep word of a non-read piece shows olp-hook itself or the device folder, protect it.
fn touches_self(deep: &[String]) -> bool {
    deep.iter().any(|w| base(w) == "olp-hook" || w.contains(".olp"))
}

fn is_null_sink(t: &str) -> bool {
    NULL_SINKS.contains(&t.to_lowercase().as_str())
}

/// Remote branch delete shapes (`--delete`, `-d`, empty-src refspec `:dev`, `+:dev`) count as the same danger kind as a force push.
fn push_deletes(rest: &[String]) -> bool {
    rest.iter().any(|a| {
        a == "--delete" || (a.starts_with('-') && !a.starts_with("--") && a.contains('d')) || a.trim_start_matches('+').starts_with(':')
    })
}

/// Counting context. One deadline per hook call, fixed at creation so the total stays under the cap even with several findings.
/// The counting base directory can change per piece, so it is passed separately as `Dir`.
struct Ctx {
    deadline: Option<Instant>,
}

impl Ctx {
    fn new(limits: Limits) -> Self {
        Ctx { deadline: limits.counter.map(|d| Instant::now() + d) }
    }
    fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
}

fn count_or_unknown(unit: Unit, r: Result<u64, &'static str>) -> Count {
    match r {
        Ok(n) => Count::counted(unit, n),
        Err(why) => Count::unknown(unit, why),
    }
}

const NO_LIMIT: &str = "no time limit set in .olp/hook-config.json";
const CHANGES_FOLDER: &str = "the command changes folder first";
const FOLDER_NOT_FOUND: &str = "the folder it changes to was not found";
/// Commands that change directory (by first word). Only a literal `cd <one word> &&` is followed; anything else is not counted.
const CD_VERBS: &[&str] = &["cd", "chdir", "pushd", "popd", "set-location", "sl", "push-location", "pop-location"];

/// Counting base directory. Starts at the hook cwd. Once Unknown, it stays Unknown.
#[derive(Debug, Clone)]
enum Dir {
    Known { path: PathBuf, moved: bool },
    Unknown(&'static str),
}

/// Drive letter (`D:`), uppercased if present.
fn drive_of(s: &str) -> Option<char> {
    let mut c = s.chars();
    match (c.next(), c.next()) {
        (Some(d), Some(':')) if d.is_ascii_alphabetic() => Some(d.to_ascii_uppercase()),
        _ => None,
    }
}

/// Base directory after a `cd` piece. Followed only when: first word is a literal `cd`, one literal word, the next separator is `&&`, and that directory exists now.
fn after_cd(dir: &Dir, seg: &Segment, next: Option<Join>, follow: bool) -> Dir {
    let Dir::Known { path, .. } = dir else { return dir.clone() };
    let [w0, w1] = seg.words.as_slice() else { return Dir::Unknown(CHANGES_FOLDER) };
    let t = w1.text.as_str();
    if !follow || w0.text != "cd" || !w1.literal || t.is_empty() || t.starts_with('-') || next.is_some_and(|j| j != Join::AndAnd) {
        return Dir::Unknown(CHANGES_FOLDER);
    }
    // Drive switches, drive-relative forms (`D:work`), and Windows rootless forms (`/x`, `\x`) are not counted (conservative).
    match drive_of(t) {
        Some(d) if !cfg!(windows) || drive_of(&path.to_string_lossy()) != Some(d) => return Dir::Unknown(CHANGES_FOLDER),
        Some(_) if !t[2..].starts_with(['/', '\\']) => return Dir::Unknown(CHANGES_FOLDER),
        None if cfg!(windows) && t.starts_with(['/', '\\']) => return Dir::Unknown(CHANGES_FOLDER),
        _ => {}
    }
    let p = path.join(t);
    if p.is_dir() {
        Dir::Known { path: p, moved: true }
    } else {
        Dir::Unknown(FOLDER_NOT_FOUND)
    }
}

/// Base directory after a PowerShell `cd`, `chdir`, `Set-Location` or `sl` piece.
/// If the target is the **same real directory** as the current base (canonicalize comparison), keep it: even if the cd fails the directory is unchanged, so the separator does not matter.
/// Anything else (other directory, subdirectory, variable, `~`, drive-relative, stack verbs, canonicalize failure) stays Unknown. Bash does not use this function.
fn ps_after_cd(dir: &Dir, seg: &Segment) -> Dir {
    let Dir::Known { path, moved } = dir else { return dir.clone() };
    let [w0, w1] = seg.words.as_slice() else { return Dir::Unknown(CHANGES_FOLDER) };
    let t = w1.text.replace('\\', "/");
    let drive_relative = drive_of(&t).is_some() && !t[2..].starts_with('/');
    if !["cd", "chdir", "set-location", "sl"].contains(&base(&w0.text).as_str()) || !w1.literal || t.is_empty() || t.starts_with('-') || drive_relative {
        return Dir::Unknown(CHANGES_FOLDER);
    }
    match (std::fs::canonicalize(path.join(&t)), std::fs::canonicalize(path)) {
        (Ok(a), Ok(b)) if a == b => Dir::Known { path: path.clone(), moved: *moved },
        _ => Dir::Unknown(CHANGES_FOLDER),
    }
}

/// If the base directory is inside `.olp`, protect it.
fn dir_in_olp(dir: &Dir) -> bool {
    matches!(dir, Dir::Known { path, moved: true } if path.components().any(|c| c.as_os_str() == ".olp"))
}

/// Count for one danger kind. Counted only for the direct form (first word is that verb); everything else is unknown.
fn count_for(kind: Kind, words: &[String], seg: &Segment, entry: Option<&Entry>, ctx: &Ctx, dir: &Dir, ps: bool) -> Count {
    let unit = kind.unit();
    if matches!(kind, Kind::DbDelete | Kind::DbUpdate) {
        // The DB is never connected to and no command is run. Only the integer the user declared is user_declared.
        return match entry.and_then(|e| e.user_db_count()) {
            Some((n, at)) => Count { n: Some(n), unit, source: Some(Source::UserDeclared), declared_at: Some(at), why_unknown: None, files: None },
            None => Count::unknown(unit, "database rows are not counted by the guard"),
        };
    }
    let cwd = match dir {
        Dir::Known { path, .. } => path.as_path(),
        Dir::Unknown(why) => return Count::unknown(unit, why),
    };
    let Some(deadline) = ctx.deadline() else { return Count::unknown(unit, NO_LIMIT) };
    let w = strip_prefix_words(words);
    let w0 = w.first().map(|x| base(x)).unwrap_or_default();
    match kind {
        Kind::FileDelete if ps && PS_DELETE_VERBS.contains(&w0.as_str()) => match ps_delete_paths(w, seg, words.len() - w.len()).and_then(|a| ps_split_commas(cwd, a)) {
            Ok(args) => match counter::list_paths(cwd, &args.iter().map(String::as_str).collect::<Vec<_>>(), deadline, PICK_CAP) {
                Ok((files, n)) => Count { files: Some(files), ..Count::counted(unit, n) },
                Err(why) => Count::unknown(unit, why),
            },
            Err(why) => Count::unknown(unit, why),
        },
        Kind::FileDelete if !ps && ["rm", "unlink", "shred", "srm", "rmdir"].contains(&w0.as_str()) => {
            let mut args: Vec<&str> = vec![];
            let mut opts_done = false;
            for (i, a) in w.iter().enumerate().skip(1) {
                if !opts_done && a.text_is_flag() {
                    if a == "--" {
                        opts_done = true;
                    }
                    continue;
                }
                if !seg_literal(seg, words.len() - w.len() + i) {
                    return Count::unknown(unit, "the paths use patterns or variables");
                }
                args.push(a.as_str());
            }
            match counter::list_paths(cwd, &args, deadline, PICK_CAP) {
                Ok((files, n)) => Count { files: Some(files), ..Count::counted(unit, n) },
                Err(why) => Count::unknown(unit, why),
            }
        }
        Kind::GitForcePush if push_deletes(&w[1..]) => Count::unknown(unit, "remote branch deletion is not counted"),
        Kind::GitForcePush if w0 == "git" && w.get(1).map(|s| s == "push").unwrap_or(false) => match push_range(&w[2..]) {
            Some(r) => count_or_unknown(unit, counter::count_commits(cwd, &r, deadline)),
            None => Count::unknown(unit, "remote and branch are not both given"),
        },
        Kind::GitResetHard if w0 == "git" && w.get(1).map(|s| s == "reset").unwrap_or(false) => count_or_unknown(unit, counter::count_uncommitted_changes(cwd, deadline)),
        Kind::FileDelete if runtime_target(w) => Count::unknown(unit, RUNTIME_TARGET),
        _ => Count::unknown(unit, "this form is not counted"),
    }
}

/// A delete inside a `sh`/`bash`/`zsh`/`dash` `-c` script whose target is a `$` value (also after `xargs` or `find -exec`): the target is only known when it runs.
fn runtime_target(w: &[String]) -> bool {
    w.windows(3).any(|t| {
        ["sh", "bash", "zsh", "dash"].contains(&base(&t[0]).as_str()) && t[1] == "-c" && t[2].contains('$') && detect_dangers(&shell::deep_words([t[2].as_str()])).contains(&Kind::FileDelete)
    })
}

const PATTERNS: &str = "the paths use patterns or variables";
const NOT_COUNTED: &str = "this form is not counted";
/// 0.1.3 ⑥-a reason (same `count not checked (reason)` frame, card.rs unchanged). Wording pending the two native-speaker checks (REVIEW_013_DESIGN_RULES Q4).
const RUNTIME_TARGET: &str = "the target is only known at run time";

/// Path arguments of PowerShell `Remove-Item` (including aliases). Only known switches, `-Path`/`-LiteralPath` values and `-ErrorAction`-style values are skipped over;
/// other parameters (-WhatIf, -Filter, -Include, -Exclude, etc.), cmd-style `/s` and piped input (no path) are not counted (no invented numbers). Comma arrays are split by `ps_split_commas`.
/// `off` = position in `words` where `w` starts (for the literal check). Returned paths have `\` converted to `/` (ps_path).
fn ps_delete_paths(w: &[String], seg: &Segment, off: usize) -> Result<Vec<String>, &'static str> {
    let mut paths = vec![];
    let mut i = 1;
    while i < w.len() {
        let a = w[i].as_str();
        if a.starts_with('-') && a.len() > 1 {
            let (name, inline) = match a[1..].split_once(':') {
                Some((n, _)) => (n.to_lowercase(), true),
                None => (a[1..].to_lowercase(), false),
            };
            match name.as_str() {
                "recurse" | "r" | "force" | "confirm" | "verbose" | "debug" => i += 1,
                "erroraction" | "ea" | "warningaction" | "wa" | "informationaction" | "infa" | "progressaction" | "proga" => i += if inline { 1 } else { 2 },
                "path" | "literalpath" | "lp" | "pspath" if !inline => {
                    let v = w.get(i + 1).ok_or(NOT_COUNTED)?;
                    paths.push(ps_path(v, seg, off + i + 1)?);
                    i += 2;
                }
                _ => return Err(NOT_COUNTED),
            }
            continue;
        }
        if a.starts_with('/') {
            return Err(NOT_COUNTED);
        }
        paths.push(ps_path(a, seg, off + i)?);
        i += 1;
    }
    if paths.is_empty() {
        return Err(NOT_COUNTED);
    }
    Ok(paths)
}

/// PowerShell accepts `\` as a path separator on every OS, so convert to `/` (Windows accepts `/` too).
/// Without this, Linux and macOS count `.\build` as a nonexistent file (0 files).
/// PowerShell comma arrays (`a, b`, `a,b`) -> several paths. Observed: an agent deleted two paths this way and the card showed 0.
/// If a real name contains a comma (`x,y.txt`): quoted = that one name, unquoted = an array. The parser does not remember quotes, so it cannot tell, and nothing is counted.
/// Zero pieces means nothing is counted.
fn ps_split_commas(cwd: &Path, paths: Vec<String>) -> Result<Vec<String>, &'static str> {
    let mut out = vec![];
    for p in paths {
        if !p.contains(',') {
            out.push(p);
            continue;
        }
        if cwd.join(&p).exists() {
            return Err(NOT_COUNTED);
        }
        out.extend(p.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from));
    }
    if out.is_empty() {
        return Err(NOT_COUNTED);
    }
    Ok(out)
}

/// Comma arrays (`a, b`) are not rejected here; `ps_split_commas` splits them when counting.
fn ps_path(a: &str, seg: &Segment, idx: usize) -> Result<String, &'static str> {
    // Non-file PowerShell provider paths (HKCU:, Env:, Registry::, Cert:, etc.) would be counted as "nonexistent file = 0", a wrong number, so they are not counted.
    // The only `:` allowed is a single drive letter (`C:\x`, `C:/x`).
    let drive_only = drive_of(a).is_some() && a.matches(':').count() == 1 && a[2..].starts_with(['/', '\\']);
    if a.contains(':') && !drive_only {
        return Err(NOT_COUNTED);
    }
    if !seg_literal(seg, idx) {
        return Err(PATTERNS);
    }
    Ok(a.replace('\\', "/"))
}

trait FlagExt {
    fn text_is_flag(&self) -> bool;
}
impl FlagExt for String {
    fn text_is_flag(&self) -> bool {
        self.starts_with('-') && self.len() > 1
    }
}

fn seg_literal(seg: &Segment, idx: usize) -> bool {
    seg.words.get(idx).map(|w| w.literal).unwrap_or(false)
}

/// Bash `rm`/`rmdir`: any target that is the working directory (hook cwd; Claude Code supplies it after the shell `cd`) or an ancestor of it.
/// A `cd ... &&` in the same command is used only as the base for resolving relative paths.
/// Target shapes: literal paths (absolute, `.`, `..`), `~`, `$PWD`/`${PWD}`. Other variables and patterns are not counted (still ask).
/// If the path cannot be determined (`~` with unknown home), treat it as a match (fails closed). Disk access is canonicalize only (read-only).
fn rm_hits_working_folder(words: &[String], seg: &Segment, dir: &Dir, cwd: &Path) -> bool {
    let w = strip_prefix_words(words);
    if !w.first().is_some_and(|x| ["rm", "rmdir"].contains(&base(x).as_str())) {
        return false;
    }
    let off = words.len() - w.len();
    let shell_dir = match dir {
        Dir::Known { path, .. } => path.as_path(),
        Dir::Unknown(_) => cwd,
    };
    let mut opts_done = false;
    for (i, a) in w.iter().enumerate().skip(1) {
        if !opts_done && a.text_is_flag() {
            opts_done = a == "--";
            continue;
        }
        let (text, from_pwd) = if seg_literal(seg, off + i) || a.starts_with('~') {
            (a.clone(), false)
        } else if let Some(rest) = a.strip_prefix("${PWD}").or_else(|| a.strip_prefix("$PWD")).filter(|r| r.is_empty() || r.starts_with('/')) {
            (format!("{}{rest}", shell_dir.to_string_lossy()), true)
        } else {
            continue;
        };
        // If the directory after a preceding `cd` is unknown (e.g. joined by `;`), block only relative targets that could be the cwd or an ancestor:
        // `$PWD`; a whole path (after stripping leading `./`) that is `.`, `..` or `../...`; a first word that names the project or an ancestor (`cd ..; rm -rf proj`).
        // Subdirectory names like `build`, `.cache`, `./build` still just ask (ordinary commands; avoid over-blocking).
        let anchored = text.starts_with(['/', '\\', '~']) || drive_of(&text).is_some();
        if matches!(dir, Dir::Unknown(_)) && !anchored {
            let mut rel = text.replace('\\', "/");
            while let Some(r) = rel.strip_prefix("./") {
                rel = r.trim_start_matches('/').to_string();
            }
            let up_or_here = rel.is_empty() || rel == "." || rel == ".." || rel.starts_with("../");
            let first = rel.split('/').next().unwrap_or("");
            // Name matching uses the project folder (if known) and its ancestors, so a hook cwd left in a subdirectory (build) by an earlier command does not let `cd <project>; rm -rf build` through.
            let base_dir = PROJECT_ANCHOR.with(|p| p.borrow().clone()).unwrap_or_else(|| cwd.to_path_buf());
            let names_cwd = base_dir.ancestors().filter_map(|a| a.file_name()).any(|n| n.to_string_lossy().eq_ignore_ascii_case(first));
            if from_pwd || up_or_here || names_cwd {
                return true;
            }
        }
        if matches!(dir, Dir::Unknown(_)) && from_pwd {
            return true;
        }
        // Relative paths and `$PWD` resolve against the folder after `cd`, but comparison is only against the hook cwd (the Claude Code working directory): `cd build && rm -rf .` is inside the working directory.
        let Some(target) = resolve_rm_target(&text, shell_dir) else { return true };
        // The home directory itself has the same built-in prompt shape (even if not an ancestor of the cwd). Root is always an ancestor of the cwd, so it falls into the comparison below.
        let is_home = home_dir().is_some_and(|h| same_or_parent(&target, &h) && same_or_parent(&h, &target));
        let in_project = PROJECT_ANCHOR.with(|p| p.borrow().as_deref().is_some_and(|pr| same_or_parent(&target, pr)));
        // If a same-line `cd X &&` moved up (X is an ancestor of or equal to the hook cwd), the working directory is X. Even if the hook cwd was left in a subdirectory (build) by an earlier command,
        // `cd <project> && rm -rf build` is a subdirectory delete. A `cd build && rm -rf .` that moves down is still judged against the hook cwd.
        let here = match dir {
            Dir::Known { path, moved: true } if same_or_parent(path, cwd) => path.as_path(),
            _ => cwd,
        };
        if is_home || in_project || same_or_parent(&target, here) {
            return true;
        }
    }
    false
}

thread_local! {
    /// Project folder to compare along with the hook cwd (location of settings.json). Set only by the hook inside its verdict scope; other callers pass None (unchanged behavior).
    static PROJECT_ANCHOR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Lets the following `assess` also treat deleting the project folder or an ancestor like deleting the working directory (hook only).
pub fn set_project_anchor(project: Option<PathBuf>) {
    PROJECT_ANCHOR.with(|p| *p.borrow_mut() = project);
}

/// On Windows, Git Bash form `/d/...` (or `/d`) -> `D:/...`. Anything else = None.
fn git_bash_path(t: &str) -> Option<PathBuf> {
    let b = t.as_bytes();
    (cfg!(windows) && b.len() >= 2 && b[0] == b'/' && b[1].is_ascii_alphabetic() && (b.len() == 2 || b[2] == b'/'))
        .then(|| PathBuf::from(format!("{}:/{}", t[1..2].to_ascii_uppercase(), t.get(3..).unwrap_or(""))))
}

/// Home directory (`HOME`, else `USERPROFILE`). Also handles Git Bash style HOME.
fn home_dir() -> Option<PathBuf> {
    let h = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let s = h.to_string_lossy();
    Some(git_bash_path(&s).unwrap_or_else(|| PathBuf::from(&h)))
}

/// Resolve `~`, Git Bash `/d/...` and relative paths to an absolute path against `base` (only `.` and `..` normalized textually). None if home is unknown.
fn resolve_rm_target(t: &str, base: &Path) -> Option<PathBuf> {
    let p = if t == "~" || t.starts_with("~/") {
        home_dir()?.join(t.trim_start_matches('~').trim_start_matches('/'))
    } else if t.starts_with('~') {
        return None; // `~user`: home of an unknown user
    } else if let Some(g) = git_bash_path(t) {
        g
    } else {
        base.join(t)
    };
    // If the path exists, use the real path (so `..` after a link resolves like the shell does); otherwise normalize `.` and `..` textually.
    if let Ok(real) = std::fs::canonicalize(&p) {
        return Some(real);
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    Some(out)
}

/// Whether `target` equals or is an ancestor of `here`, using real paths (symlinks resolved), falling back to textual paths. On Windows, ignores case, the verbatim prefix and `\`.
fn same_or_parent(target: &Path, here: &Path) -> bool {
    let key = |p: &Path| -> PathBuf {
        let real = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if cfg!(windows) {
            let s = real.to_string_lossy().replace('\\', "/").to_lowercase();
            PathBuf::from(s.strip_prefix("//?/").map(str::to_string).unwrap_or(s))
        } else {
            real
        }
    };
    key(here).starts_with(key(target))
}

/// `git push [flags] <remote> <[+]src[:dst]>` -> `src..refs/remotes/<remote>/<dst>` (commits that would vanish from the remote = those not present locally).
fn push_range(args: &[String]) -> Option<String> {
    let value_opts = ["-o", "--push-option", "--repo", "--receive-pack", "--exec"];
    if args.iter().any(|a| value_opts.iter().any(|v| a == v || a.starts_with(&format!("{v}=")))) {
        return None;
    }
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if pos.len() != 2 {
        return None;
    }
    let remote = pos[0];
    let spec = pos[1].trim_start_matches('+');
    let (src, dst) = spec.split_once(':').unwrap_or((spec, spec));
    let ok = |s: &str| !s.is_empty() && !s.starts_with('-') && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c));
    if !(ok(remote) && ok(src) && ok(dst)) {
        return None;
    }
    let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
    Some(format!("{src}..refs/remotes/{remote}/{dst}"))
}

/// Classify one Bash command line.
pub fn assess_bash(cmd: &str, map: &DeviceMap, cwd: &Path, limits: Limits) -> Assessment {
    assess_shell(cmd, map, cwd, limits, false)
}

/// `ps` = PowerShell tool: backslash paths, Remove-Item counting, read cmdlets. Following `cd <dir> &&` is Bash only (PowerShell is never counted).
fn assess_shell(cmd: &str, map: &DeviceMap, cwd: &Path, limits: Limits, ps: bool) -> Assessment {
    let parsed = if ps { shell::parse_ps(cmd) } else { shell::parse(cmd) };
    if parsed.complex {
        return builds_itself(cmd);
    }
    if parsed.substs.is_empty() {
        return assess_parsed(cmd, &parsed, map, cwd, limits, ps);
    }
    // A read-only `$(...)` is expanded and inspected: it passes only if outside and inside (depth 1; nested is complex per the parser) are all reads, the substitution is not in a command-name slot or in the arguments of a command that reads options,
    // and it does not touch olp-hook or .olp. If any condition fails, the current verdict (builds_itself) stands, with the same card text and numbers.
    let outer = assess_parsed(cmd, &parsed, map, cwd, limits, ps);
    let ok = outer.is_read()
        && subst_places_ok(&parsed)
        && !touches_self(&shell::deep_words([cmd]))
        && parsed.substs.iter().all(|s| assess_shell(s, map, cwd, limits, ps).is_read());
    if ok {
        outer
    } else {
        builds_itself(cmd)
    }
}

/// Command substitution, heredoc, etc.: scan the whole text deeply for danger kinds only; otherwise block as not answerable by a group answer.
fn builds_itself(cmd: &str) -> Assessment {
    let mut a = Assessment::default();
    let deep = shell::deep_words([cmd]);
    for k in detect_dangers(&deep) {
        a.findings.push(Finding { kind: k, count: Count::unknown(k.unit(), "the command builds parts of itself at run time") });
    }
    a.self_guard |= touches_self(&deep);
    a.unanswerable = true;
    a
}

/// Position of `$(...)`. A command-name slot (`$(echo rm) -rf x`, `env $(...) ls`) is a no.
/// An argument slot qualifies only for commands that are reads by name alone (READ_CMDS, BASH_READ_CMDS); commands like find, git and rg decide by arguments, so
/// the substitution result could become an option (`-delete`, `--output=`, `--pre=`) and it is a no. A `for NAME in $(...)` header executes nothing, so it is a yes.
fn subst_places_ok(p: &shell::Parsed) -> bool {
    p.segments.iter().all(|seg| {
        let words: Vec<String> = seg.words.iter().map(|w| w.text.clone()).collect();
        let stripped = strip_prefix_words(&words);
        let Some(o) = bash_cmd_start(stripped) else { return true };
        let (name, args) = (&stripped[o], &stripped[o + 1..]);
        let b = base(name);
        !name.contains("$(") && (!args.iter().any(|w| w.contains("$(")) || READ_CMDS.contains(&b.as_str()) || BASH_READ_CMDS.contains(&b.as_str()))
    })
}

fn assess_parsed(cmd: &str, parsed: &shell::Parsed, map: &DeviceMap, cwd: &Path, limits: Limits, ps: bool) -> Assessment {
    let follow_cd = !ps;
    let mut exec_names: Vec<String> = Vec::new();
    let ctx = Ctx::new(limits);
    let mut a = Assessment::default();
    let mut dir = Dir::Known { path: cwd.to_path_buf(), moved: false };
    let segs = &parsed.segments;
    for (i, seg) in segs.iter().enumerate() {
        // After a directory change, a separator other than `&&` (;, ||, |, newline, &) runs even if the earlier part failed, so the directory is unknown
        if matches!(dir, Dir::Known { moved: true, .. }) && seg.joined_by != Join::AndAnd {
            dir = Dir::Unknown(CHANGES_FOLDER);
        }
        let in_olp = dir_in_olp(&dir);
        let words: Vec<String> = seg.words.iter().map(|w| w.text.clone()).collect();
        for r in &seg.redirects {
            match r {
                Some(t) if is_null_sink(t) => {}
                Some(t) => {
                    a.self_guard |= t.contains(".olp") || in_olp;
                    let c = match (&dir, ctx.deadline()) {
                        (Dir::Unknown(why), _) => Count::unknown(Unit::File, why),
                        (Dir::Known { path, .. }, Some(d)) => count_or_unknown(Unit::File, counter::count_paths(path, &[t.as_str()], d)),
                        (Dir::Known { .. }, None) => Count::unknown(Unit::File, NO_LIMIT),
                    };
                    a.findings.push(Finding { kind: Kind::FileOverwrite, count: c });
                }
                None => a.findings.push(Finding { kind: Kind::FileOverwrite, count: Count::unknown(Unit::File, "the target uses patterns or variables") }),
            }
        }
        let stripped = strip_prefix_words(&words);
        if stripped.is_empty() {
            continue;
        }
        if base(&stripped[0]) == "olp-hook" {
            a.self_guard = true;
            continue;
        }
        let b0 = base(&stripped[0]);
        let changes_dir = CD_VERBS.contains(&b0.as_str()) || (b0 == "builtin" && stripped.get(1).is_some_and(|w| CD_VERBS.contains(&base(w).as_str())));
        if changes_dir {
            dir = if ps { ps_after_cd(&dir, seg) } else { after_cd(&dir, seg, segs.get(i + 1).map(|s| s.joined_by), follow_cd) };
        }
        // PowerShell pieces are split at control words and braces and each command is inspected separately ("if" is not a command name; the command after `... } else { mytool ...` is inspected too)
        let start = words.len() - stripped.len();
        let parts: Vec<(usize, &[String])> = if ps { ps_parts(stripped).into_iter().map(|(o, p)| (start + o, p)).collect() } else { bash_cmd_start(stripped).map(|o| (start + o, &stripped[o..])).into_iter().collect() };
        let safe: Vec<bool> = seg.words.iter().map(|w| w.literal && !w.brace).collect();
        let unread: Vec<(usize, &[String])> = parts.into_iter().filter(|(at, p)| !is_read_segment(p, safe.get(*at..*at + p.len()).unwrap_or(&[]), ps) && !(ps && ps_string_output(seg, *at, p))).collect();
        let Some(&(first_at, first)) = unread.first() else { continue };
        let deep = shell::deep_words(words.iter().map(|s| s.as_str()));
        a.self_guard |= touches_self(&deep) || in_olp;
        let fam = family_of(first).unwrap_or_default();
        let key = format!("bash:{fam}");
        let entry = map.entries.get(&key);
        let dangers = detect_dangers(&deep);
        if !dangers.is_empty() {
            for k in dangers {
                if k == Kind::FileDelete && !ps {
                    a.cwd_critical |= rm_hits_working_folder(&words, seg, &dir, cwd);
                }
                let c = count_for(k, &words, seg, entry, &ctx, &dir, ps);
                a.findings.push(Finding { kind: k, count: c });
            }
            continue;
        }
        // Groups not shaped like a name are used neither as candidates nor for table reads (previously stored meaningless keys are ignored too); they are only treated as writes
        // + first word not literal (`$s.Sum/1MB` -> base `1mb`) or a PowerShell digit-leading word (numeric expression) = not a name (prevents regression on known cases)
        // PowerShell inspects each non-read command separately (earlier commands are handled like fam and entry above)
        let fams: Vec<(String, usize)> = if ps { unread.iter().map(|(at, p)| (family_of(p).unwrap_or_default(), *at)).collect() } else { vec![(fam, first_at)] };
        for (fam, at) in fams {
            let first_literal = seg_literal(seg, at);
            let ps_number = ps && ps_number_literal(fam.split(' ').next().unwrap_or(""));
            // A path-qualified first word (`./mytool`, `bin/mytool`) runs that file, so it never takes the bare name's answer (review memo M1).
            let pathed_name = seg.words.get(at).is_some_and(|w| pathed(&w.text));
            if !family_is_name(&fam) || !first_literal || ps_number || pathed_name {
                a.unnamed = true;
                // PowerShell info line (when the parser is unavailable): block/exec verbs are shown because the name is human-readable; meaningless pieces (`"1mb"`, `$x`) give 0
                if let Some(w) = seg.words.get(at).filter(|w| EXEC_VERBS.contains(&base(&w.text).as_str())) {
                    if !exec_names.contains(&w.text) {
                        exec_names.push(w.text.clone());
                    }
                }
                continue;
            }
            match map.entries.get(&format!("bash:{fam}")) {
                Some(e) if e.effect == Effect::ReadOnly => {}
                Some(e) => a.classified.push(e.clone()),
                None => {
                    if !a.to_classify.contains(&fam) {
                        a.to_classify.push(fam);
                    }
                }
            }
        }
    }
    // PowerShell hybrid: when the Rust analysis ends only in "no name, unclassified" (no danger kind, table entry or self-protection), re-read with the PowerShell syntax parser.
    // Only parsed reads pass, risky methods get a card, unknown things still ask. If the parser is unavailable, the current verdict plus an info line.
    if ps && limits.ps_parser && a.findings.is_empty() && a.classified.is_empty() && !a.self_guard && !a.unanswerable && (a.unnamed || !a.to_classify.is_empty()) {
        match crate::ps_ast::parse(cmd) {
            Some(facts) => {
                let (kinds, unknown) = judge_ps_facts(&facts);
                if kinds.is_empty() && unknown.is_empty() {
                    return Assessment::default();
                }
                for (k, why) in kinds {
                    if !a.findings.iter().any(|f| f.kind == k) {
                        a.findings.push(Finding { kind: k, count: Count::unknown(k.unit(), why) });
                    }
                }
                if unknown.is_empty() {
                    a.unnamed = false;
                    a.to_classify.clear();
                } else {
                    a.not_recognized = unknown;
                }
            }
            None => a.not_recognized = exec_names,
        }
    }
    a
}

/// Safe methods (string/value handling only). Methods outside this list are unknown (ask).
/// `replace` is in neither the safe nor the write list: from the name alone it is unclear whether it is a string replace or `FileInfo.Replace`, so it is unknown.
const PS_SAFE_METHODS: &[&str] = &[
    "tostring", "tolower", "toupper", "tolowerinvariant", "toupperinvariant", "trim", "trimstart", "trimend", "split", "contains", "startswith", "endswith",
    "substring", "indexof", "lastindexof", "padleft", "padright", "equals", "gettype", "compareto", "gethashcode", "tochararray", "format", "join", "round", "floor", "ceiling",
];
/// Methods that change files or folders (instance and static): delete = `delete*`, others = overwrite shape.
const PS_WRITE_METHODS: &[&str] = &[
    "moveto", "copyto", "create", "createsubdirectory", "createtext", "appendtext", "openwrite", "encrypt", "decrypt", "setaccesscontrol", "move", "copy",
    "writealltext", "writealllines", "writeallbytes", "appendalltext", "appendalllines", "setattributes", "setlastwritetime", "setcreationtime",
];
/// Read-only static calls: read methods of `[IO.File]` and `[IO.Directory]`, and string/number types.
const PS_STATIC_READ: &[&str] = &["exists", "readalltext", "readalllines", "readallbytes", "getfiles", "getdirectories", "enumeratefiles", "enumeratedirectories", "enumeratefilesystementries", "getlastwritetime", "getcreationtime", "getattributes"];
const PS_VALUE_TYPES: &[&str] = &["math", "string", "io.path", "convert", "datetime", "timespan", "regex", "text.regularexpressions.regex", "int", "int32", "int64", "double"];
/// Method name prefixes that write even on value-type static calls.
const PS_STATIC_WRITE_PREFIXES: &[&str] = &["gettemp", "create", "write", "append", "move", "copy", "delete", "set", "replace", "encrypt", "decrypt"];

/// Parser facts -> (danger kind, unknown names). With no danger kind and no unknown, the result is a parsed read.
fn judge_ps_facts(facts: &[crate::ps_ast::Fact]) -> (Vec<(Kind, &'static str)>, Vec<String>) {
    use crate::ps_ast::Fact;
    let mut kinds: Vec<(Kind, &'static str)> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let note = |u: &mut Vec<String>, s: String| {
        if !u.contains(&s) {
            u.push(s);
        }
    };
    for f in facts {
        match f {
            Fact::Command { op, elems } => {
                let Some((k0, v0)) = elems.first() else { continue };
                if *k0 == 'b' && op == "Ampersand" {
                    continue; // `& { ... }`: commands inside the block arrive separately as facts
                }
                if *k0 != 's' {
                    note(&mut unknown, format!("{} {v0}", if op == "Dot" { "." } else { "&" }));
                    continue;
                }
                if op == "Dot" {
                    note(&mut unknown, format!(". {v0}"));
                    continue;
                }
                let name = base(v0);
                // A path-qualified name such as `.\ForEach-Object` runs that file in place = unknown
                if pathed(v0) {
                    note(&mut unknown, v0.clone());
                    continue;
                }
                if matches!(name.as_str(), "where-object" | "where" | "?") {
                    // Comparison only; the block body arrives separately as facts. If the first argument or -FilterScript is an expression (`$sb`) rather than a literal block, the body is not visible = unknown
                    // Abbreviations such as `-F` and `-Fi:` also mean -FilterScript; unless the following slot (including a colon argument) is a literal block, unknown
                    let filter = |p: &str| !p.is_empty() && "filterscript".starts_with(&p.to_ascii_lowercase());
                    // An expression not after a parameter (`$sb` in `-InputObject 1 $sb`) is positional and may bind to FilterScript
                    let positional_expr = elems.iter().enumerate().skip(1).any(|(i, (k, _))| *k == 'x' && elems[i - 1].0 != 'p');
                    let hidden = positional_expr || elems.windows(2).any(|w| w[0].0 == 'p' && filter(&w[0].1) && w[1].0 != 'b');
                    if hidden {
                        note(&mut unknown, v0.clone());
                    }
                    continue;
                }
                if matches!(name.as_str(), "foreach-object" | "foreach" | "%") {
                    // A read only when it takes just a block: `ForEach-Object Delete` (-MemberName) calls a method by name
                    if !elems[1..].iter().all(|(k, v)| *k == 'b' || (*k == 'p' && matches!(v.to_ascii_lowercase().as_str(), "process" | "begin" | "end"))) {
                        note(&mut unknown, v0.clone());
                    }
                    continue;
                }
                let words: Vec<String> = elems.iter().map(|(k, v)| match k {
                    'p' => format!("-{v}"),
                    'b' => "{}".to_string(),
                    _ => v.clone(),
                }).collect();
                if !is_read_segment(&words, &[], true) {
                    note(&mut unknown, v0.clone());
                }
            }
            Fact::Method { is_static, ty, name } => {
                let n = name.to_ascii_lowercase();
                let t = ty.to_ascii_lowercase();
                let t = t.strip_prefix("system.").unwrap_or(&t);
                let shown = if *is_static { format!("[{ty}]::{name}()") } else { format!(".{name}()") };
                if n.starts_with("delete") {
                    kinds.push((Kind::FileDelete, "a .NET method call deletes files"));
                } else if *is_static && PS_VALUE_TYPES.contains(&t) {
                    // Value-type static call: write-shaped names (`[IO.Path]::GetTempFileName()`) ask, without asserting that a file is changed (`[regex]::Replace` touches no file)
                    // A dynamic method name (`[IO.Path]::$m()` = `?`) is also unknown
                    if n == "?" || PS_STATIC_WRITE_PREFIXES.iter().any(|p| n.starts_with(p)) {
                        note(&mut unknown, shown);
                    }
                } else if PS_WRITE_METHODS.contains(&n.as_str()) {
                    kinds.push((Kind::FileOverwrite, "a .NET method call changes files"));
                } else {
                    let read = if *is_static {
                        matches!(t, "io.file" | "io.directory") && PS_STATIC_READ.contains(&n.as_str())
                    } else {
                        PS_SAFE_METHODS.contains(&n.as_str())
                    };
                    if !read {
                        note(&mut unknown, shown);
                    }
                }
            }
            Fact::Redirect(t) => {
                if !t.eq_ignore_ascii_case("$null") {
                    kinds.push((Kind::FileOverwrite, "the target uses patterns or variables"));
                }
            }
            Fact::Assign => kinds.push((Kind::FileOverwrite, "a property is set, which can change a file")),
            Fact::Convert(ty) => {
                // Only value-type casts (`[int]$x`) are reads; other casts call a constructor (`[IO.StreamWriter]'file'` creates and truncates a file)
                let t = ty.to_ascii_lowercase();
                if !PS_VALUE_TYPES.contains(&t.strip_prefix("system.").unwrap_or(&t)) {
                    note(&mut unknown, format!("[{ty}]"));
                }
            }
            // Nodes outside the allow list (`using`, class, switch, trap, etc.) = unknown
            Fact::Unlisted(k) => note(&mut unknown, k.strip_suffix("Ast").unwrap_or(k).to_string()),
        }
    }
    // Zero facts = nothing verified = unknown (not a pass)
    if facts.is_empty() {
        note(&mut unknown, "(nothing the hook could check)".to_string());
    }
    (kinds, unknown)
}

/// PowerShell control words: not command names; they head a following `{ ... }` block.
const PS_CONTROL: &[&str] = &["if", "elseif", "else", "foreach", "for", "while", "do", "until", "switch", "try", "catch", "finally"];

/// A standalone quoted word is output only (nothing runs). Only when the previous word is the piece start, `{`, `}` or a control word (not `sudo "x"` or dot-sourcing `. "x"`).
/// A double-quoted `$(...)` is routed to complex by the parser and does not reach here.
fn ps_string_output(seg: &Segment, at: usize, part: &[String]) -> bool {
    let after_cut = at == 0 || seg.words.get(at - 1).is_some_and(|w| w.text == "{" || w.text == "}" || PS_CONTROL.contains(&w.text.to_ascii_lowercase().as_str()));
    part.len() == 1 && after_cut && seg.words.get(at).is_some_and(|w| w.quoted)
}

/// Position where the real command starts after stripping leading Bash grammar words. None = no command (`done`, `fi`, `}`, `for NAME ...`, `select NAME ...`).
/// `case`, function definitions, `eval` and `source` are not handled here (unknown command = ask, as before).
fn bash_cmd_start(w: &[String]) -> Option<usize> {
    let lead = w.iter().take_while(|x| BASH_LEAD.contains(&x.as_str())).count();
    let rest = &w[lead..];
    if matches!(rest.first().map(String::as_str), Some("for" | "select")) || (rest.len() == 1 && matches!(rest[0].as_str(), "done" | "fi" | "}")) {
        return None;
    }
    let tail = strip_prefix_words(rest);
    (!tail.is_empty()).then(|| w.len() - tail.len())
}

/// Commands from PowerShell piece words split at control words, `{` and `}` (start position within the piece, words). The splitting words themselves are dropped.
fn ps_parts(words: &[String]) -> Vec<(usize, &[String])> {
    let cut = |w: &String| w == "{" || w == "}" || PS_CONTROL.contains(&w.to_ascii_lowercase().as_str());
    let mut out = Vec::new();
    let mut s = 0;
    for (i, w) in words.iter().enumerate() {
        if cut(w) {
            if i > s {
                out.push((s, &words[s..i]));
            }
            s = i + 1;
        }
    }
    if s < words.len() {
        out.push((s, &words[s..]));
    }
    out
}

/// Classify one tool call (tool_name and tool_input from the hook input).
pub fn assess(tool: &str, input: &Value, map: &DeviceMap, cwd: &Path, limits: Limits) -> Assessment {
    // Built-in tools that take one command line. PowerShell is parsed with the PowerShell parser (backslash = path), and the deep-word check catches Remove-Item etc. (blocks if unsure).
    if SHELL_TOOLS.contains(&tool) {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if cmd.trim().is_empty() {
            return Assessment { unanswerable: true, ..Default::default() };
        }
        return assess_shell(cmd, map, cwd, limits, tool == "PowerShell");
    }
    if BUILTIN_READ_TOOLS.contains(&tool) {
        return Assessment::default();
    }
    if BUILTIN_OVERWRITE_TOOLS.contains(&tool) {
        let target = input.get("file_path").or_else(|| input.get("notebook_path")).and_then(|v| v.as_str());
        let mut a = Assessment { self_guard: target.map(|t| t.contains(".olp")).unwrap_or(false), ..Default::default() };
        let c = match (target, limits.counter) {
            (Some(t), Some(d)) => count_or_unknown(Unit::File, counter::count_paths(cwd, &[t], Instant::now() + d)),
            (None, _) => Count::unknown(Unit::File, "no target file given"),
            (_, None) => Count::unknown(Unit::File, NO_LIMIT),
        };
        a.findings.push(Finding { kind: Kind::FileOverwrite, count: c });
        return a;
    }
    let key = format!("tool:{tool}");
    let mut a = Assessment::default();
    match map.entries.get(&key) {
        Some(e) if e.effect == Effect::ReadOnly => {}
        Some(e) => a.classified.push(e.clone()),
        None => a.to_classify.push(format!("--tool {tool}")),
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dw(s: &str) -> Vec<Kind> {
        detect_dangers(&shell::deep_words([s]))
    }
    #[test]
    fn dangers() {
        assert_eq!(dw("rm -rf x"), [Kind::FileDelete]);
        assert_eq!(dw("git -C a push -fu origin main"), [Kind::GitForcePush]);
        assert!(dw("git push -u origin main").is_empty());
        assert_eq!(dw("git reset --hard"), [Kind::GitResetHard]);
        assert!(dw("git reset --soft HEAD~1").is_empty());
        assert_eq!(dw("psql -c 'delete from t'"), [Kind::DbDelete]);
        assert_eq!(dw("UPDATE t SET a=1"), [Kind::DbUpdate]);
        assert!(dw("npm update").is_empty());
        assert_eq!(dw("/usr/bin/rm.exe a"), [Kind::FileDelete]);
    }
    #[test]
    fn families() {
        let f = |s: &str| family_of(&s.split(' ').map(String::from).collect::<Vec<_>>()).unwrap();
        assert_eq!(f("cargo test --release"), "cargo test");
        assert_eq!(f("python script.py"), "python");
        assert_eq!(f("FOO=1 sudo -E npm run build"), "npm run");
        assert_eq!(f("ls -la"), "ls");
    }
    /// Per-kind delete counts for the PowerShell tool: only literal paths are counted; for the rest, the reason nothing was counted.
    fn ps_counts(cmd: &str) -> Vec<(Option<u64>, Option<&'static str>)> {
        let d = std::env::temp_dir().join(format!("olp_ps_count_{}", std::process::id()));
        let _ = std::fs::create_dir_all(d.join("build"));
        for f in ["a.txt", "b.txt", "c.log"] {
            std::fs::write(d.join("build").join(f), "x").unwrap();
        }
        let a = assess("PowerShell", &serde_json::json!({ "command": cmd }), &DeviceMap::default(), &d, Limits { counter: Some(Duration::from_secs(3)), ps_parser: true });
        a.findings.iter().map(|f| (f.count.n, f.count.why_unknown)).collect()
    }
    #[test]
    fn powershell_delete_forms() {
        const PAT: Option<&str> = Some("the paths use patterns or variables");
        const FORM: Option<&str> = Some("this form is not counted");
        // Literal paths (backslash = path separator; aliases del, rd, ri, rm, rmdir, erase behave the same) are counted
        assert_eq!(ps_counts(r"Remove-Item -LiteralPath .\build -Recurse -Force -Confirm:$false"), [(Some(3), None)]);
        // Comma array (observed: an agent deleted two paths as `a, b` and the card showed 0): counted if every piece is literal
        assert_eq!(ps_counts(r"Remove-Item -Path build\a.txt, build\b.txt"), [(Some(2), None)]);
        assert_eq!(ps_counts(r"Remove-Item .\build\a.txt, .\build\c.log -Force"), [(Some(2), None)]);
        assert_eq!(ps_counts(r"Remove-Item build\a.txt,build\b.txt"), [(Some(2), None)]);
        assert_eq!(ps_counts(r"Remove-Item build\a.txt, $p"), [(None, PAT)]);
        assert_eq!(ps_counts(r"Remove-Item build\a.txt, build\*.log"), [(None, PAT)]);
        assert_eq!(ps_counts(r"del .\build\a.txt"), [(Some(1), None)]);
        assert_eq!(ps_counts(r"rd build -Recurse"), [(Some(3), None)]);
        assert_eq!(ps_counts(r"ri -r build -ErrorAction SilentlyContinue"), [(Some(3), None)]);
        // Variables and wildcards are patterns; pipes, -WhatIf, -Filter/-Include/-Exclude and cmd-style /s are not counted (no invented numbers)
        assert_eq!(ps_counts(r"Remove-Item $p -Recurse"), [(None, PAT)]);
        assert_eq!(ps_counts(r"Remove-Item .\build\*.txt"), [(None, PAT)]);
        assert_eq!(ps_counts(r"Get-ChildItem build | Remove-Item"), [(None, FORM)]);
        assert_eq!(ps_counts(r"Remove-Item build -Recurse -WhatIf"), [(None, FORM)]);
        assert_eq!(ps_counts(r"Remove-Item build -Recurse -Filter *.log"), [(None, FORM)]);
        assert_eq!(ps_counts(r"rd /s /q build"), [(None, FORM)]);
        // Security check: counting a non-file provider path (registry, environment variable, etc.) as "0 files" would give a wrong exact number, so it is not counted
        assert_eq!(ps_counts(r"Remove-Item HKCU:\Software\OlpTest -Recurse"), [(None, FORM)]);
        assert_eq!(ps_counts(r"Remove-Item Env:\OLP_TEST"), [(None, FORM)]);
        assert_eq!(ps_counts(r"Remove-Item -LiteralPath Registry::HKEY_CURRENT_USER\x"), [(None, FORM)]);
    }
    #[test]
    fn powershell_name_with_a_comma_is_not_split() {
        // If a real name contains a comma: quoted = that one name, unquoted = an array (two). The parser does not remember quotes, so it cannot tell, and nothing is counted
        let d = std::env::temp_dir().join(format!("olp_ps_comma_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        std::fs::write(d.join("x,y.txt"), "x").unwrap();
        for cmd in ["Remove-Item 'x,y.txt'", "Remove-Item x,y.txt"] {
            let a = assess("PowerShell", &serde_json::json!({ "command": cmd }), &DeviceMap::default(), &d, Limits { counter: Some(Duration::from_secs(3)), ps_parser: true });
            assert_eq!(a.findings.iter().map(|f| (f.count.n, f.count.why_unknown)).collect::<Vec<_>>(), [(None, Some(NOT_COUNTED))], "{cmd}");
        }
    }
    #[test]
    fn powershell_script_block_gets_no_junk_classify_lines() {
        // Regression: for `... | Where-Object { ... } | ForEach-Object { ... "{0} files={1}" -f ... }; git check-ignore build`, the card listed pieces
        // such as `"}"`, `"1mb"` and `"{0} files=..."` as classification candidates. PowerShell block commands now give 0 candidates plus one block line, still treated as a write (never a read pass).
        let cmd = r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s=(Get-ChildItem $_.FullName -Recurse -File | Measure-Object Length -Sum); "{0} files={1} MB={2:N1}" -f $_.FullName,$s.Count,($s.Sum/1MB) }; git status --short | Select-Object -First 10; git check-ignore build"#;
        let run = |tool: &str, c: &str| assess(tool, &serde_json::json!({ "command": c }), &DeviceMap::default(), Path::new("."), Limits { counter: None, ps_parser: true });
        // Pieces not shaped like a name and block/exec verbs give 0 candidates. `git check-ignore` is a read, so it also gives 0 candidates.
        let a = run("PowerShell", cmd);
        assert!(a.to_classify.is_empty(), "{:?}", a.to_classify);
        // The verdict is unchanged: even without a candidate it is a write (no read pass, effect unknown). With the parser available, a parsed read passes;
        // cases that depend on the parser moved to tests/ps_parser_paths.rs (shared helper require_ps_parser_or_env_fallback)
        // (`(foo)` and `& x` are split by the parser at the parenthesis and `&` into `foo` and `x`, real command names, so they correctly remain candidates and were removed from the list)
        // A piece of only braces (`}`) has no command to run = read (a block `if (x) { (...) }` produces `{` and `}` pieces), so it is removed from the list
        assert!(run("PowerShell", "}").is_read(), "중괄호만 = 돌 명령 0");
        // A standalone string `"1mb"` is output only = read (removed from this list). `"..." -f x` with more words is unchanged.
        assert!(run("PowerShell", r#""1mb""#).is_read(), "홀로 선 문자열 = 출력");
        // With the parser available, variable output, blocks with only numbers and comparisons are parsed reads, so those two shapes (`ForEach-Object { 1 }`, `where-object x`) live in tests/ps_parser_paths.rs
        // A lone `$x` yields zero parser facts = unknown, so it stays on the junk side. `iex` and syntax-error shapes are unchanged regardless of the parser.
        for junk in [r#""{0} files={1}" -f x"#, "iex x", "$x"] {
            let j = run("PowerShell", junk);
            assert!(j.to_classify.is_empty(), "{junk}: {:?}", j.to_classify);
            assert!(!j.is_read() && j.effect().is_none(), "{junk}: 쓰기 취급 그대로");
            let card = crate::card::ask_card(&j);
            assert!(card.contains("Not classified yet") && !card.contains("olp-hook classify"), "{junk}: {card}");
        }
        // Real tools starting with a digit (Bash) remain candidates (no regression)
        assert_eq!(run("Bash", "7z a x.zip").to_classify, ["7z a"]);
        assert_eq!(run("Bash", "2to3 -w x.py").to_classify, ["2to3"]);
        // PowerShell digit-leading words are numeric expressions; a non-literal first word (`$s.Sum/1MB` -> base `1mb`) gives 0 candidates
        assert!(run("PowerShell", r#""1mb""#).to_classify.is_empty());
        // In PowerShell too, digit-leading tools stay candidates (no regression on the main Windows surface); only numeric literal shapes give 0
        assert_eq!(run("PowerShell", "7z a x.zip").to_classify, ["7z a"]);
        assert!(run("PowerShell", r#""1MB""#).to_classify.is_empty());
        assert!(run("PowerShell", "2.5gb").to_classify.is_empty());
        assert!(run("PowerShell", "$s.Sum/1MB").to_classify.is_empty());
        assert!(run("Bash", "$x/1mb run").to_classify.is_empty());
        // Quoted-output piece observed in a real terminal session
        assert!(run("PowerShell", r#""--- diff"; git diff --stat"#).to_classify.is_empty());
        // `source` is an execution verb just like `.`
        assert!(run("Bash", "source env.sh").to_classify.is_empty());
        // Existing candidates are unchanged (same filter as Bash)
        assert_eq!(run("Bash", "npm run build").to_classify, ["npm run"]);
        assert_eq!(run("Bash", "cargo test --release").to_classify, ["cargo test"]);
        assert!(run("Bash", "}").to_classify.is_empty());
    }
    #[test]
    fn cd_to_the_same_folder_keeps_counting() {
        // Observed: an agent wrote `cd <project>; Remove-Item ...` and got "changes folder first". Same directory means no directory change (even if cd fails it stays), so count.
        let d = std::env::temp_dir().join(format!("olp_cd_same_{}", std::process::id()));
        let _ = std::fs::create_dir_all(d.join("build"));
        let _ = std::fs::create_dir_all(d.join("other"));
        for f in ["a.txt", "b.txt"] {
            std::fs::write(d.join("build").join(f), "x").unwrap();
        }
        let ds = d.to_string_lossy().to_string();
        let run = |tool: &str, cmd: &str| -> Vec<(Option<u64>, Option<&'static str>)> {
            let a = assess(tool, &serde_json::json!({ "command": cmd }), &DeviceMap::default(), &d, Limits { counter: Some(Duration::from_secs(3)), ps_parser: true });
            a.findings.iter().map(|f| (f.count.n, f.count.why_unknown)).collect()
        };
        let moved = Some(CHANGES_FOLDER);
        // (1) same absolute path (2) `.` and `.\` (3) case and trailing `\` variants (Windows) = counted; any of cd, chdir, Set-Location, sl, joined by `;` or `&&`
        assert_eq!(run("PowerShell", &format!(r"cd {ds}; Remove-Item build\a.txt, build\b.txt -Force")), [(Some(2), None)]);
        assert_eq!(run("PowerShell", &format!("Set-Location '{ds}'; Remove-Item build -Recurse")), [(Some(2), None)]);
        assert_eq!(run("PowerShell", &format!("sl '{ds}' && Remove-Item build -Recurse")), [(Some(2), None)]);
        assert_eq!(run("PowerShell", r"cd .; Remove-Item build -Recurse"), [(Some(2), None)]);
        assert_eq!(run("PowerShell", r"chdir .\; Remove-Item build -Recurse"), [(Some(2), None)]);
        if cfg!(windows) {
            assert_eq!(run("PowerShell", &format!(r"cd {}\; Remove-Item build -Recurse", ds.to_uppercase())), [(Some(2), None)]);
        }
        // (4) a different real directory (including subdirectories) (5) variable or `~` (6) pushd (stack verb) = not counted, as before
        assert_eq!(run("PowerShell", r"cd other; Remove-Item ..\build -Recurse"), [(None, moved)]);
        assert_eq!(run("PowerShell", r"cd build; Remove-Item a.txt"), [(None, moved)]);
        assert_eq!(run("PowerShell", r"cd $env:TEMP; Remove-Item build -Recurse"), [(None, moved)]);
        assert_eq!(run("PowerShell", r"cd ~; Remove-Item build -Recurse"), [(None, moved)]);
        assert_eq!(run("PowerShell", &format!("pushd '{ds}'; Remove-Item build -Recurse")), [(None, moved)]);
        // (7) Bash rules unchanged: even the same absolute path after `;` is not counted
        assert_eq!(run("Bash", &format!("cd '{ds}'; rm -rf build")), [(None, moved)]);
    }
    #[test]
    fn powershell_backslash_is_a_separator_on_every_os() {
        // Linux CI failure: `.\build` was read as a literal file name, giving "0 files". PowerShell accepts `\` as a separator on every OS.
        let p = shell::parse_ps(r"Remove-Item .\build\sub -LiteralPath a\b.txt");
        let w: Vec<String> = p.segments[0].words.iter().map(|x| x.text.clone()).collect();
        assert_eq!(ps_delete_paths(&w, &p.segments[0], 0).unwrap(), ["./build/sub", "a/b.txt"]);
    }
    #[test]
    fn powershell_read_cmdlets_pass_but_bash_list_unchanged() {
        assert!(ps_counts(r"Get-ChildItem -Force -Directory | Select-Object Name; Test-Path .\build").is_empty());
        let rd = |t: &str, c: &str| assess(t, &serde_json::json!({ "command": c }), &DeviceMap::default(), Path::new("."), Limits { counter: None, ps_parser: true });
        let a = rd("PowerShell", r"Get-Content .\a.txt | Measure-Object -Line");
        assert!(a.findings.is_empty() && a.to_classify.is_empty() && a.classified.is_empty());
        // ForEach-Object and Where-Object run script blocks, so they are outside the read list
        assert!(!rd("PowerShell", "Get-ChildItem | ForEach-Object Delete").is_read());
        // The Bash read list is unchanged (PowerShell names are not added)
        assert!(!rd("Bash", "Test-Path x").to_classify.is_empty());
    }
    #[test]
    fn push_ranges() {
        let v = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(push_range(&v("--force origin main")).unwrap(), "main..refs/remotes/origin/main");
        assert_eq!(push_range(&v("origin +HEAD:dev")).unwrap(), "HEAD..refs/remotes/origin/dev");
        assert!(push_range(&v("--force")).is_none());
        assert!(push_range(&v("origin :dev")).is_none());
    }
}
