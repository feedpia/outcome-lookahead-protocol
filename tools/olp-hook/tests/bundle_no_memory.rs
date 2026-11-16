//! 0.1.3 rc4, ledger 1321 ①: a command with any bundle sign (`;` `&&` `||` `|` `&` `{ }` `( )` `$( )` backticks, `if`/`try`/loops,
//! PowerShell `&{ }`) never takes a remembered answer and never offers one: it asks every time.
//! The table is generated (wrapper × bundle × program) so a new wrapper or bundle shape is one entry, not one test (principle 7).
//! All file operations happen only in throwaway folders under CARGO_TARGET_TMPDIR.
use olp_hook::classify::{self, Limits};
use olp_hook::{run_hook, store};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// Programs with the answer stored for them (read_only), as the card would have offered it.
const PROGRAMS: [(&str, &str); 6] =
    [("git -C x push", "git"), ("mytool run", "mytool run"), ("node a.js", "node a.js"), ("npx cowsay", "npx cowsay"), ("make build", "make build"), ("mytool -l", "mytool")];
const WRAPPERS: [&str; 5] = ["", "sudo ", "env ", "env -i ", "command "];
const BASH_BUNDLES: [&str; 14] = [
    // bash ANSI-C quote: `$'\''` is one closed string, so the `;` after it is a real bundle sign (B RECHECK db23b532c blocking 1)
    r"X $'\'' ; echo \'",
    "X; ls", "ls; X", "ls && X", "X || true", "ls | X", "X | cat", "{ X; }", "(X)", "echo $(X)", "echo `X`", "if true; then X; fi", "X &", "for f in a; do X; done",
];
const PS_BUNDLES: [&str; 7] = ["X; Get-ChildItem", "if ($true) { X }", "try { X } finally { }", "& { X }", "X | Out-Null", "Write-Output $(X)", "foreach ($f in 1) { X }"];

