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
/// Names whose definition is a delete but that take no file list (0.1.3): a drive format and a recycle-bin clear. Not in DELETE_VERBS, so `count_for` keeps "this form is not counted".
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
    if let Some(k) = runner_key(w, &w0) {
        return Some(k);
    }
    if let Some(name) = interpreter_name(&w0) {
        return Some(interpreter_key(w, &name));
    }
    if let Some(w1) = w.get(1) {
        if !w1.starts_with('-') && w1.chars().all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c)) {
            return Some(format!("{w0} {}", w1.to_lowercase()));
        }
    }
    Some(w0)
}

/// Programs that start a package runner. One-word runners run the package named next;
/// the others are runners only with the second word in `TWO_WORD_RUNNERS` (`npm run`, `yarn build` keep their 0.1.x key; design 1-6 adds `pnpm exec`, `yarn exec`).
const RUNNER_PROGRAMS: &[&str] = &["npx", "pnpx", "bunx", "npm", "pnpm", "yarn", "bun"];
const ONE_WORD_RUNNERS: &[&str] = &["npx", "pnpx", "bunx"];
const TWO_WORD_RUNNERS: &[(&str, &str)] =
    &[("npm", "exec"), ("npm", "x"), ("pnpm", "dlx"), ("pnpm", "exec"), ("yarn", "dlx"), ("yarn", "exec"), ("bun", "x")];
/// Value-less runner flags skipped before the package name (§2-2 2). Any other flag means the package cannot be named.
const RUNNER_PLAIN_FLAGS: &[&str] = &["-y", "--yes", "-q", "--quiet", "--no", "--no-install", "--prefer-offline", "--prefer-online", "--"];

/// The runner program behind a first word, also in its Windows `.cmd`/`.ps1` form (review blocking 2 ⓑ).
fn runner_program(w0: &str) -> Option<&'static str> {
    let n = w0.strip_suffix(".cmd").or_else(|| w0.strip_suffix(".ps1")).unwrap_or(w0);
    RUNNER_PROGRAMS.iter().copied().find(|p| *p == n)
}

/// Group key for a package runner call: `<runner> <package>` (§2-2 4), or the runner alone when the package cannot be named.
/// A runner-only key is never a name (`runner_only`), so it is never stored, read or offered on a card. None = not a runner call.
fn runner_key(w: &[String], w0: &str) -> Option<String> {
    let prog = runner_program(w0)?;
    let (runner, rest): (String, &[String]) = if ONE_WORD_RUNNERS.contains(&prog) {
        (prog.to_string(), &w[1..])
    } else {
        match w.get(1) {
            // A global flag before the runner word (`npm --yes exec x`): no group answer this version (review blocking 2 ⓐ).
            Some(w1) if w1.starts_with('-') => return Some(prog.to_string()),
            Some(w1) if TWO_WORD_RUNNERS.contains(&(prog, w1.to_lowercase().as_str())) => (format!("{prog} {}", w1.to_lowercase()), &w[2..]),
            // `yarn tsx`, `bun tsx`: the program runs an interpreter bin, so one answer would cover every script (second review blocking 3).
            Some(w1) if is_interpreter(&w1.to_lowercase()) => return Some(prog.to_string()),
            Some(_) => return None,
            None => return Some(prog.to_string()),
        }
    };
    let i = rest.iter().take_while(|x| RUNNER_PLAIN_FLAGS.contains(&x.as_str())).count();
    match rest.get(i).and_then(|s| package_name(s)) {
        // `npx tsx`, `pnpm exec node`: an interpreter as the package takes no group answer (second review blocking 3).
        Some(p) if is_interpreter(&p) => Some(runner),
        Some(p) => Some(format!("{runner} {p}")),
        None => Some(runner),
    }
}

/// An npm package name (`name` or `@scope/name`, lowercased) with a trailing version spec dropped. None when the word is not
/// a name: a flag, path, URL, variable, or an `@` spec that is not a version (`@npm:`, `@file:`, `@github:` name another package,
/// review blocking 1).
fn package_name(spec: &str) -> Option<String> {
    if spec.starts_with('-') {
        return None;
    }
    let s = spec.to_lowercase();
    let (scoped, body) = match s.strip_prefix('@') {
        Some(b) => (true, b),
        None => (false, s.as_str()),
    };
    let (name, version) = match body.find('@') {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    if let Some(v) = version {
        if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || ".^~<>=*|+ -".contains(c)) {
            return None;
        }
    }
    let part = |p: &str| !p.is_empty() && !p.starts_with('.') && p.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c));
    if scoped {
        let (scope, n) = name.split_once('/')?;
        (part(scope) && part(n)).then(|| format!("@{name}"))
    } else {
        part(name).then(|| name.to_string())
    }
}

/// A key that names only a package runner (or the one-word runner program): one answer would cover every package it runs.
pub fn runner_only(family: &str) -> bool {
    RUNNER_PROGRAMS.contains(&family) || TWO_WORD_RUNNERS.iter().any(|(a, b)| family.split_once(' ') == Some((a, b)))
}

/// Script interpreters, a closed list (19): the group key is `<interpreter> <script path>`,
/// so one script's answer never covers another.
const INTERPRETERS: &[&str] = &[
    "node", "nodejs", "deno", "bun", "tsx", "ts-node", "python", "python3", "py", "ruby", "perl", "php", "bash", "sh", "zsh", "dash", "pwsh", "powershell", "cmd",
];

/// Whether a name (already without `.exe`/`.cmd`/`.ps1`) is a script interpreter, also in a versioned form such as `python3.12`.
fn is_interpreter(n: &str) -> bool {
    INTERPRETERS.contains(&n) || n.strip_prefix("python").is_some_and(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// The interpreter behind a first word, with its Windows `.cmd`/`.ps1` suffix dropped (`.exe` is already dropped by `base`).
fn interpreter_name(w0: &str) -> Option<String> {
    let n = w0.strip_suffix(".cmd").or_else(|| w0.strip_suffix(".ps1")).unwrap_or(w0);
    is_interpreter(n).then(|| n.to_string())
}

/// §2-1c key: `<interpreter> <script>` when the very next word is a script path inside the project (rules 1-3, 5), and
/// `deno run <path>`, `bun run <file.ext>`, `bun <file>` (rule 6). Anything else is the interpreter alone, which is never a name.
/// `bun run <script>` stays `bun run`; other subcommand words are handled below.
fn interpreter_key(w: &[String], name: &str) -> String {
    if (name == "deno" || name == "bun") && w.get(1).is_some_and(|w1| w1.eq_ignore_ascii_case("run")) {
        return match w.get(2) {
            Some(w2) if name == "bun" && plain_word(w2) => "bun run".to_string(),
            Some(w2) => match script_path(w2) {
                Some(sp) if name == "deno" || sp.rsplit('/').next().is_some_and(|f| f.contains('.')) => format!("{name} run {sp}"),
                _ => name.to_string(),
            },
            None => name.to_string(),
        };
    }
    match w.get(1) {
        // Design 1-8: a word without `/`, `\` or `.` is not a path. For bun and deno a subcommand-shaped word keeps the 0.1.x
        // two-word key (`bun install`, `deno test`); for every other interpreter it takes no group answer (`node server`, `php artisan`).
        Some(w1) if !w1.contains(['/', '\\', '.']) => match (name == "bun" || name == "deno") && plain_word(w1) {
            true => format!("{name} {}", w1.to_lowercase()),
            false => name.to_string(),
        },
        Some(w1) => match script_path(w1) {
            Some(sp) => format!("{name} {sp}"),
            None => name.to_string(),
        },
        None => name.to_string(),
    }
}

/// A word shaped like a 0.1.x subcommand: letters, digits, `-`, `_`, `:` only.
fn plain_word(s: &str) -> bool {
    !s.starts_with('-') && !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_:".contains(c))
}

/// §2-1c rules 2-3: a literal relative path inside the project. Allowed letters are `A-Z a-z 0-9 . _ - /` and `\` only (design 1-6);
/// `\` is read as `/` and leading `./` is dropped (case kept); a leading `/` (also a network path), a drive letter, `~`, `@`, spaces,
/// any `..` piece, variables, globs and braces are not a path here.
fn script_path(word: &str) -> Option<String> {
    let mut p = word.replace('\\', "/");
    while let Some(r) = p.strip_prefix("./") {
        p = r.to_string();
    }
    let ok = !p.starts_with('-')
        && !p.starts_with('/')
        && p.chars().any(|c| c.is_ascii_alphanumeric())
        && p.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
        && p.split('/').all(|piece| !piece.is_empty() && piece != "..")
        // Design 1-8: still a path after normalizing (`./test` -> `test` is not, so it never shares `bun test`).
        && p.contains(['.', '/']);
    ok.then_some(p)
}

/// §2-1c rule 4: the hook folder written relative to the project root, as `""` (root) or `"sub/dir/"`. None when the folder is not
/// inside the project or a piece is not a plain path letter; then an interpreter key takes no group answer. No file is opened.
fn root_prefix(cwd: &Path) -> Option<String> {
    let Some(root) = PROJECT_ANCHOR.with(|p| p.borrow().clone()) else { return Some(String::new()) };
    if cwd == root {
        return Some(String::new());
    }
    let rel = cwd.strip_prefix(&root).ok()?;
    let mut out = String::new();
    for c in rel.components() {
        let std::path::Component::Normal(s) = c else { return None };
        let s = s.to_str()?;
        if s.is_empty() || !s.chars().all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch)) || s == "." || s == ".." {
            return None;
        }
        out.push_str(s);
        out.push('/');
    }
    Some(out)
}

/// An interpreter key with its script path written from the project root (`node a.js` run in `sub` -> `node sub/a.js`).
/// Subcommand keys (`php artisan`) are left alone; a folder outside the project leaves the interpreter alone (unnamed).
fn rooted_interpreter_key(fam: &str, cwd: &Path) -> String {
    let Some((head, path)) = fam.rsplit_once(' ') else { return fam.to_string() };
    if plain_word(path) && !head.contains(' ') && !path.contains('.') {
        return fam.to_string();
    }
    match root_prefix(cwd) {
        Some(pre) if pre.is_empty() => fam.to_string(),
        Some(pre) => format!("{head} {pre}{path}"),
        None => fam.split(' ').next().unwrap_or(fam).to_string(),
    }
}

/// Design 1-11 §2-4d ①②③: the words before the program are only exact table wrappers and the value-less flags
/// `sudo -n`/`-E`/`-H` and `env -i`. Any `NAME=value`, any other flag (value-taking, bundled, `=`, shortened) or
/// any other word means the shape is not understood.
fn understood_prefix(prefix: &[String]) -> bool {
    let mut wrapper = "";
    for w in prefix {
        let b = base(w);
        if !pathed(w) && WRAPPERS.contains(&b.as_str()) {
            wrapper = WRAPPERS.iter().copied().find(|x| *x == b).unwrap_or("");
            continue;
        }
        let ok_flag = (wrapper == "sudo" && ["-n", "-E", "-H"].contains(&w.as_str())) || (wrapper == "env" && w == "-i");
        if !ok_flag {
            return false;
        }
    }
    true
}

/// Shell words that change the environment, options or what later names mean (design 1-12 §2-4d ⑤ «outside the list»):
/// never a group answer and never offered on a card (second review RECHECK 9a54918df blocking 1: the card offered `export`).
const ENV_CHANGE_VERBS: &[&str] =
    &["export", "declare", "typeset", "readonly", "local", "set", "unset", "alias", "unalias", "eval", "hash", "trap", "function", "shopt", "set-item", "set-variable", "new-variable"];

