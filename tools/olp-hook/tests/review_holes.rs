//! 0.1.3 rc4 rev 17: every shape the second reviews of rev 13 to rev 16 found, in one table. Each one must ask (exit code other
//! than 0), with no stored answer and with stored read answers, in Bash and PowerShell. Rev 17 replaces the growing list of
//! blocked characters with an allow list: the built-in read pass needs a read command (not test, [, [[ or echo) whose words use
//! only letters, digits, Hangul and `- _ . / = : , @ + %`; the pieces may be joined by `;`, `|` or `&`.
//! All file operations happen only in throwaway folders under CARGO_TARGET_TMPDIR.
use olp_hook::run_hook;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn project(stored: &[&str]) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_review_holes").join(format!("{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(p.join(".olp")).unwrap();
    fs::create_dir_all(p.join("build")).unwrap();
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    let entries: serde_json::Map<String, serde_json::Value> = stored.iter().map(|s| (format!("bash:{s}"), ro.clone())).collect();
    // rev 17-3: answers live in the user's trust folder, not in the project
    let t = olp_hook::store::trust_dir(&p);
    let _ = fs::remove_dir_all(&t);
    fs::create_dir_all(&t).unwrap();
    fs::write(t.join("hook-map.json"), json!({"version": 1, "entries": entries}).to_string()).unwrap();
    p
}

/// true = the call went through without asking.
fn passes(tool: &str, cmd: &str, stored: &[&str]) -> bool {
    let p = project(stored);
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_h"});
    run_hook(v.to_string().as_bytes(), Some(&p)).code == 0
}

const STORED: [&str; 5] = ["git", "git status", "ls", "mytool", "mytool run"];

/// (review that found it, shell, command)
const HOLES: &[(&str, &str, &str)] = &[
    // rev 13: a character the shell reads differently hides a subcommand or a variable
    ("rev 13", "PowerShell", "git ,push"),
    ("rev 13", "PowerShell", "git ,-C,x,push"),
    ("rev 13", "PowerShell", "mytool ,push"),
    ("rev 13", "Bash", "mytool run %X%"),
    ("rev 13", "PowerShell", "mytool run @a"),
    ("rev 13", "PowerShell", "mytool run $env:X"),
    // rev 14: an assignment in front of the program
    ("rev 14", "Bash", "PATH=x/bin git"),
    ("rev 14", "Bash", "PATH=x/bin git status"),
    ("rev 14", "Bash", "GIT_EXTERNAL_DIFF=./w.sh git diff"),
    ("rev 14", "Bash", "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0=./w.sh git status"),
    ("rev 14", "Bash", "env A=x mytool run"),
    ("rev 14", "Bash", "PATH=/tmp/x && ls"),
    ("rev 14", "Bash", "X=$(pwd) && ls build"),
    // rev 14 second review: after a leading word, behind a wrapper flag value, or set by a builtin
    ("rev 14b", "Bash", "! GIT_EXTERNAL_DIFF=./w.sh git diff"),
    ("rev 14b", "Bash", "{ PATH=x/bin git status; }"),
    ("rev 14b", "Bash", "if PATH=x/bin git status; then true; fi"),
    ("rev 14b", "Bash", "env -u ls PATH=x/bin git status"),
    ("rev 14b", "Bash", "exec -a ls rm -rf build"),
    ("rev 14b", "Bash", "env -u ls rm -rf build"),
    ("rev 14b", "Bash", "printf -v PATH x/bin; git status"),
    ("rev 14b", "Bash", "read PATH < p.txt; git status"),
    // rev 15 second review: arithmetic assignment
    ("rev 15", "Bash", "test -v 'x[PATH=5]'; git status"),
    ("rev 15", "Bash", "[ -v 'x[PATH=5]' ] || git status"),
    ("rev 15", "Bash", "echo $[PATH=5]; git status"),
    ("rev 15", "Bash", "ls $[PATH=5]; git status"),
    ("rev 15", "Bash", "ls $((PATH=5)); git status"),
    // rev 16 second review: the bracket hidden by ANSI-C quoting or matched by a glob
    ("rev 16", "Bash", r"test -v $'x\x5bPATH=5]' || git status"),
    ("rev 16", "Bash", "test -v x?PATH=5] || git status"),
    ("rev 16", "Bash", "test -v x*PATH=5] || git status"),
    ("rev 16", "Bash", "test -d build || git status"),
    ("rev 16", "Bash", "echo x || git status"),
    // rev 17-2 second review: `file -C` compiles a magic file and writes `<name>.mgc`
    ("rev 17-2b", "Bash", "file -C -m x"),
    ("rev 17-2b", "Bash", "file -C"),
    ("rev 17-2b", "Bash", "file -bC -m x"),
    ("rev 17-2b", "Bash", "file --compile -m x"),
    ("rev 17-2b", "Bash", "file --comp -m x"),
    ("rev 17-2b", "Bash", "ls build && file -C -m x"),
    // rev 17-2 edges: what stays outside the allow list
    ("rev 17-2", "Bash", "ls ~/x"),
    ("rev 17-2", "Bash", "ls build\u{3000}x"),
    ("rev 17-2", "Bash", "ls build\tx"),
    ("rev 17-2", "Bash", "ls build\nls x"),
    ("rev 17-2", "Bash", "ls !x"),
    ("rev 17-2", "Bash", "ls #x"),
    ("rev 17-2", "Bash", "grep ^x a.txt"),
    ("rev 17-2", "PowerShell", r"Get-ChildItem C:\x"),
    ("rev 17-2", "Bash", "ls $'build'"),
    ("rev 17-2", "Bash", "grep \"a$HOME\" a.txt"),
    ("rev 17-2", "Bash", "grep \"a`id`\" a.txt"),
    ("rev 17-2", "Bash", "grep \"a\\\"; git status; \" a.txt"),
    ("rev 17-2", "Bash", "grep \"a!x\" a.txt"),
    ("rev 17-2", "PowerShell", "Get-ChildItem 'a\u{2018}; Remove-Item x; \u{2018}b'"),
    ("rev 17-2", "PowerShell", "Get-ChildItem \"a\u{201c}; Remove-Item x; \u{201d}b\""),
    ("rev 17-2", "Bash", "ls build >out.txt"),
    ("rev 17-2", "Bash", "ls build 2>out.txt"),
    ("rev 17-2", "Bash", "ls build >>/dev/null"),
    ("rev 17-2", "Bash", "ls build 2>&1>out.txt"),
    ("rev 17-2", "Bash", "ls build < a.txt"),
    ("rev 17-2", "Bash", "; ls build"),
    ("rev 17-2", "Bash", "ls build ;; git status"),
    // a glob only for commands with no option that writes or runs anything (a name like `--output=x` may come out of it)
    ("rev 17-2", "Bash", "sort *.txt"),
    ("rev 17-2", "Bash", "sed -n p *.txt"),
    ("rev 17-2", "Bash", "git diff *"),
    ("rev 17-2", "Bash", "rg x *"),
    ("rev 17-2", "Bash", "find *"),
    ("rev 17-2", "Bash", "file *"),
    ("rev 17-2", "Bash", "ls build && sort ?.txt"),
];