fn project(stored: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_bundle_no_memory").join(format!("{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::create_dir_all(p.join("x")).unwrap();
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    // rev 17-3: answers live in the user's trust folder, not in the project
    let t = store::trust_dir(&p);
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    fs::write(t.join("hook-map.json"), json!({"version": 1, "entries": {format!("bash:{stored}"): ro}}).to_string()).unwrap();
    p
}

fn check(tool: &str, cmd: &str, stored: &str, bad: &mut Vec<String>) {
    let p = project(stored);
    let map = store::load_map(&p).unwrap();
    let a = classify::assess(tool, &json!({ "command": cmd }), &map, &p, Limits { counter: Some(Duration::from_secs(3)), ps_parser: true });
    if !a.to_classify.is_empty() {
        bad.push(format!("{tool} {cmd}: offers {:?}", a.to_classify));
    }
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_b"});
    let o = run_hook(v.to_string().as_bytes(), Some(&p));
    if o.code == 0 {
        bad.push(format!("{tool} {cmd}: the stored answer for {stored:?} let it pass"));
    }
    if format!("{}{}", o.stderr, o.stdout).contains("classify --bash") {
        bad.push(format!("{tool} {cmd}: card offers a remembered answer"));
    }
}

// Value: protects=ledger 1321 ① — any bundle sign means no remembered answer is used or offered, for every wrapper × bundle × program; fails_when=one shape still passes on a stored read answer or the card offers `classify --bash`; seam=classify::assess + run_hook
#[test]
fn bundled_commands_never_remember() {
    let mut bad = Vec::new();
    let mut n = 0;
    for (prog, stored) in PROGRAMS {
        for w in WRAPPERS {
            for b in BASH_BUNDLES {
                check("Bash", &b.replace('X', &format!("{w}{prog}")), stored, &mut bad);
                n += 1;
            }
            for b in PS_BUNDLES {
                check("PowerShell", &b.replace('X', &format!("{w}{prog}")), stored, &mut bad);
                n += 1;
            }
        }
    }
    println!("bundle table: {n} shapes");
    assert!(bad.is_empty(), "{} of {n} shapes remember:\n{}", bad.len(), bad.join("\n"));
}

// Value: protects=ledger 1321 ① taken literally (second review RECHECK 39e4549d9: quote-reading holes 3 times) — a bundle sign anywhere in the
// raw text, inside any quote of any shell (straight, curly, `$'…'`, `${…}`), means no remembered answer; fails_when=a quoted sign still lets a stored answer pass; seam=classify::assess + run_hook
#[test]
fn quoted_bundle_signs_never_remember() {
    let signs = [";", "|", "&", "{", "}", "(", ")", "$(", "`", "\n"];
    let quotes: [(&str, &str, &str); 7] = [
        ("Bash", "\"", "\""),
        ("Bash", "'", "'"),
        ("Bash", "$'", "'"),
        ("PowerShell", "\"", "\""),
        ("PowerShell", "'", "'"),
        ("PowerShell", "\u{2018}", "\u{2019}"),
        ("PowerShell", "\u{201C}", "\u{201D}"),
    ];
    let mut bad = Vec::new();
    for (tool, open, close) in quotes {
        for s in signs {
            check(tool, &format!("mytool run {open}a{s}b{close}"), "mytool run", &mut bad);
        }
    }
    assert!(bad.is_empty(), "{} quoted shapes remember:\n{}", bad.len(), bad.join("\n"));
}

// Value: protects=allow-list rule (coordinator, ledger 1321 ① — «no fine-tuning per shell»): a remembered answer is used only when every
// character of the raw command is in [A-Za-z0-9 space . _ - / \ = : +] (`%` `@` `,` removed); anything else (quotes, full-width signs, control characters,
// PowerShell backtick, `$`, redirections, tabs, non-ASCII) means ask every time; fails_when=one character outside the list still lets a stored answer pass; seam=classify::assess + run_hook
#[test]
fn characters_outside_the_allow_list_never_remember() {
    let odd = [
        "\u{FF1B}", "\u{FF5C}", "\u{FF06}", "\r", "\t", "\u{0}", "\u{1B}", "`", "$", "*", "?", "~", "!", "#", "<", ">", "[", "]", "^", "'", "\"",
        "\u{2018}", "\u{2019}", "\u{201C}", "\u{201D}", "\u{00A0}", "\u{3000}", "\u{D55C}", "\u{200B}",
    ];
    let mut bad = Vec::new();
    for tool in ["Bash", "PowerShell"] {
        for o in odd {
            check(tool, &format!("mytool run a{o}b"), "mytool run", &mut bad);
        }
    }
    check("Bash", "mytool run 2>&1", "mytool run", &mut bad);
    // `%` is not allowed: cmd.exe expands `%X%` when a .cmd/.bat runs and reads `&` inside the value as a separator (second review RECHECK 96ae2100a)
    check("Bash", "mytool run %X%", "mytool run", &mut bad);
    check("PowerShell", "mytool run %X%", "mytool run", &mut bad);
    // `,` is not allowed: PowerShell splits `git ,push` into `git push` (second review RECHECK 21880f55a)
    for t in ["Bash", "PowerShell"] {
        check(t, "mytool run a,b", "mytool run", &mut bad);
    }
    // `@` is not allowed: PowerShell `@env:X` / `@a` expand a variable, and the value reaches cmd.exe for a .cmd (second review RECHECK 0820557e9)
    for t in ["Bash", "PowerShell"] {
        check(t, "mytool run @env:X", "mytool run", &mut bad);
        check(t, "mytool run @a", "mytool run", &mut bad);
    }
    assert!(bad.is_empty(), "{} shapes outside the allow list remember:\n{}", bad.len(), bad.join("\n"));
}

// Value: protects=commands made only of allowed characters keep their remembered answer (no ask fatigue for plain commands, paths, flags, `npx @scope/tool`); fails_when=the allow list is narrower than designed; seam=classify::assess + run_hook
#[test]
fn single_commands_still_remember() {
    // `C:/x/y.txt`, not `C:\x\y.txt`: in Bash an unquoted `\` is dropped, so `C:\x\y.txt` is the drive-relative `C:xy.txt`, whose folder
    // (that drive's current folder) the hook does not know when the working folder is on another drive — design addendum 4-3 keeps it
    // closed on Windows (pinned below); elsewhere `C:xy.txt` is an ordinary relative name.
    for (cmd, stored) in [("mytool run", "mytool run"), ("mytool run src/a_b-c.txt", "mytool run"), ("mytool run C:/x/y.txt --k=v:1+2", "mytool run"), ("mytool run x-y", "mytool run")] {
        let p = project(stored);
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "Bash",
                       "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_s"});
        assert_eq!(run_hook(v.to_string().as_bytes(), Some(&p)).code, 0, "{cmd}");
    }
    let p = project("mytool run");
    let here = p.to_string_lossy().chars().next().unwrap_or('C').to_ascii_uppercase();
    let other = if here == 'C' { 'D' } else { 'C' };
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "Bash",
                   "tool_input": {"command": format!(r"mytool run {other}:\x\y.txt")}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_s"});
    // asks only when that other drive is the trust folder's drive (the home drive): another drive letter cannot reach it (B R26)
    let home_drive = std::env::var_os("USERPROFILE").and_then(|h| h.to_string_lossy().chars().next()).map(|c| c.to_ascii_uppercase());
    let asks = cfg!(windows) && home_drive == Some(other) && std::path::Path::new(&format!("{other}:\\")).exists();
    assert_eq!(run_hook(v.to_string().as_bytes(), Some(&p)).code != 0, asks, "{other}:xy.txt on another drive than the working folder");
}