/// Design 1-12 §2-4d ⑤ closed tables: first words of an earlier piece that may stand before an interpreter or runner key.
const PROVEN_READ_BASH: &[&str] = &["ls", "cat", "echo", "pwd", "true", "date", "sleep", "head", "tail", "grep", "rg", "wc", "which", "git"];
const PROVEN_READ_PS: &[&str] = &["get-childitem", "get-content", "write-output", "get-location", "git"];

/// Keys the closed grammar applies to: the program is an interpreter, a runner program or a §2-4c ⓑ word.
fn closed_grammar_family(family: &str) -> bool {
    let w0 = family.split(' ').next().unwrap_or("");
    is_interpreter(w0) || RUNNER_PROGRAMS.contains(&w0) || w0 == "corepack"
}

/// A key made by `runner_key` (`npx cowsay`, `bun x cowsay`): its last word is a package, not a path.
fn runner_family(family: &str) -> bool {
    let mut it = family.split(' ');
    let (a, b) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    ONE_WORD_RUNNERS.contains(&a) || TWO_WORD_RUNNERS.contains(&(a, b))
}

/// A key that names only an interpreter: never stored, read or offered on a card.
pub fn interpreter_only(family: &str) -> bool {
    !family.contains(' ') && is_interpreter(family)
}

/// A key whose first word is an interpreter (§2-1c rule 4: after a folder move in the same command it takes no group answer).
pub fn interpreter_family(family: &str) -> bool {
    family.split(' ').next().is_some_and(is_interpreter)
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
        && !runner_only(family)
        && !interpreter_only(family)
        && !olp_self(family)
        && !wide_last_word(family)
        && !ENV_CHANGE_VERBS.contains(&w0)
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
    DELETE_VERBS.contains(&w0) || OVERWRITE_VERBS.contains(&w0) || w0 == "olp-hook" || olp_self(family) || wide_last_word(family)
}

/// Design 1-10 §2-4c: a key whose last word (suffix `.cmd`/`.ps1`/`.bat`/`.exe` dropped, lowercased) is an interpreter or a
/// runner/package-manager word (`npx tsx`, `yarn tsx`, `x node`, `corepack pnpm`) would cover every script or package it runs.
fn wide_last_word(family: &str) -> bool {
    let Some((_, last)) = family.rsplit_once(' ') else { return false };
    let l = last.to_lowercase();
    let n = [".cmd", ".ps1", ".bat", ".exe"].iter().find_map(|s| l.strip_suffix(*s)).unwrap_or(l.as_str());
    is_interpreter(n) || ["npx", "pnpx", "bunx", "npm", "pnpm", "yarn", "bun", "corepack"].contains(&n)
}

/// Design 1-9 §2-4b: a key that runs OLP itself through a runner (`npx @feedpia/olp`, `pnpm exec olp`) or an interpreter
/// (`node .../olp.js`): one read answer would also cover its write sub-commands, so it never takes a group answer.
fn olp_self(family: &str) -> bool {
    let Some((head, last)) = family.rsplit_once(' ') else { return false };
    if runner_only(head) {
        return last == "olp" || last == "@feedpia/olp";
    }
    interpreter_family(head) && ["olp", "olp.js", "olp-hook"].contains(&last.rsplit('/').next().unwrap_or(last))
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
        // `file -C` / `--compile` (any unambiguous prefix) writes `<magic>.mgc` (second review RECHECK rev 17-2 blocking 3).
        "file" => !rest.iter().any(|a| match a.strip_prefix("--") {
            Some(long) => {
                let name = long.split('=').next().unwrap_or("");
                !name.is_empty() && "compile".starts_with(name)
            }
            None => a.starts_with('-') && a.contains('C'),
        }),
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
    deep.iter().any(|w| base_name_is_hook(w) || w.contains(".olp"))
}

/// The one self-protection check (second review RECHECK 2 of rel c9d34233a, blocking 1): every path that judges a command —
/// a plain piece, a redirect the parser could not name, a command that builds itself (heredoc, `$( )`, `>( )`) — calls this, so a
/// target spelled the same way is judged the same way. `words` are judged one by one like `tee`/`cp` targets, and so is every word
/// written after `>` in the raw text.
fn self_guard(words: &[String], base: Option<&Path>, cmd: &str) -> bool {
    touches_self(words) || touches_self_at(words, base, cmd) || redirect_words(cmd).iter().any(|w| target_is_olp_state(w, base, cmd))
}

/// OLP's own state (rev 17-3; B review of rel 49d7a9281, its RECHECK and the second review of b59d967e2). The trust folder is
/// `<home>/.olp/projects/<key>/` and the installed hook is in `<home>/olp/`, so:
/// 1. `.olp` anywhere in the target (or, for a target the parser could not name, anywhere in the command) = OLP's state.
/// 2. A target that can be resolved — home variables expanded, made absolute against `base` (the folder after any `cd` the parser
///    followed), normalised, and every link in its existing part resolved on disk — is compared once with `<home>/.olp` and
///    `<home>/olp`. A link that cannot be resolved = OLP's state.
/// 3. A target that cannot be resolved for sure (another variable, `~user`, a folder the parser could not follow) = OLP's state
///    when the command contains `olp` anywhere. A glob is matched component by component against the two roots.
fn target_is_olp_state(t: &str, base: Option<&Path>, ctx: &str) -> bool {
    if t.contains(".olp") || (t.is_empty() && ctx.contains(".olp")) {
        return true;
    }
    // A part the shell expands but this check does not ($'..', $"..", a lone $, a brace list) counts as `*` (second review RECHECK 6).
    let g = unexpanded_as_wildcard(t);
    if g != t && target_is_olp_state(&g, base, ctx) {
        return true;
    }
    // A target led by a variable that is not a home variable can be anywhere: its trust-file name alone decides
    // (second review RECHECK 8 coord side of rel 71131c8ee).
    if g.starts_with('*') && t.starts_with('$') {
        let leaf = g.rsplit(['/', '\\']).next().unwrap_or_default();
        // the leaf must be written (letters), not itself a variable: a lone `$path` says nothing about the name
        let written = g.contains(['/', '\\']) && leaf.chars().any(|c| c.is_ascii_alphabetic());
        if written && ["hook-map.json", "hook-config.json"].iter().any(|n| pattern_may_match(leaf, n)) {
            return true;
        }
    }
    let unsure = || mentions_olp_component(ctx);
    let Some(e) = expand_home(t) else { return other_home_may_hit_olp(t) || unsure() };
    // The Git Bash spelling `/c/Users/<me>/…` (the Bash tool's own `$HOME` and `pwd` on Windows) is read as `C:/Users/<me>/…` before it is
    // made absolute: `std::path::absolute` would otherwise put the rootless `\c\…` on the process drive (second review coord RECHECK 19).
    // Only `/c/…` with a path after the letter: a two-letter word such as the `xcopy`/`robocopy` flag `/E` stays a word.
    let e = if e.as_bytes().get(2) == Some(&b'/') { git_bash_path(&e).map(|p| p.to_string_lossy().into_owned()).unwrap_or(e) } else { e };
    // A drive-relative target (`c:olp`, `c:.olp`) is relative to that drive's own current folder (design addendum 4-3): resolved against
    // the working folder when that folder is on the same drive. On another drive its current folder is not known here: a drive letter
    // no trust-folder root is on cannot reach them («the drive letter differs», existing or not — B FAIL R26 of rel 8d914a1e0), so it is
    // not the trust folder unless the text names `olp` or a trust file; a root's own drive stays closed.
    let drive_rel = drive_of(&e).filter(|_| cfg!(windows) && !e[2..].starts_with(['/', '\\']));
    let e = match drive_rel {
        Some(d) => match base.filter(|b| drive_of(&b.to_string_lossy()) == Some(d)) {
            Some(b) => b.join(&e[2..]).to_string_lossy().into_owned(),
            None => {
                let root_drives: Vec<char> = olp_roots().iter().filter_map(|r| drive_of(r)).collect();
                let shown_not = !root_drives.is_empty() && !root_drives.contains(&d);
                // naming `olp` (`Z:olp/bin` names it only once the drive is cut off) or a trust file keeps it closed: the same command may
                // still map that drive (`subst`)
                let rest = e[2..].to_ascii_lowercase();
                let names_trust_file = ["hook-map.json", "hook-config.json"].iter().any(|n| rest.contains(n));
                return !shown_not || unsure() || mentions_olp_component(&e[2..]) || names_trust_file;
            }
        },
        None => e,
    };
    let p = PathBuf::from(&e);
    let abs = if p.is_absolute() || p.has_root() {
        p
    } else {
        match base {
            Some(b) => b.join(p),
            None => return relative_glob_may_hit_olp(&e) || unsure(),
        }
    };
    // a relative working folder (`.`) would leave the path relative, and its links could then never be resolved (every target = OLP's
    // state, rc CI run 43724580621): made absolute against the process folder first
    let abs = normalize(&std::path::absolute(&abs).unwrap_or(abs));
    if e.contains(['*', '?', '[']) {
        return glob_may_hit_olp(&abs) || unsure();
    }
    match resolve_links(&abs) {
        Some(real) => under_olp_root(&abs) || under_olp_root(&real),
        None => true,
    }
}

/// `~user/rest` (another user's home, or `~+`/`~-`): bash expands it, so the first component of `rest` is matched like a glob
/// against `olp` and `.olp` (more asking, never less: the home is not known here).
fn other_home_may_hit_olp(t: &str) -> bool {
    let Some(rest) = t.strip_prefix('~') else { return false };
    let Some(at) = rest.find(['/', '\\']) else { return false };
    // `~user/../x` leaves that home: where it lands is not known here (second review RECHECK 3 of rel 61983f0d3, blocking 1 ③)
    if rest[at + 1..].split(['/', '\\']).any(|c| c == "..") {
        return true;
    }
    let first = normalize(&Path::new("/").join(rest[at + 1..].replace('\\', "/")));
    let Some(c) = first.components().find_map(|c| match c {
        std::path::Component::Normal(n) => Some(n.to_string_lossy().to_string()),
        _ => None,
    }) else {
        return false;
    };
    ["olp", ".olp"].iter().any(|r| c.eq_ignore_ascii_case(r) || pattern_may_match(&c, r))
}