// Value: protects=every hole the second reviews of rev 13-16 found stays closed, in one place (owner: «block the root», principle 7);
// fails_when=one shape goes through without asking, with or without a stored read answer; seam=run_hook (the real hook verdict)
#[test]
fn every_review_hole_asks() {
    let mut bad = Vec::new();
    for (rev, tool, cmd) in HOLES {
        for stored in [&[][..], &STORED[..]] {
            if passes(tool, cmd, stored) {
                bad.push(format!("[{rev}] {tool} {cmd:?} (stored {stored:?})"));
            }
        }
    }
    assert!(bad.is_empty(), "{} of {} shapes went through without asking:\n{}", bad.len(), HOLES.len() * 2, bad.join("\n"));
}

// Value: protects=the allow list keeps plain reads passing (ask fatigue); fails_when=a read made only of allowed characters, or a
// plain join of such reads, starts asking; seam=run_hook
#[test]
fn allowed_reads_still_pass() {
    let mut bad = Vec::new();
    for (tool, cmd) in [
        ("Bash", "git status"),
        ("Bash", "ls build"),
        ("Bash", "cd build && ls"),
        ("Bash", "git status && git diff"),
        ("Bash", "ls build | head -5"),
        ("Bash", "git log -1 --format=%h"),
        ("Bash", "ls 한글폴더"),
        ("Bash", "wc -l a.txt"),
        ("Bash", "file a.txt"),
        ("Bash", "file -b a.txt"),
        ("PowerShell", "Get-ChildItem build"),
        // rev 17-2: quoted text, the four discard words and a glob on a read-only command
        ("Bash", "grep -n 'a|b; c' a.txt"),
        ("Bash", "grep -n \"a b\" a.txt | head -5"),
        ("Bash", "grep 'x$' a.txt"),
        ("Bash", "grep \"^#\" a.txt"),
        ("Bash", "git log -1 --format='%h %s' && git status"),
        ("Bash", "ls build 2>/dev/null"),
        ("Bash", "ls build >/dev/null 2>&1"),
        ("Bash", "ls build &>/dev/null || true"),
        ("Bash", "ls *.rs"),
        ("Bash", "wc -l build/*.txt | tail -1"),
        ("Bash", "cat a?.txt"),
        ("PowerShell", "Get-ChildItem build -Filter '*.rs'"),
    ] {
        if !passes(tool, cmd, &[]) {
            bad.push(format!("{tool} {cmd:?}"));
        }
    }
    assert!(bad.is_empty(), "a read made only of allowed characters now asks: {bad:?}");
}

// Value: protects=rev 17-2 ④ — a glob may expand to a name that looks like an option; for the commands in the glob list that is
// still only a read, so a file named `--output=x` or `-delete` next to `ls *` changes nothing, and the list itself is pinned (a
// command with a writing or running option added to it fails here); fails_when=the list grows, or `ls *` asks; seam=run_hook + source text
#[test]
fn glob_list_has_only_commands_without_writing_options() {
    let src = fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/classify.rs")).unwrap();
    let line = src.lines().find(|l| l.starts_with("const GLOB_READS: &[&str] = &[")).expect("GLOB_READS line");
    let got: Vec<&str> = line.split('[').nth(2).unwrap().trim_end_matches("];").split(',').map(|s| s.trim().trim_matches('"')).collect();
    assert_eq!(
        got,
        ["ls", "cat", "head", "tail", "wc", "grep", "du", "get-childitem", "gci", "dir", "get-content", "gc", "select-string", "sls"],
        "glob list changed; every command added must have no option that writes, deletes or runs anything"
    );
    let p = project(&[]);
    for name in ["--output=x", "-delete", "--pre=sh"] {
        fs::write(p.join("build").join(name), "x").unwrap();
    }
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.join("build").to_string_lossy(), "hook_event_name": "PreToolUse",
                   "tool_name": "Bash", "tool_input": {"command": "ls *"}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_g"});
    assert_eq!(run_hook(v.to_string().as_bytes(), Some(&p)).code, 0, "ls * is a read whatever names the glob finds");
}