/// The allowed characters that are left (design 1-20): each one before, after and between words keeps the remembered answer (no extra asks),
/// and next to any variable sign of bash, PowerShell or cmd.exe (`$X`, `${X}`, `$env:X`, `%X%`, `!X!`, `@X`) it never does.
// Value: protects=the allow list is pinned per character so the next review does not need another round (coordinator 11-14); fails_when=an
// allowed character opens a way around the list, or a plain use of it starts asking; seam=classify::assess + run_hook
#[test]
fn every_allowed_character_is_pinned() {
    let mut bad = Vec::new();
    let mut lost = Vec::new();
    // A Bash word `NAME=value` (NAME = [A-Za-z_][A-Za-z0-9_]*) is an assignment.
    let assignment = |w: &str| {
        w.split_once('=').is_some_and(|(k, _)| !k.is_empty() && !k.starts_with(|ch: char| ch.is_ascii_digit()) && k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'))
    };
    for c in ["\\", "=", ":", "+", ".", "_", "-", "/"] {
        for form in [format!("mytool run {c}x"), format!("mytool run x{c}"), format!("mytool run x{c}y")] {
            let p = project("mytool run");
            for tool in ["Bash", "PowerShell"] {
                // rc4 rev 15 (simple form only): an assignment anywhere asks, so a Bash form whose word became `NAME=value`
                // (`mytool run x=`, `mytool run x=y`) loses the remembered answer instead of keeping it.
                if tool == "Bash" && form.split(' ').any(assignment) {
                    check(tool, &form, "mytool run", &mut bad);
                    continue;
                }
                let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                               "tool_input": {"command": form}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_k"});
                if run_hook(v.to_string().as_bytes(), Some(&p)).code != 0 {
                    lost.push(format!("{tool} {form}"));
                }
            }
        }
        for var in ["$X", "${X}", "$env:X", "%X%", "!X!", "@X"] {
            for tool in ["Bash", "PowerShell"] {
                check(tool, &format!("mytool run x{c}{var}"), "mytool run", &mut bad);
                check(tool, &format!("mytool run {var}{c}x"), "mytool run", &mut bad);
            }
        }
    }
    // Key positions (second review RECHECK 21880f55a): a one-word stored answer never covers a second word that starts with any character,
    // and a leading `NAME=value` (or a name holding the character) never uses a remembered answer, for every remaining allowed character.
    // The second-word axis asks only that the stored one-word answer does not let the call pass: a word that is itself a valid key
    // (`mytool _push`, `mytool :push`) may still be offered as its own two-word key, which names the real word.
    for c in ["\\", "=", ":", "+", ".", "_", "-", "/", ","] {
        for tool in ["Bash", "PowerShell"] {
            for form in [format!("mytool {c}push"), format!("mytool {c}-rf x")] {
                let p = project("mytool");
                let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                               "tool_input": {"command": form}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_w"});
                if run_hook(v.to_string().as_bytes(), Some(&p)).code == 0 {
                    bad.push(format!("{tool} {form}: the stored one-word answer \"mytool\" let it pass"));
                }
            }
        }
        check("Bash", &format!("A{c}b=1 mytool run"), "mytool run", &mut bad);
        check("Bash", &format!("A=x{c}y mytool run"), "mytool run", &mut bad);
        check("Bash", &format!("env A=x{c}y mytool run"), "mytool run", &mut bad);
    }
    assert!(bad.is_empty(), "{} variable shapes remember:\n{}", bad.len(), bad.join("\n"));
    assert!(lost.is_empty(), "{} plain shapes lost their answer:\n{}", lost.len(), lost.join("\n"));
}