/// The target with every part the shell expands but this check cannot follow replaced by `*`: `$'..'` and `$".."` (and a `$` whose
/// quotes were already removed, i.e. a `$` not starting a variable name, `{` or `(`), and a brace list `{a,b}` not after `$`.
/// Home variables (`$HOME`, `${HOME}`, `$USERPROFILE`, `$env:USERPROFILE` …) are kept for `expand_home`; every other variable is `*`. Applying it twice gives the same text.
fn unexpanded_as_wildcard(t: &str) -> String {
    let c: Vec<char> = t.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let next = c.get(i + 1).copied();
        if ch == '$' && matches!(next, Some('\'' | '"')) {
            let q = next.unwrap_or('\'');
            let mut j = i + 2;
            while j < c.len() && c[j] != q {
                j += if c[j] == '\\' { 2 } else { 1 };
            }
            out.push('*');
            i = j + 1;
            continue;
        }
        if ch == '$' && !next.is_some_and(|n| n.is_ascii_alphabetic() || n == '_' || n == '{' || n == '(') {
            out.push('*');
            i += 1;
            continue;
        }
        // Any other variable (`$X`, `${X}`, `$(..)`, and `$olp` left by `$'o'lp` once quotes are gone) is `*`, unless it is one of
        // the home variables `expand_home` resolves (second review RECHECK 7 coord side · B RECHECK 10 of 922c6bf14).
        if ch == '$' {
            let (name, end) = match next {
                Some('{') => {
                    let k = c[i..].iter().position(|&x| x == '}').map_or(c.len(), |k| i + k);
                    (c[i + 2..k.min(c.len())].iter().collect::<String>(), (k + 1).min(c.len()))
                }
                Some('(') => {
                    let mut depth = 0;
                    let mut k = i + 1;
                    while k < c.len() {
                        depth += match c[k] {
                            '(' => 1,
                            ')' => -1,
                            _ => 0,
                        };
                        k += 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    (String::from("("), k)
                }
                _ => {
                    let mut k = i + 1;
                    if c[k..].iter().take(4).collect::<String>().eq_ignore_ascii_case("env:") {
                        k += 4;
                    }
                    while k < c.len() && (c[k].is_ascii_alphanumeric() || c[k] == '_') {
                        k += 1;
                    }
                    (c[i + 1..k].iter().collect::<String>(), k)
                }
            };
            let homes = ["HOME", "USERPROFILE", "env:HOME", "env:USERPROFILE"];
            let home = homes.iter().any(|h| name.eq_ignore_ascii_case(h));
            // `${HOME%/}` and the default forms (`${HOME:-x}`, `${HOME-x}`, `${HOME:=x}`, `${HOME=x}`, `${HOME:?x}`, `${HOME?x}`) are
            // still the home; any other operation (`${HOME%/*}` = the parent, `#…`, `/a/b`, `:n:m`, `:+`, `^`, `,`) is `*` (B RECHECK 12).
            let home_op = (next == Some('{'))
                .then(|| {
                    homes.iter().find(|h| {
                        name.get(..h.len()).is_some_and(|p| p.eq_ignore_ascii_case(h))
                            && name.get(h.len()..).is_some_and(|r| r == "%/" || [":-", "-", ":=", "=", ":?", "?"].iter().any(|o| r.starts_with(o)))
                    })
                })
                .flatten();
            if let Some(h) = home_op {
                out.push_str(&format!("${{{h}}}"));
            } else if home {
                out.extend(&c[i..end]);
            } else {
                out.push('*');
            }
            i = end;
            continue;
        }
        if ch == '{' && !(i > 0 && c[i - 1] == '$') {
            if let Some(k) = c[i..].iter().position(|&x| x == '}') {
                out.push('*');
                i += k + 1;
                continue;
            }
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// A relative glob whose folder is not known (after a folder move the parser could not follow): any component that could name
/// `.olp` or `olp` = OLP's state, when the pattern starts with a dot or has at least one letter (so a bare `*` or `?` does not count).
fn relative_glob_may_hit_olp(e: &str) -> bool {
    e.split(['/', '\\']).filter(|c| c.contains(['*', '?', '['])).any(|c| {
        (c.starts_with('.') || c.chars().any(|ch| ch.is_ascii_alphabetic())) && (pattern_may_match(c, ".olp") || pattern_may_match(c, "olp"))
    })
}

/// The real words of a command the parser could not split (heredoc, `$( )`, `>( )`): quotes removed, `$`, `{` and `}` kept, split at
/// blanks and `; | & < > ( )` (and backticks in Bash); the words of a `$( )` inside double quotes are words too. A word whose
/// pattern characters were all quoted is a plain name, not a glob, and is left out (its `.olp` letters are still seen by
/// `touches_self`). The same kind of words as a plain piece, so both paths judge a target the same way.
fn raw_words(text: &str, ps: bool) -> Vec<String> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut w = String::new();
    let (mut glob_q, mut glob_u) = (false, false);
    let (mut sq, mut dq) = (false, false);
    let esc = if ps { '`' } else { '\\' };
    let mut flush = |w: &mut String, glob_q: &mut bool, glob_u: &mut bool| {
        if !w.is_empty() && !(*glob_q && !*glob_u) {
            out.push(std::mem::take(w));
        }
        w.clear();
        *glob_q = false;
        *glob_u = false;
    };
    let mut inner: Vec<String> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        let pat = "*?[".contains(ch);
        if sq {
            if ch == '\'' {
                sq = false;
            } else {
                glob_q |= pat;
                w.push(ch);
            }
        } else if ch == esc && i + 1 < c.len() {
            glob_q |= "*?[".contains(c[i + 1]);
            w.push(c[i + 1]);
            i += 1;
        } else if dq {
            if ch == '"' {
                dq = false;
            } else if ch == '$' && c.get(i + 1) == Some(&'(') {
                // the command inside runs even within double quotes; its end is found with quotes respected (second review RECHECK 4)
                let j = subst_end(&c, i + 2, ps);
                let body: String = c[i + 2..j.min(c.len())].iter().collect();
                inner.extend(raw_words(&body, ps));
                w.push_str("$()");
                i = j;
            } else if !ps && ch == '`' {
                // a backtick command inside double quotes runs too (second review RECHECK 4 of rel 622fe5781)
                let mut j = i + 1;
                while j < c.len() && c[j] != '`' {
                    j += if c[j] == '\\' { 2 } else { 1 };
                }
                let body: String = c[i + 1..j.min(c.len())].iter().collect();
                inner.extend(raw_words(&body, ps));
                w.push_str("``");
                i = j;
            } else {
                glob_q |= pat;
                w.push(ch);
            }
        } else if ch == '\'' {
            sq = true;
        } else if ch == '"' {
            dq = true;
        } else if ch.is_whitespace() || ";|&<>()".contains(ch) || (!ps && ch == '`') {
            flush(&mut w, &mut glob_q, &mut glob_u);
        } else {
            glob_u |= pat;
            w.push(ch);
        }
        i += 1;
    }
    flush(&mut w, &mut glob_q, &mut glob_u);
    out.extend(inner);
    out
}

/// A second reading of a built command that does not track quotes at all: quote, backtick and backslash characters are dropped and the
/// text is split at blanks and `; | & < > ( )`. A lone quote in a heredoc body or a comment (`don't`), or a `#` / `case` `)` inside
/// `$( )`, can make the quote-aware reading see a later line as quoted; this reading never does, so the two together miss less (more
/// asking, never less — second review RECHECK 5 of rel 80db1a4cc).
fn blind_words(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || ";|&<>()".contains(c))
        .map(|w| w.chars().filter(|c| !"'\"`\\".contains(*c)).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect()
}

/// The index of the `)` that closes a `$(` whose inside starts at `from`, counting parentheses outside quotes only (like shell.rs);
/// the text length when it is not closed.
fn subst_end(c: &[char], from: usize, ps: bool) -> usize {
    let esc = if ps { '`' } else { '\\' };
    let (mut sq, mut dq) = (false, false);
    let mut depth = 1;
    let mut j = from;
    while j < c.len() {
        let x = c[j];
        if sq {
            sq = x != '\'';
        } else if x == esc {
            j += 1;
        } else if dq {
            dq = x != '"';
        } else if x == '\'' {
            sq = true;
        } else if x == '"' {
            dq = true;
        } else if x == '(' {
            depth += 1;
        } else if x == ')' {
            depth -= 1;
            if depth == 0 {
                return j;
            }
        }
        j += 1;
    }
    c.len()
}

/// A glob that can only name folders above `<home>/olp` and `<home>/.olp` (`/*`, `/**`, `~/*` written in a heredoc body or a code
/// comment): in a command that builds itself every word is judged, and such a word is mostly text, so it is left out there. Plain
/// pieces still judge it as a target (second review RECHECK 3 of rel 61983f0d3, 1-b, without a second heredoc parser).
fn glob_above_olp_only(w: &str, base: Option<&Path>) -> bool {
    if !w.contains(['*', '?', '[']) {
        return false;
    }
    let Some(e) = expand_home(w) else { return false };
    let p = PathBuf::from(&e);
    let abs = if p.is_absolute() || p.has_root() {
        p
    } else {
        match base {
            Some(b) => b.join(p),
            None => return false,
        }
    };
    let n = path_key(&normalize(&abs).to_string_lossy()).split('/').count();
    olp_roots().iter().all(|r| n < r.split('/').count())
}

/// A word that moves the folder the rest runs in (`cd`, `pushd`, `Set-Location`, `env --chdir`, `-C dir`): after one, relative
/// targets have no known folder.
fn moves_folder(w: &str) -> bool {
    let l = w.to_ascii_lowercase();
    CD_VERBS.contains(&base(w).as_str()) || l.starts_with("cd..") || l.starts_with("cd\\") || l.starts_with("cd~") || l == "--chdir" || l.starts_with("--chdir=") || w.starts_with("-C") || w.starts_with("-D")
}

/// The words written right after `>` / `>>` / `2>` / `&>` outside quotes in the raw text (quotes around the word removed). Used only for
/// targets the parser could not name; `>&2`-style duplications have no word.
fn redirect_words(cmd: &str) -> Vec<String> {
    let c: Vec<char> = cmd.chars().collect();
    let mut out = Vec::new();
    let (mut sq, mut dq) = (false, false);
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if sq {
            sq = ch != '\'';
        } else if dq {
            dq = ch != '"';
        } else if ch == '\'' {
            sq = true;
        } else if ch == '"' {
            dq = true;
        } else if ch == '>' {
            while i + 1 < c.len() && (c[i + 1] == '>' || c[i + 1] == '|') {
                i += 1;
            }
            if i + 1 < c.len() && c[i + 1] == '&' {
                i += 1;
                continue;
            }
            while i + 1 < c.len() && c[i + 1].is_whitespace() {
                i += 1;
            }
            let mut w = String::new();
            let (mut wsq, mut wdq) = (false, false);
            while i + 1 < c.len() {
                let n = c[i + 1];
                if !wsq && !wdq && (n.is_whitespace() || ";|&<>()".contains(n)) {
                    break;
                }
                i += 1;
                match n {
                    '\'' if !wdq => wsq = !wsq,
                    '"' if !wsq => wdq = !wdq,
                    _ => w.push(n),
                }
            }
            if !w.is_empty() {
                out.push(w);
            }
        }
        i += 1;
    }
    out
}

thread_local! {
    /// Links resolved during one judgement (`assess_shell` empties it first): the disk does not change while one command is judged, and
    /// many checks resolve the same target (rc CI run 43724580621 fix — relative targets now reach the disk for every word).
    static RESOLVED: std::cell::RefCell<std::collections::HashMap<PathBuf, Option<PathBuf>>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// The existing part of the path with every link resolved on disk, the rest appended; None when an existing link cannot be resolved.
fn resolve_links(abs: &Path) -> Option<PathBuf> {
    if let Some(hit) = RESOLVED.with(|m| m.borrow().get(abs).cloned()) {
        return hit;
    }
    let r = resolve_links_now(abs);
    RESOLVED.with(|m| m.borrow_mut().insert(abs.to_path_buf(), r.clone()));
    r
}

fn resolve_links_now(abs: &Path) -> Option<PathBuf> {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = abs.to_path_buf();
    loop {
        if std::fs::symlink_metadata(&cur).is_ok() {
            let mut real = std::fs::canonicalize(&cur).ok()?;
            for c in rest.iter().rev() {
                real.push(c);
            }
            return Some(real);
        }
        let name = cur.file_name()?.to_os_string();
        rest.push(name);
        if !cur.pop() {
            return Some(abs.to_path_buf());
        }
    }
}

/// Could the glob pattern (absolute, normalised) name `<home>/olp` or `<home>/.olp`, or something inside or above them? Component by
/// component with `*`, `?` and `[...]`. A leading dot is matched by wildcards too: PowerShell does so, and so does Bash with
/// `dotglob` (B RECHECK 5 condition 1 — the protecting side).
fn glob_may_hit_olp(pattern: &Path) -> bool {
    // the pattern keeps its letter case (design addendum 3); the matcher folds case when it compares
    let pat = path_key_cased(&pattern.to_string_lossy());
    let pat: Vec<&str> = pat.split('/').collect();
    olp_roots().iter().any(|root| {
        let root: Vec<&str> = root.split('/').collect();
        pat.iter().zip(root.iter()).all(|(p, r)| pattern_may_match(p, r))
    })
}

/// Design addendum 2 of 11-15: the one matcher for every expansion the shell does from the text alone. Could `pattern` (one name,
/// no `/`) name `literal`? Braces are expanded first (`{a,b}`, `{x..y}` numbers or letters, at most `BRACE_CAP` results), then each
/// result is read as a glob: `*`, `?`, `[..]` (ranges, `!`/`^`, POSIX classes `[[:lower:]]`; sets are compared in both letter
/// cases), extended globs `?( *( +( @( !(` (`!(..)` = anything), and the same text without extended globs (PowerShell, or bash
/// without `extglob`) — either reading that can match is a match. A negated set is any one letter (PowerShell reads `[!o]` as the
/// set `!`,`o`). A part it cannot read (an open `[`, an open extended glob, a `{x..y}` that is not a range, more brace results than
/// `BRACE_CAP`, a match that takes too many steps) reaches the name unless the letters before it already differ from the name
/// (addendum 2-1: `~/x[l` does not reach; `~/.o[l` and, at the very front of a piece, `~/[.o` do). A number range over the cap is
/// read as digits (`[-0-9]*`), so `file{1..300}.txt` stays ordinary.
fn pattern_may_match(pattern: &str, literal: &str) -> bool {
    may_match(pattern, literal, true)
}

/// `pattern_may_match` with `lead` = whether an unreadable part at the very front reaches. Only `names_trust_folder` passes
/// `false`, for word parts that are not in a path place (the first piece of a word, or a piece cut at every separator), so code
/// text such as `a=[a, 1]` is not read as a path — addendum 2's hidden-name place condition, not a change to 2-1 (a).
fn may_match(pattern: &str, literal: &str, lead: bool) -> bool {
    let lit: Vec<char> = literal.chars().collect();
    let mut st = Steps { left: MATCH_BUDGET, lead };
    brace_expand(pattern).iter().any(|p| {
        let c: Vec<char> = p.chars().collect();
        [true, false].into_iter().any(|ext| glob_match(&parse_glob(&c, ext), &lit, 0, lit.len(), &mut st))
    })
}

/// Matching steps left, and whether an unreadable part at the very front reaches.
struct Steps {
    left: u32,
    lead: bool,
}

/// Most results one brace expansion gives before the rest counts as unreadable.
const BRACE_CAP: usize = 256;
/// Most matching steps before the match counts as unreadable (`*` repeated many times).
const MATCH_BUDGET: u32 = 200_000;
/// Stands for a part `brace_expand` could not read; `parse_glob` turns it into `G::Unknown`.
const UNREADABLE: char = '\u{1}';

enum G {
    Lit(char),
    One,
    Star,
    Set(Vec<SetItem>),
    Ext(char, Vec<Vec<G>>),
    Unknown,
}

enum SetItem {
    Ch(char),
    Range(char, char),
    Class(String),
    Any,
}

/// `{a,b}` and `{x..y[..step]}` expanded the way bash does (not after `$`); an unclosed `{` or one with neither a comma nor `..`
/// is a plain letter. A `{x..y}` that is not a number or one-letter range, or more than `BRACE_CAP` results, is `UNREADABLE`.
fn brace_expand(w: &str) -> Vec<String> {
    let c: Vec<char> = w.chars().collect();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '{' && !(i > 0 && c[i - 1] == '$') {
            if let Some(close) = matching_brace(&c, i) {
                let inner: String = c[i + 1..close].iter().collect();
                if let Some(alts) = brace_alternatives(&inner) {
                    let pre: String = c[..i].iter().collect();
                    let post: String = c[close + 1..].iter().collect();
                    let mut out = Vec::new();
                    for a in alts {
                        for t in brace_expand(&format!("{a}{post}")) {
                            out.push(format!("{pre}{t}"));
                            if out.len() > BRACE_CAP {
                                // too many to list (design addendum 3): keep the letters that can come first, then anything —
                                // only up to the end of this path piece; the pieces after it are expanded and read as before
                                // (second review R15: `~/{a..z}{a..z}/../{.o,x}lp` must still show `.olp`)
                                let j = piece_end(&c, i);
                                let piece: String = c[i..j].iter().collect();
                                let after: String = c[j..].iter().collect();
                                let tail = first_letters(&piece, 0).map_or_else(|| UNREADABLE.to_string(), |s| letter_set(&s));
                                return brace_expand(&format!("{pre}{tail}{after}"));
                            }
                        }
                    }
                    return out;
                }
            }
        }
        i += 1;
    }
    vec![w.to_string()]
}

/// The index of the first `/` or `\` at or after `from` that is outside braces (the end of this path piece), or the text length.
fn piece_end(c: &[char], from: usize) -> usize {
    let mut depth = 0i32;
    for (k, &x) in c.iter().enumerate().skip(from) {
        match x {
            '{' => depth += 1,
            '}' => depth -= 1,
            '/' | '\\' if depth <= 0 => return k,
            _ => {}
        }
    }
    c.len()
}

/// Deepest brace nesting `first_letters` follows before the letters count as unknown.
const BRACE_DEPTH: usize = 8;
/// What the over-cap number range reads as (`brace_alternatives`).
const NUMBER_RUN: &str = "[-0-9]*";

/// The letters the brace expansion of `s` can start with, without listing it (design addendum 3). `None` = not known: the text
/// can be empty there, starts with a pattern letter (`*`, `?`, `[`, an extended glob), a `\`, something unreadable, or the
/// braces nest deeper than `BRACE_DEPTH`.
fn first_letters(s: &str, depth: usize) -> Option<Vec<char>> {
    if depth > BRACE_DEPTH {
        return None;
    }
    let c: Vec<char> = s.chars().collect();
    let first = *c.first()?;
    if first == '{' {
        if let Some(close) = matching_brace(&c, 0) {
            let inner: String = c[1..close].iter().collect();
            if let Some(alts) = brace_alternatives(&inner) {
                let post: String = c[close + 1..].iter().collect();
                let mut out = Vec::new();
                for a in alts {
                    if a == NUMBER_RUN {
                        out.extend("-0123456789".chars());
                        continue;
                    }
                    out.extend(first_letters(&format!("{a}{post}"), depth + 1)?);
                }
                return Some(out);
            }
        }
        return Some(vec!['{']);
    }
    let ext_open = "?*+@!".contains(first) && c.get(1) == Some(&'(');
    if ext_open || "*?[\\".contains(first) || first == UNREADABLE {
        return None;
    }
    Some(vec![first])
}

/// The letters as one glob set followed by `*` (`[.a]*`), or `UNREADABLE` when a letter cannot sit in a set safely.
fn letter_set(letters: &[char]) -> String {
    let mut l: Vec<char> = letters.to_vec();
    l.sort_unstable();
    l.dedup();
    if l.is_empty() || l.iter().any(|ch| "]!^[\\/".contains(*ch) || *ch == UNREADABLE) {
        return UNREADABLE.to_string();
    }
    // `-` last so it is a letter, not a range
    let dash = l.contains(&'-');
    let mut set: String = l.into_iter().filter(|ch| *ch != '-').collect();
    if dash {
        set.push('-');
    }
    format!("[{set}]*")
}

fn matching_brace(c: &[char], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (k, &x) in c.iter().enumerate().skip(open) {
        match x {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// The results of one brace group's inside, or `None` when it is not a brace expression (then `{` is a plain letter).
fn brace_alternatives(inner: &str) -> Option<Vec<String>> {
    let mut parts = vec![String::new()];
    let mut depth = 0;
    for ch in inner.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(String::new());
                continue;
            }
            _ => {}
        }
        if let Some(last) = parts.last_mut() {
            last.push(ch);
        }
    }
    if parts.len() >= 2 {
        return Some(parts);
    }
    if !inner.contains("..") {
        return None;
    }
    let unreadable = Some(vec![UNREADABLE.to_string()]);
    let seq: Vec<&str> = inner.split("..").collect();
    if !(2..=3).contains(&seq.len()) {
        return unreadable;
    }
    let step = match seq.get(2) {
        Some(s) => match s.parse::<i64>() {
            Ok(n) if n != 0 => n.unsigned_abs(),
            Ok(_) => 1,
            Err(_) => return unreadable,
        },
        None => 1,
    };
    let count = |lo: i64, hi: i64| lo.abs_diff(hi) / step + 1;
    if let (Ok(a), Ok(b)) = (seq[0].parse::<i64>(), seq[1].parse::<i64>()) {
        if count(a, b) > BRACE_CAP as u64 {
            // numbers only: a sign or digit first, read as `[-0-9]*` (never a dot)
            return Some(vec!["[-0-9]*".to_string()]);
        }
        let pad = [seq[0], seq[1]].iter().any(|s| s.trim_start_matches('-').len() > 1 && s.trim_start_matches('-').starts_with('0'));
        let width = if pad { seq[0].len().max(seq[1].len()) } else { 0 };
        let dir: i64 = if b >= a { 1 } else { -1 };
        return Some((0..count(a, b) as i64).map(|k| format!("{:0width$}", a + dir * k * step as i64)).collect());
    }
    let (a, b): (Vec<char>, Vec<char>) = (seq[0].chars().collect(), seq[1].chars().collect());
    if a.len() == 1 && b.len() == 1 && a[0].is_ascii() && b[0].is_ascii() {
        let (a, b) = (a[0] as i64, b[0] as i64);
        let dir: i64 = if b >= a { 1 } else { -1 };
        return Some((0..count(a, b) as i64).filter_map(|k| u32::try_from(a + dir * k * step as i64).ok().and_then(char::from_u32)).map(String::from).collect());
    }
    unreadable
}

/// One name read as a glob; `ext` = extended globs are read too. Reading stops at the first part that cannot be read (`G::Unknown`).
fn parse_glob(c: &[char], ext: bool) -> Vec<G> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch == UNREADABLE {
            out.push(G::Unknown);
            return out;
        }
        if ext && "?*+@!".contains(ch) && c.get(i + 1) == Some(&'(') {
            match parse_ext_group(c, i + 2) {
                Some((alts, next)) => {
                    out.push(G::Ext(ch, alts));
                    i = next;
                    continue;
                }
                None => {
                    out.push(G::Unknown);
                    return out;
                }
            }
        }
        match ch {
            '*' => {
                if !matches!(out.last(), Some(G::Star)) {
                    out.push(G::Star);
                }
            }
            '?' => out.push(G::One),
            '[' => match parse_set(c, i + 1) {
                Some((g, next)) => {
                    out.push(g);
                    i = next;
                    continue;
                }
                None => {
                    out.push(G::Unknown);
                    return out;
                }
            },
            _ => out.push(G::Lit(ch)),
        }
        i += 1;
    }
    out
}

/// The `|`-separated choices of an extended glob whose inside starts at `from`, and the index after its `)`; `None` when not closed.
fn parse_ext_group(c: &[char], from: usize) -> Option<(Vec<Vec<G>>, usize)> {
    let mut depth = 1;
    let mut alts = Vec::new();
    let mut start = from;
    for (k, &x) in c.iter().enumerate().skip(from) {
        match x {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    alts.push(parse_glob(&c[start..k], true));
                    return Some((alts, k + 1));
                }
            }
            '|' if depth == 1 => {
                alts.push(parse_glob(&c[start..k], true));
                start = k + 1;
            }
            _ => {}
        }
    }
    None
}

/// A bracket set whose inside starts at `from`, and the index after its `]`; `None` when not closed. A negated set is `G::One`.
fn parse_set(c: &[char], from: usize) -> Option<(G, usize)> {
    let mut j = from;
    let neg = matches!(c.get(j), Some('!' | '^'));
    if neg {
        j += 1;
    }
    let mut items = Vec::new();
    let mut first = true;
    loop {
        let ch = *c.get(j)?;
        if ch == ']' && !first {
            break;
        }
        first = false;
        if ch == '[' {
            if let Some(&d @ (':' | '=' | '.')) = c.get(j + 1) {
                if let Some(k) = (j + 2..c.len().saturating_sub(1)).find(|&k| c[k] == d && c[k + 1] == ']') {
                    let name: String = c[j + 2..k].iter().collect();
                    items.push(if d == ':' { SetItem::Class(name) } else { SetItem::Any });
                    j = k + 2;
                    continue;
                }
            }
        }
        if c.get(j + 1) == Some(&'-') && c.get(j + 2).is_some_and(|x| *x != ']') {
            items.push(SetItem::Range(ch, c[j + 2]));
            j += 3;
            continue;
        }
        items.push(SetItem::Ch(ch));
        j += 1;
    }
    Some((if neg { G::One } else { G::Set(items) }, j + 1))
}

/// The same letter when letter case is folded the Unicode way (`Ö` = `ö`), as Windows and macOS do for file names (second review R15).
fn same_letter(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

fn set_has(items: &[SetItem], x: char) -> bool {
    // the letter in both cases, Unicode way (single-letter case forms only)
    let one = |mut it: std::char::ToLowercase| it.next().filter(|_| it.next().is_none());
    let up = |mut it: std::char::ToUppercase| it.next().filter(|_| it.next().is_none());
    [Some(x), one(x.to_lowercase()), up(x.to_uppercase())].into_iter().flatten().any(|y| {
        items.iter().any(|it| match it {
            SetItem::Ch(a) => *a == y,
            SetItem::Range(a, b) => *a <= y && y <= *b,
            SetItem::Any => true,
            SetItem::Class(n) => match n.as_str() {
                "alpha" => y.is_alphabetic(),
                "digit" => y.is_ascii_digit(),
                "alnum" => y.is_alphanumeric(),
                "upper" => y.is_uppercase(),
                "lower" => y.is_lowercase(),
                "space" => y.is_whitespace(),
                "blank" => y == ' ' || y == '\t',
                "punct" => y.is_ascii_punctuation(),
                "print" => !y.is_control(),
                "graph" => !y.is_control() && !y.is_whitespace(),
                "cntrl" => y.is_control(),
                "xdigit" => y.is_ascii_hexdigit(),
                "word" => y.is_alphanumeric() || y == '_',
                _ => true,
            },
        })
    })
}

/// Does `p` match all of `s[i..end]`? `G::Unknown` reaches when the part before it matched — something (`i > 0`), or nothing
/// at all when `lead` (addendum 2-1 (a)); running out of steps reaches.
fn glob_match(p: &[G], s: &[char], i: usize, end: usize, budget: &mut Steps) -> bool {
    if budget.left == 0 {
        return true;
    }
    budget.left -= 1;
    match p.first() {
        None => i == end,
        Some(G::Unknown) => i > 0 || budget.lead,
        Some(G::Star) => (i..=end).any(|k| glob_match(&p[1..], s, k, end, budget)),
        Some(G::One) => i < end && glob_match(&p[1..], s, i + 1, end, budget),
        // letter case is folded here, when comparing — never by lower-casing the pattern first (design addendum 3: `[Z-p]`)
        Some(G::Lit(c)) => i < end && same_letter(s[i], *c) && glob_match(&p[1..], s, i + 1, end, budget),
        Some(G::Set(items)) => i < end && set_has(items, s[i]) && glob_match(&p[1..], s, i + 1, end, budget),
        Some(G::Ext(op, alts)) => (i..=end).any(|k| ext_match(*op, alts, s, i, k, budget) && glob_match(&p[1..], s, k, end, budget)),
    }
}

/// Does the extended glob `op(alts)` match all of `s[i..k]`? `!(..)` matches anything (the closed side).
fn ext_match(op: char, alts: &[Vec<G>], s: &[char], i: usize, k: usize, budget: &mut Steps) -> bool {
    match op {
        '!' => true,
        '@' => alts.iter().any(|a| glob_match(a, s, i, k, budget)),
        '?' => i == k || alts.iter().any(|a| glob_match(a, s, i, k, budget)),
        '*' => i == k || ext_repeat(alts, s, i, k, budget),
        _ => ext_repeat(alts, s, i, k, budget),
    }
}

/// One or more choices of `alts` one after another match all of `s[i..k]`.
fn ext_repeat(alts: &[Vec<G>], s: &[char], i: usize, k: usize, budget: &mut Steps) -> bool {
    if i == k {
        return alts.iter().any(|a| glob_match(a, s, i, i, budget));
    }
    (i + 1..=k).any(|m| alts.iter().any(|a| glob_match(a, s, i, m, budget)) && (m == k || ext_repeat(alts, s, m, k, budget)))
}

/// `<home>/olp` and `<home>/.olp` as comparison keys, for HOME and USERPROFILE, as written and with links resolved.
fn olp_roots() -> Vec<String> {
    // the roots depend only on HOME and USERPROFILE: kept per thread for the same values (each canonicalize of the home is a disk call,
    // and one command judges many targets — rc CI run 43724580621 fix made relative targets reach this for every word)
    thread_local!(static ROOTS: std::cell::RefCell<Option<(Option<std::ffi::OsString>, Option<std::ffi::OsString>, Vec<String>)>> = const { std::cell::RefCell::new(None) });
    let key = (std::env::var_os("HOME"), std::env::var_os("USERPROFILE"));
    if let Some(v) = ROOTS.with(|r| r.borrow().as_ref().filter(|c| c.0 == key.0 && c.1 == key.1).map(|c| c.2.clone())) {
        return v;
    }
    let v = olp_roots_now();
    ROOTS.with(|r| *r.borrow_mut() = Some((key.0, key.1, v.clone())));
    v
}

fn olp_roots_now() -> Vec<String> {
    let mut out = Vec::new();
    for h in [std::env::var_os("HOME"), std::env::var_os("USERPROFILE")].into_iter().flatten().filter(|h| !h.is_empty()) {
        let h = PathBuf::from(h);
        for base in [Some(h.clone()), std::fs::canonicalize(&h).ok()].into_iter().flatten() {
            for name in ["olp", ".olp"] {
                let k = path_key(&base.join(name).to_string_lossy());
                if !out.contains(&k) {
                    out.push(k);
                }
            }
        }
    }
    out
}

/// Does the command name a path component that is exactly `olp` or `.olp` (`~/olp`, `'olp\..'`, `x/.olp/y`)? A name that only
/// contains the letters (`olp-hook`, `olp_cache`, `~nobody_olp`) does not count.
fn mentions_olp_component(ctx: &str) -> bool {
    ctx.split(|c: char| c == '/' || c == '\\' || c.is_whitespace() || "\"'`;&|<>=(),+{}$".contains(c))
        .any(|w| w.eq_ignore_ascii_case("olp") || w.eq_ignore_ascii_case(".olp"))
}

/// `words` are the piece's real words (quotes removed, `$`, `{`, `}` kept), so a path like `${USERPROFILE}/olp/x` is resolved whole.
/// A piece with parentheses or backticks builds its arguments at run time (`Join-Path $env:USERPROFILE 'olp'`): it cannot be resolved
/// word by word, so an `olp` component anywhere in the command makes it OLP's state.
fn touches_self_at(words: &[String], base: Option<&Path>, ctx: &str) -> bool {
    if ctx.contains(['(', ')', '`']) && mentions_olp_component(ctx) {
        return true;
    }
    words.iter().any(|w| base_name_is_hook(w) || target_is_olp_state(w, base, ctx))
}

fn base_name_is_hook(w: &str) -> bool {
    base(w) == "olp-hook"
}

/// Home variables replaced by their values; None when another `$`, `%` or backtick remains (cannot be resolved).
fn expand_home(t: &str) -> Option<String> {
    let home = crate::store::home().map(|h| h.to_string_lossy().to_string());
    let env_home = std::env::var("HOME").ok().filter(|h| !h.is_empty()).or_else(|| home.clone());
    let env_profile = std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()).or_else(|| home.clone());
    let mut s = t.to_string();
    if s == "~" || s.starts_with("~/") || s.starts_with("~\\") {
        s = format!("{}{}", home.clone()?, &s[1..]);
    } else if s.starts_with('~') {
        return None; // `~user`: another user's home, not resolved here
    }
    for (var, val) in [
        ("${USERPROFILE}", &env_profile),
        ("$USERPROFILE", &env_profile),
        ("%USERPROFILE%", &env_profile),
        ("${env:USERPROFILE}", &env_profile),
        ("$env:USERPROFILE", &env_profile),
        ("${HOME}", &env_home),
        ("$HOME", &env_home),
        ("%HOME%", &env_home),
        ("$env:HOME", &env_home),
    ] {
        let lower = s.to_ascii_lowercase();
        if let Some(at) = lower.find(&var.to_ascii_lowercase()) {
            s = format!("{}{}{}", &s[..at], val.clone()?, &s[at + var.len()..]);
        }
    }
    (!s.contains(['$', '%', '`'])).then_some(s)
}

/// Lexical normalisation: `.` dropped, `..` pops a component, no disk access (the target may not exist yet).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// A path as text for comparison: `/` slashes, Git Bash `/c/...` written as `c:/...`, lower case on Windows.
fn path_key(p: &str) -> String {
    let s = path_key_cased(p);
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s
    }
}

/// `path_key` without lower-casing: for a glob pattern, whose ranges must keep the letters as written (`[Z-p]`).
fn path_key_cased(p: &str) -> String {
    let mut s = p.replace('\\', "/");
    if let Some(r) = s.strip_prefix("//?/") {
        s = r.to_string();
    }
    let b = s.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b'/' {
        s = format!("{}:{}", &s[1..2], &s[2..]);
    }
    s.trim_end_matches('/').to_string()
}

/// Is the normalised absolute path at or under `<home>/olp` or `<home>/.olp` (see olp_roots)?
fn under_olp_root(p: &Path) -> bool {
    let target = path_key(&p.to_string_lossy());
    olp_roots().iter().any(|root| target == *root || target.starts_with(&format!("{root}/")))
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

const NO_LIMIT: &str = "no time limit set in your hook-config.json";
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
    match dir {
        Dir::Known { path, moved: true } if path.components().any(|c| c.as_os_str() == ".olp") => true,
        // the working folder (moved or not) inside the user folder `<home>/olp` (rev 17-3)
        Dir::Known { path, .. } => under_olp_root(&normalize(path)),
        _ => false,
    }
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
/// 0.1.3 ⑥-a reason (same `count not checked (reason)` frame, card.rs unchanged). Wording pending two native-speaker checks.
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
/// A command with `#` (a comment for the shell, where a lone quote such as `don't` is not a quote) is also judged on the reading that
/// does not track quotes, so a comment never hides the next line from self-protection (B RECHECK 8 of rel e430fd0d5; more asking only).
fn assess_shell(cmd: &str, map: &DeviceMap, cwd: &Path, limits: Limits, ps: bool) -> Assessment {
    RESOLVED.with(|m| m.borrow_mut().clear());
    let mut a = assess_shell_parsed(cmd, map, cwd, limits, ps);
    if !a.self_guard && cmd.contains('#') && !proven_reads_only(cmd, ps) {
        let base = built_base(cmd, cwd, ps);
        let words: Vec<String> = blind_words(cmd).into_iter().filter(|w| !glob_above_olp_only(w, base)).collect();
        a.self_guard = self_guard(&words, base, cmd);
    }
    // One rule for every place a trust-folder path can sit (a value, a `cd` argument, a link source, joined strings): a command that
    // writes (a danger kind was found, or it builds itself) and whose whole text, once quotes and string joins are undone, names `.olp`
    // is self-protection (second review RECHECK 9 coord side of rel 2f6d4fcf4 — the fifth miss of the same kind, so one rule instead
    // of one more shape). Reading the folder (`cat`, `ls`) is not a write and is left alone (B RECHECK 14 condition 2).
    // The design addendum of 11-15: the name (or a glob that can name it) seen anywhere = self-protection unless the
    // command is proven to be reads only — reads are proven, writes are never searched for (the five misses came from adding write shapes).
    let reads_only = proven_reads_only(cmd, ps);
    if !a.self_guard && names_trust_folder(&flattened(cmd, ps)) && !reads_only {
        a.self_guard = true;
    }
    // the other side of the same rule: a command proven to be reads only is never self-protection, whatever an earlier word check said
    // (`jq . ~/.olp/…`, second review RECHECK 11)
    if reads_only {
        a.self_guard = false;
    }
    a
}

/// Does the flattened text name the trust folder: `.olp` anywhere, or a word part that, as a glob, can name `.olp` and either has a
/// letter (`.o*`, `.ol?`, `.[o]lp`) or starts with a dot (`.*`, `.???` — second review RECHECK 11)? A bare `*` or `?` does not count.
/// Design addendum 2 of 11-15: the word parts are judged twice and either can reach — cut at every separator as before, and
/// whole (braces expanded over the whole word, then cut at `/`, `\` and `=`), so `{.o,x}lp`, `.{o..o}lp`, `.[[:lower:]]lp` and
/// `.@(o)lp` reach `pattern_may_match` as written.
/// Addendum 2-1 (a): a piece after a `/` or `\` in the same word part (a path place: `~/[.o`, `$HOME\[[:lower:]lp`, `~/` + more
/// brace results than the cap) reaches when its very front cannot be read. The first piece of a word part (after a blank or `=`)
/// and the pieces cut at every separator are not path places, so code text (`a=[a, 1]`, `echo '[a,'`) stays ordinary.
/// Design addendum 3: the word parts keep their letter case (`[Z-p]` holds `o`); only the plain `.olp` look-up is lower-cased, and
/// the matcher folds case when it compares.
fn names_trust_folder(flat: &str) -> bool {
    let may_name = |w: &str, path: bool| {
        (w.starts_with('.') || w.chars().any(|c| c.is_ascii_alphabetic()) || (path && w.starts_with([UNREADABLE, '['])))
            && may_match(w, ".olp", path)
    };
    let l = flat;
    flat.to_ascii_lowercase().contains(".olp")
        || l.split(|c: char| c.is_whitespace() || "/\\;|&<>()=:,{}$".contains(c)).any(|w| w.contains(['*', '?', '[']) && may_name(w, false))
        || l.split(|c: char| c.is_whitespace() || ";|&<>".contains(c)).any(|word| {
            brace_expand(word).iter().any(|e| {
                e.split('=').any(|part| part.split(['/', '\\']).enumerate().any(|(k, w)| !w.is_empty() && may_name(w, k > 0)))
            })
        })
}

/// Commands that only read files (the design addendum's closed list). Anything not on it is not a read.
/// (`file -C` writes a magic file: not on the list. `less`/`more` are, but never with a log-file option `-o`/`-O`/`--log-file`.)
const READ_ONLY_BASH: &[&str] = &["cat", "ls", "head", "tail", "grep", "egrep", "fgrep", "rg", "wc", "stat", "type", "sha256sum", "md5sum", "jq", "echo", "less", "more"];
/// `git` subcommands that only read (with no `--output`); every other git use is not a read.
const READ_ONLY_GIT: &[&str] = &["log", "show", "diff", "status", "blame", "ls-files"];
const READ_ONLY_PS: &[&str] = &["get-content", "gc", "cat", "type", "get-childitem", "gci", "dir", "ls", "get-item", "gi", "test-path", "select-string", "sls", "resolve-path", "measure-object", "get-filehash"];

/// The whole command is reads only: no subshell, substitution, heredoc or backtick, and every piece is a command on the closed read list
/// with no redirect, no assignment and no folder change. Pipes between reads are fine.
fn proven_reads_only(cmd: &str, ps: bool) -> bool {
    let parsed = if ps { shell::parse_ps(cmd) } else { shell::parse(cmd) };
    if parsed.complex || !parsed.substs.is_empty() || cmd.contains(['`', '(', ')']) && !ps || cmd.contains("<<") {
        return false;
    }
    // a comment with a quote or a second line can hide the next line from the parser (B RECHECK 8): never proven reads only
    if cmd.contains('#') && (cmd.contains(['\n', '\r', '\'', '"'])) {
        return false;
    }
    let list = if ps { READ_ONLY_PS } else { READ_ONLY_BASH };
    parsed.segments.iter().all(|seg| {
        let words: Vec<&str> = seg.words.iter().map(|w| w.text.as_str()).collect();
        let Some(first) = words.first() else { return true };
        seg.redirects.iter().all(|r| r.as_deref().is_some_and(is_null_sink))
            && !first.contains('=')
            // an option that runs or writes something (`grep --pre`, `rg --pre`, `--output`, `-exec`) is never a read
            && !words.iter().any(|w| w.starts_with("--pre") || w.starts_with("--output") || w.starts_with("-exec") || w.starts_with("--exec"))
            // less/more: a log-file option, also inside a bundle (`-So`, `-SO`), or a `+` start-up command is never a read
            && !(matches!(base(first).as_str(), "less" | "more")
                && words[1..].iter().any(|w| {
                    w.starts_with('+') || w.to_ascii_lowercase().starts_with("--log") || (w.starts_with('-') && !w.starts_with("--") && w.contains(['o', 'O']))
                }))
            // rg: options that run another program (`--hostname-bin`, `--pre`, `--search-zip`/`-z` decompressors) are never a read
            && !(base(first) == "rg" && words[1..].iter().any(|w| w.starts_with("--hostname-bin") || w.starts_with("--search-zip") || (w.starts_with('-') && !w.starts_with("--") && w.contains('z'))))
            && (list.contains(&base(first).as_str())
                || (!ps && base(first) == "git" && words.get(1).is_some_and(|s| READ_ONLY_GIT.contains(s)) && !words.iter().any(|w| w.starts_with("--output"))))
            && !(ps && words.iter().any(|w| w.starts_with('$') && words.get(1) == Some(&"=")))
    })
}

/// The command text with quoting undone the way the shell would before it uses a word: string joins (`"a" + "b"`, `'a''b'`) closed,
/// `$` before a quote dropped (`$'o'` → `o`), quote characters and escapes (backslash in Bash, backtick in PowerShell) removed.
/// Used only to look for `.olp`; never to name a target.
fn flattened(cmd: &str, ps: bool) -> String {
    let c: Vec<char> = cmd.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if "'\"".contains(ch) {
            // a closing quote, blanks, `+`, blanks, an opening quote = one joined string
            let mut j = i + 1;
            while j < c.len() && c[j].is_whitespace() {
                j += 1;
            }
            if j < c.len() && c[j] == '+' {
                let mut k = j + 1;
                while k < c.len() && c[k].is_whitespace() {
                    k += 1;
                }
                if k < c.len() && "'\"".contains(c[k]) {
                    i = k + 1;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if !ps && ch == '$' && c.get(i + 1) == Some(&'\'') {
            // ANSI-C quoting: decode `\xHH`, `\uHHHH`, `\UHHHHHHHH` and `\NNN` the way bash does (second review RECHECK 11:
            // `$'\x6f'` = `o`); other escapes keep their letter
            let mut j = i + 2;
            while j < c.len() && c[j] != '\'' {
                if c[j] == '\\' && j + 1 < c.len() {
                    let (radix, max, start) = match c[j + 1] {
                        'x' => (16, 2, j + 2),
                        'u' => (16, 4, j + 2),
                        'U' => (16, 8, j + 2),
                        '0'..='7' => (8, 3, j + 1),
                        _ => (0, 0, j + 1),
                    };
                    if radix == 0 {
                        out.push(c[j + 1]);
                        j += 2;
                        continue;
                    }
                    let digits: String = c[start..].iter().take(max).take_while(|d| d.is_digit(radix)).collect();
                    match u32::from_str_radix(&digits, radix).ok().and_then(char::from_u32) {
                        Some(d) if !digits.is_empty() => out.push(d),
                        _ => out.push(c[j + 1]),
                    }
                    j = start + digits.len();
                    continue;
                }
                out.push(c[j]);
                j += 1;
            }
            i = j + 1;
            continue;
        }
        if ch == '$' && c.get(i + 1).is_some_and(|n| "'\"".contains(*n)) {
            i += 1;
            continue;
        }
        if (!ps && ch == '\\') || (ps && ch == '`') {
            i += 1;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

fn assess_shell_parsed(cmd: &str, map: &DeviceMap, cwd: &Path, limits: Limits, ps: bool) -> Assessment {
    let parsed = if ps { shell::parse_ps(cmd) } else { shell::parse(cmd) };
    if parsed.complex {
        return builds_itself(cmd, cwd, ps);
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
        && !self_guard(&raw_words(cmd, ps).into_iter().chain(blind_words(cmd)).collect::<Vec<_>>(), built_base(cmd, cwd, ps), cmd)
        // Ledger 1321 ①: the whole command is a bundle, so its inner commands are judged without remembered answers.
        && parsed.substs.iter().all(|s| assess_shell(s, &DeviceMap::default(), cwd, limits, ps).is_read());
    if ok {
        outer
    } else {
        // the outer verdict's self-protection is kept (second review RECHECK 3 of rel 61983f0d3: never dropped on this path)
        let mut b = builds_itself(cmd, cwd, ps);
        b.self_guard |= outer.self_guard;
        // and so is each inner command's, read by the real parser (second review RECHECK 4 of rel 622fe5781, ⓓ)
        b.self_guard |= parsed.substs.iter().any(|s| assess_shell(s, &DeviceMap::default(), cwd, limits, ps).self_guard);
        b
    }
}

/// The folder relative targets of a built command are judged against: the working folder, unless a word moves the folder.
fn built_base<'a>(text: &str, cwd: &'a Path, ps: bool) -> Option<&'a Path> {
    (!raw_words(text, ps).iter().any(|w| moves_folder(w))).then_some(cwd)
}

/// Command substitution, heredoc, etc.: scan the whole text deeply for danger kinds only; otherwise block as not answerable by a group answer.
fn builds_itself(cmd: &str, cwd: &Path, ps: bool) -> Assessment {
    let mut a = Assessment::default();
    let deep = shell::deep_words([cmd]);
    for k in detect_dangers(&deep) {
        a.findings.push(Finding { kind: k, count: Count::unknown(k.unit(), "the command builds parts of itself at run time") });
    }
    // one check for every path: the real words and the working folder (second review RECHECK 3 of rel 61983f0d3). The whole text is
    // judged, heredoc bodies too: a second heredoc reader read `<<` differently from bash and hid later commands (RECHECK 4, B RECHECK 6).
    let base = built_base(cmd, cwd, ps);
    let words: Vec<String> = raw_words(cmd, ps).into_iter().chain(blind_words(cmd)).filter(|w| !glob_above_olp_only(w, base)).collect();
    a.self_guard |= touches_self(&deep) || self_guard(&words, base, cmd) || mentions_olp_component(cmd);
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

/// Shell words that open a block (loops, branches, functions, PowerShell try).
const BUNDLE_WORDS: &[&str] = &[
    "if", "then", "elif", "else", "fi", "for", "while", "until", "do", "done", "case", "esac", "select", "function", "elseif", "try", "catch", "finally",
    "foreach", "switch", "trap", "filter",
];

/// Ledger 1321 ① as an allow list (coordinator, design 1-15 «allowed characters only»): a remembered answer may be used only when every
/// character of the raw command is one of `A-Z a-z 0-9`, a space, or `. _ - / \ = : +`, and no word is a block word (`%` `@` `,` are out —
/// cmd.exe expands `%X%` when a .cmd/.bat runs, PowerShell `@env:X`/`@a` expand a variable and splits `git ,push` into `git push`, and `&`
/// inside an expanded value splits the line). Anything else —
/// quotes of any kind, `;` `|` `&` `$` backticks, redirections, tabs, control or full-width characters, non-ASCII — makes the command a
/// bundle: it never uses or offers a remembered answer and asks every time. Nothing is parsed, so no shell's quoting can open a hole.
/// `_ps` is kept so both shells share one call shape.
pub fn bundled(cmd: &str, _ps: bool) -> bool {
    let allowed = |ch: char| ch.is_ascii_alphanumeric() || " ._-/\\=:+".contains(ch);
    !cmd.chars().all(allowed) || cmd.split(' ').any(|w| BUNDLE_WORDS.contains(&w.to_ascii_lowercase().as_str()))
}

/// Coordinator decision after second review RECHECK c501bf669 (third hole-by-hole round, principle 7: a device, not more sentences):
/// a remembered answer and the built-in read pass are used only for a simple form — the whole command is one program name and its
/// arguments. Anything else asks every time:
/// - shell grammar: `;` `|` `&` `(` `)` `{` `}` backtick or a line break anywhere in the raw text, or a leading grammar word
///   (`!` `if` `while` `for` `case` `[[` …);
/// - a wrapper that runs another program (env, exec, sudo, command, builtin, nice, time, nohup, xargs, timeout, …);
/// - a builtin that changes variables or the shell (printf -v, read, declare, typeset, export, local, set, source, `.`, …);
/// - an assignment anywhere (`NAME=value`; PowerShell `$x = …`, `$env:X=…`).
/// Nothing depends on where the real command starts, so the place a check looks at cannot differ from the place the shell starts
/// the program (the root of RECHECK c501bf669 blocking 1-3).
const NOT_SIMPLE_BASH: &[&str] = &[
    // grammar words
    "!", "[[", "if", "then", "elif", "else", "fi", "for", "while", "until", "do", "done", "case", "esac", "select", "function", "coproc",
    // wrappers
    "env", "exec", "sudo", "doas", "su", "runuser", "command", "builtin", "nice", "time", "nohup", "xargs", "timeout", "stdbuf", "ionice",
    "chrt", "taskset", "setsid", "unbuffer", "watch", "strace", "ltrace", "eval", "chroot", "nsenter", "unshare", "flock",
    // builtins that change variables or the shell
    "read", "readarray", "mapfile", "declare", "typeset", "export", "local", "readonly", "set", "unset", "source", ".", "alias", "unalias",
    "shopt", "enable", "hash", "let", "getopts", "trap",
    // never a built-in read (rc4 rev 17): test, [ and [[ evaluate array subscripts as arithmetic; echo prints arithmetic too
    "test", "[", "echo",
];
const NOT_SIMPLE_PS: &[&str] = &[
    "&", ".", "start-process", "saps", "start", "invoke-expression", "iex", "invoke-command", "icm", "set-variable", "sv", "new-variable", "nv",
    "clear-variable", "clv", "remove-variable", "rv", "set-item", "si", "if", "elseif", "else", "for", "foreach", "while", "do", "switch",
    "try", "catch", "finally", "trap", "filter", "function",
    "echo",
];

fn ps_assignment(w: &str) -> bool {
    matches!(w, "=" | "+=" | "-=" | "*=" | "/=" | "??=") || (w.starts_with('$') && w.contains('='))
}

/// One piece (no separator inside) is a program name and its arguments only.
fn simple_piece(words: &[String], ps: bool) -> bool {
    let Some(first) = words.first() else { return true };
    let b = base(first);
    if ps {
        !NOT_SIMPLE_PS.contains(&b.as_str()) && !words.iter().any(|w| ps_assignment(w))
    } else {
        !NOT_SIMPLE_BASH.contains(&b.as_str()) && !(b == "printf" && words.iter().skip(1).any(|w| w.starts_with("-v"))) && !words.iter().any(|w| is_assignment(w))
    }
}

/// Simple form (see NOT_SIMPLE_BASH).
/// `whole` (a remembered answer): one piece, no separator character, no `$( )`; its characters are judged by `bundled`'s own allow
/// list.
/// Otherwise (the built-in read pass, rc4 rev 17-2 — decide what is allowed, not what is blocked): `read_text_pieces` must accept the
/// raw text, it must split into exactly as many pieces as the parser found, every piece is simple, and a piece with an unquoted glob
/// runs a command from GLOB_READS.
fn simple_form(cmd: &str, parsed: &shell::Parsed, ps: bool, whole: bool) -> bool {
    let words = |s: &shell::Segment| s.words.iter().map(|w| w.text.clone()).collect::<Vec<_>>();
    if whole {
        return !cmd.chars().any(|c| ";|&\n\r".contains(c)) && parsed.segments.len() <= 1 && parsed.substs.is_empty() && parsed.segments.iter().all(|s| simple_piece(&words(s), ps));
    }
    let Some(globs) = read_text_pieces(cmd, ps) else { return false };
    parsed.substs.is_empty()
        && globs.len() == parsed.segments.len()
        && parsed.segments.iter().zip(&globs).all(|(s, &glob)| {
            let w = words(s);
            simple_piece(&w, ps) && (!glob || w.first().is_some_and(|f| GLOB_READS.contains(&base(f).as_str())))
        })
}

/// Commands that have no option which writes, deletes or runs anything, so a glob that expands to a name like `--output=x` or
/// `-delete` still only reads (rev 17-2 ④). Not here: sort (-o), sed (-i), git (diff --output), rg (--pre), find (-delete, -exec),
/// file (-C), tee, and test, [ and echo.
const GLOB_READS: &[&str] = &["ls", "cat", "head", "tail", "wc", "grep", "du", "get-childitem", "gci", "dir", "get-content", "gc", "select-string", "sls"];

/// Output discarded, never written to a file (rev 17-2 ③): exactly these words, nothing else with `>` or `<`.
const NULL_REDIRECTS: [&str; 4] = ["2>/dev/null", ">/dev/null", "2>&1", "&>/dev/null"];

/// The allow list of the built-in read pass, read character by character (rc4 rev 17-2). Outside quotes: letters, digits, Hangul,
/// spaces and `- _ . / = : , @ + %`, the separators `;` `|` `&` between non-empty pieces, an unquoted `*` or `?` (glob, see
/// GLOB_READS) and the four NULL_REDIRECTS words. `'…'`: anything (Bash keeps it literally; PowerShell also ends a string at a
/// typographic quote, so those are refused inside). `"…"`: anything except `$`, backtick, backslash, `!` and typographic quotes. Every other
/// character — `$`, backslash, brackets of any kind, `~ ! # ^ < >`, backticks, tabs, line breaks, other spaces — refuses.
/// Returns, per piece, whether it has an unquoted glob; None = ask.
fn read_text_pieces(cmd: &str, ps: bool) -> Option<Vec<bool>> {
    let c: Vec<char> = cmd.chars().collect();
    let plain = |ch: char| ch.is_ascii_alphanumeric() || "-_./=:,@+%".contains(ch) || is_hangul(ch);
    let typographic = |ch: &char| matches!(*ch, '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{201c}' | '\u{201d}' | '\u{201e}');
    let mut globs = vec![false];
    let mut filled = false;
    let mut word_start = true;
    let mut i = 0;
    while i < c.len() {
        if word_start {
            let hit = NULL_REDIRECTS.iter().find(|r| {
                let rc: Vec<char> = r.chars().collect();
                c[i..].starts_with(&rc) && c.get(i + rc.len()).is_none_or(|n| *n == ' ' || *n == ';' || *n == '|')
            });
            if let Some(r) = hit {
                i += r.chars().count();
                word_start = false;
                filled = true;
                continue;
            }
        }
        let ch = c[i];
        match ch {
            ' ' => {
                word_start = true;
                i += 1;
            }
            ';' | '|' | '&' => {
                if !filled {
                    return None;
                }
                let run: String = c[i..].iter().take_while(|x| ";|&".contains(**x)).collect();
                if !matches!(run.as_str(), ";" | "|" | "&" | "&&" | "||") {
                    return None;
                }
                i += run.chars().count();
                globs.push(false);
                filled = false;
                word_start = true;
            }
            '\'' => {
                let len = c[i + 1..].iter().position(|&x| x == '\'')?;
                if ps && c[i + 1..i + 1 + len].iter().any(typographic) {
                    return None;
                }
                i += len + 2;
                word_start = false;
                filled = true;
            }
            '"' => {
                let len = c[i + 1..].iter().position(|&x| x == '"')?;
                if c[i + 1..i + 1 + len].iter().any(|x| "$`\\!".contains(*x) || typographic(x)) {
                    return None;
                }
                i += len + 2;
                word_start = false;
                filled = true;
            }
            '*' | '?' => {
                *globs.last_mut()? = true;
                i += 1;
                word_start = false;
                filled = true;
            }
            _ if plain(ch) => {
                i += 1;
                word_start = false;
                filled = true;
            }
            _ => return None,
        }
    }
    filled.then_some(globs)
}

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}')
}

/// `NAME=value` with a shell variable name (letters, digits, `_`, not starting with a digit).
fn is_assignment(w: &str) -> bool {
    w.split_once('=')
        .map(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.starts_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(false)
}

fn assess_parsed(cmd: &str, parsed: &shell::Parsed, map: &DeviceMap, cwd: &Path, limits: Limits, ps: bool) -> Assessment {
    let follow_cd = !ps;
    let is_bundle = bundled(cmd, ps);
    // Simple form only (coordinator decision after RECHECK c501bf669, path B; replaces the front-assignment rule of RECHECK 21880f55a):
    // a remembered answer needs the whole command to be one program name with its arguments; the built-in read pass needs every piece
    // to be one (A would add about 998 asks a day on this PC, B about 216 = 3.7 % of about 5,900 calls).
    let simple = simple_form(cmd, parsed, ps, true);
    let simple_pieces = simple_form(cmd, parsed, ps, false);
    let mut exec_names: Vec<String> = Vec::new();
    // A remembered read answer (already limited to a whole simple command by `bundled`) is not a built-in read, so the read-pass
    // allow list at the end does not apply to it (rev 17-2: a saved answer with a Windows path keeps working).
    let mut remembered = false;
    let ctx = Ctx::new(limits);
    let mut a = Assessment::default();
    let mut dir = Dir::Known { path: cwd.to_path_buf(), moved: false };
    let mut moved_dir = false;
    let segs = &parsed.segments;
    // Design 1-12 §2-4d ⑤: whether each piece is a proven read (closed table first word, read by the built-in rules, and
    // joined to the next piece by `;`/newline, `&&` or `||`). A later interpreter or runner key needs every earlier piece proven.
    let proven: Vec<bool> = segs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w: Vec<String> = s.words.iter().map(|x| x.text.clone()).collect();
            let safe: Vec<bool> = s.words.iter().map(|x| x.literal && !x.brace).collect();
            let joined_ok = segs.get(i + 1).is_none_or(|n| matches!(n.joined_by, Join::Seq | Join::AndAnd | Join::OrOr));
            let table = if ps { PROVEN_READ_PS } else { PROVEN_READ_BASH };
            let first_ok = w.first().is_some_and(|f| !pathed(f) && table.contains(&f.to_lowercase().as_str()));
            joined_ok && first_ok && is_read_segment(&w, &safe, ps)
        })
        .collect();
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
                    let base_dir = match &dir {
                        Dir::Known { path, .. } => Some(path.as_path()),
                        Dir::Unknown(_) => None,
                    };
                    a.self_guard |= target_is_olp_state(t, base_dir, cmd) || in_olp;
                    let c = match (&dir, ctx.deadline()) {
                        (Dir::Unknown(why), _) => Count::unknown(Unit::File, why),
                        (Dir::Known { path, .. }, Some(d)) => count_or_unknown(Unit::File, counter::count_paths(path, &[t.as_str()], d)),
                        (Dir::Known { .. }, None) => Count::unknown(Unit::File, NO_LIMIT),
                    };
                    a.findings.push(Finding { kind: Kind::FileOverwrite, count: c });
                }
                None => {
                    // a target written with a variable (`> $HOME/olp/...`) is not resolved, so the whole text decides self-protection
                    // the parser gives no name for a glob or variable target: take the words after `>` from the raw text and judge them
                    // on the same path as `tee`/`cp` (second review RECHECK 1 of rel dd79e582c, blocking 2)
                    let base_dir = match &dir {
                        Dir::Known { path, .. } => Some(path.as_path()),
                        Dir::Unknown(_) => None,
                    };
                    a.self_guard |= target_is_olp_state("", None, cmd) || self_guard(&[], base_dir, cmd) || in_olp;
                    a.findings.push(Finding { kind: Kind::FileOverwrite, count: Count::unknown(Unit::File, "the target uses patterns or variables") })
                }
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
        // Design 1-8 §2-1c rule 4 (closed): any of the 8 folder-move words earlier in the command, whatever joins the pieces
        // (also `(cd x; ...)`), means a later interpreter key takes no group answer.
        // Review RECHECK 1 blocking 1: the folder-move word is looked up where the command really starts, after leading syntax
        // words (`{`, `if`, `then`, `while`, `do`, `!` via bash_cmd_start) and, for PowerShell, in every piece split by ps_parts.
        // PowerShell also has the built-in functions `cd..` and `cd\` (second review blocking 1).
        let is_cd = |w: &String| {
            let l = w.to_lowercase();
            CD_VERBS.contains(&base(w).as_str()) || l.starts_with("cd..") || l.starts_with("cd\\") || l.starts_with("cd~")
        };
        let lead_cd = if ps {
            ps_parts(&words).iter().any(|(_, p)| p.first().is_some_and(is_cd))
        } else {
            bash_cmd_start(&words).and_then(|o| words.get(o)).is_some_and(is_cd)
        };
        if changes_dir || lead_cd || CD_VERBS.contains(&b0.trim_start_matches(['(', '{'])) {
            moved_dir = true;
        }
        if changes_dir {
            dir = if ps { ps_after_cd(&dir, seg) } else { after_cd(&dir, seg, segs.get(i + 1).map(|s| s.joined_by), follow_cd) };
        }
        // PowerShell pieces are split at control words and braces and each command is inspected separately ("if" is not a command name; the command after `... } else { mytool ...` is inspected too)
        let start = words.len() - stripped.len();
        // A wrapper folder flag (`env -Cx`, `env --chdir=x`, `sudo -D x`) runs the program in another folder (design 1-8).
        // Wrapper words are those before the real command start (after leading syntax words too: `! env -Cx node a.js`).
        let cmd_at = if ps { start } else { bash_cmd_start(&words).unwrap_or(start).max(start) };
        let wrapper_moved = words[..cmd_at].iter().any(|w| w == "--chdir" || w.starts_with("--chdir=") || w.starts_with("-C") || w.starts_with("-D"));
        // Design 1-11 §2-4d: every word before the program must be understood (leading syntax words aside).
        let lead = if ps { 0 } else { words.iter().take_while(|x| BASH_LEAD.contains(&x.as_str())).count().min(cmd_at) };
        let prefix_understood = understood_prefix(&words[lead..cmd_at]);
        let parts: Vec<(usize, &[String])> = if ps { ps_parts(stripped).into_iter().map(|(o, p)| (start + o, p)).collect() } else { bash_cmd_start(stripped).map(|o| (start + o, &stripped[o..])).into_iter().collect() };
        let safe: Vec<bool> = seg.words.iter().map(|w| w.literal && !w.brace).collect();
        let first_part_at = parts.first().map(|(at, _)| *at);
        let unread: Vec<(usize, &[String])> = parts.into_iter().filter(|(at, p)| !is_read_segment(p, safe.get(*at..*at + p.len()).unwrap_or(&[]), ps) && !(ps && ps_string_output(seg, *at, p))).collect();
        let Some(&(first_at, first)) = unread.first() else { continue };
        let deep = shell::deep_words(words.iter().map(|s| s.as_str()));
        let base_dir = match &dir {
            Dir::Known { path, .. } => Some(path.as_path()),
            Dir::Unknown(_) => None,
        };
        a.self_guard |= self_guard(&words, base_dir, cmd) || touches_self(&deep) || in_olp;
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
        // Second review RECHECK 9a54918df blocking 2: a PowerShell piece split by ps_parts (try/finally, if, braces) is a group
        // the parser did not fully follow. A part that is not the piece's first part, or whose own leading words are not
        // understood, takes no group answer for an interpreter or runner key.
        let ps_unproven_parts: Vec<usize> = if ps {
            unread
                .iter()
                .filter(|(at, p)| {
                    let s = strip_prefix_words(p);
                    Some(*at) != first_part_at || !understood_prefix(&p[..p.len() - s.len()])
                })
                .map(|(at, _)| *at)
                .collect()
        } else {
            Vec::new()
        };
        for (fam, at) in fams {
            let first_literal = seg_literal(seg, at);
            let ps_number = ps && ps_number_literal(fam.split(' ').next().unwrap_or(""));
            // A path-qualified first word (`./mytool`, `bin/mytool`) runs that file, so it never takes the bare name's answer (review memo M1).
            let pathed_name = seg.words.get(at).is_some_and(|w| pathed(&w.text));
            // An interpreter key after a folder move in the same command names another file (design 1-5 §2-1c rule 4),
            // and its script path is written from the project root (design 1-6, list row i16).
            // A runner key (`bun x cowsay`) names a package, not a file: no folder rule and no root path (review RECHECK 1 blocking 2).
            let script_key = interpreter_family(&fam) && !runner_family(&fam);
            let moved_script = (moved_dir || wrapper_moved) && script_key;
            // Second review blocking 2: stripping stopped on a wrapper flag's value (`env -C x node a.js` -> `x node`); when an
            // interpreter or runner follows, the real program is behind the value, so the piece takes no group answer.
            let flag_value = !ps
                && at > 0
                && words.get(at - 1).is_some_and(|w| w.starts_with('-'))
                && words.iter().skip(at + 1).any(|w| interpreter_name(&base(w)).is_some() || runner_program(&base(w)).is_some());
            let earlier_unproven = proven[..i].iter().any(|p| !p) || ps_unproven_parts.contains(&at);
            let moved_script = moved_script || flag_value || ((!prefix_understood || earlier_unproven) && closed_grammar_family(&fam));
            // Second review RECHECK 3f5b6b3ef blocking 1: a one-word key taken from a command whose next word is a flag
            // (`git -c x=y status`, `git -C x push`) drops the subcommand, so one answer would cover every form: a read answer never
            // passes it and the card never offers it. A stored non-read answer (`psql` writes_other with a declared row count) still
            // applies, since it never lets a call through without asking.
            // RECHECK 21880f55a blocking 1: any second word that did not become part of the key (`,push`, `-C`, `x.txt`) drops the
            // subcommand, so the one-word answer does not cover it.
            let flagged_one_word = !fam.contains(' ')
                && seg.words.get(at + 1).is_some()
                && map.entries.get(&format!("bash:{fam}")).is_none_or(|e| e.effect == Effect::ReadOnly);
            let fam = if script_key && fam.contains(' ') { rooted_interpreter_key(&fam, cwd) } else { fam };
            if !family_is_name(&fam) || !first_literal || ps_number || pathed_name || moved_script || flagged_one_word || is_bundle || !simple {
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
                Some(e) if e.effect == Effect::ReadOnly => remembered = true,
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
                if kinds.is_empty() && unknown.is_empty() && simple_pieces {
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
    // Only a call that would otherwise pass is turned into an ask; a card that already asks keeps its own text.
    // rev 17-3: no file inside the project (not even an exact "always allow" rule in .claude/settings*.json) can turn this ask into a
    // pass; a cloned repository could plant one.
    if !simple_pieces && a.is_read() && !remembered {
        a.unnamed = true;
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
        let mut a = Assessment { self_guard: target.map(|t| target_is_olp_state(t, Some(cwd), t)).unwrap_or(false), ..Default::default() };
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
        assert_eq!(f("python script.py"), "python script.py");
        assert_eq!(f("python -m pytest"), "python");
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
        // A piece of only braces (`}`) has no command to run, but a brace anywhere means the split cannot be trusted, so it is not a
        // simple form and asks (rc4 rev 15, «simple form only»); it still gives 0 candidates (removed from the junk list below)
        assert!(!run("PowerShell", "}").is_read(), "a lone brace is not a simple form, so it asks (rc4 rev 15)");
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
        assert_eq!(run("Bash", "2to3").to_classify, ["2to3"]);
        // A second word that is not part of the key (a flag, a file name, `,push`) leaves a one-word key without its subcommand, so it takes
        // no group answer (rc4 second review RECHECK 3f5b6b3ef blocking 1 · RECHECK 21880f55a blocking 1)
        assert!(run("Bash", "2to3 -w x.py").to_classify.is_empty());
        assert!(run("Bash", "2to3 x.py").to_classify.is_empty());
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
