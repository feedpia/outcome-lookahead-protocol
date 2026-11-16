//! 0.1.3 rc4, coordinator decision after second review RECHECK c501bf669 (third hole-by-hole round, principle 7): a remembered answer
//! and the built-in read pass are used only for a simple form — a program name and its arguments. An assignment anywhere, a wrapper
//! (env, exec, sudo, command, builtin, nice, time, nohup, xargs, …), shell grammar (`!` `if` `while` `for` `case` `{` `(` `;` `&&` `||` `|`
//! newline) or a builtin that changes variables (printf -v, read, declare, typeset, export, local, set, source, `.`) means it asks.
//! The table is generated: shapes × three positions (in front of the program, after a leading grammar word, in an earlier piece)
//! × three judging paths (Bash tool, PowerShell tool, Bash inside `$( )`). All file operations happen only in throwaway folders
//! under CARGO_TARGET_TMPDIR.
use olp_hook::run_hook;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn project(stored: &[&str]) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_simple_form_only").join(format!("{}_{n}", std::process::id()));
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
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_s"});
    run_hook(v.to_string().as_bytes(), Some(&p)).code == 0
}

/// Read programs the built-in rules pass today, with the answers a person might have stored for them.
const READS: [&str; 3] = ["git status", "ls build", "mytool run"];
const STORED: [&str; 4] = ["git", "git status", "ls", "mytool run"];

const ASSIGN: [&str; 3] = ["PATH=x/bin", "GIT_EXTERNAL_DIFF=./w.sh", "A=1"];
const WRAPPERS: [&str; 15] = [
    "env", "env -u ls", "env -i", "exec", "exec -a ls", "sudo", "sudo -u ls", "command", "builtin", "nice -n 1", "time", "nohup", "xargs", "timeout 5",
    "stdbuf -o0",
];
const VAR_BUILTINS: [&str; 10] = [
    "printf -v PATH x/bin", "read PATH < p.txt", "declare PATH=x/bin", "typeset PATH=x/bin", "export PATH=x/bin", "local PATH=x/bin", "set -- x",
    "source ./e.sh", ". ./e.sh", "readarray -t PATH < p.txt",
];
/// Leading grammar words and the text that closes them, `X` = the piece.
const LEADS: [&str; 6] = ["! X", "if X; then true; fi", "{ X; }", "while X; do break; done", "until X; do break; done", "(X)"];
// `X; Get-ChildItem` was here: a plain join of reads (`git status; Get-ChildItem`) passes under path B since rev 15, so it does
// not belong in a must-ask table (remembered answers for joined commands are pinned in bundle_no_memory.rs).
const PS_SHAPES: [&str; 13] = [
    "& X", ". X", "Start-Process X", "iex 'X'", "Invoke-Expression 'X'", "if ($true) { X }", "try { X } finally { }", "$env:PATH='x'; X", "$x = 1; X",
    "Set-Variable PATH x; X", "sv a 1; X", "New-Variable a 1; X", "X | Out-Null",
];

fn bash_shapes() -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    for r in READS {
        // position 1: in front of the program
        for p in ASSIGN.iter().chain(WRAPPERS.iter()) {
            v.push(("front", format!("{p} {r}")));
        }
        // position 2: after a leading grammar word (also grammar alone)
        for l in LEADS {
            v.push(("after-lead", l.replace('X', r)));
            for p in ASSIGN.iter().chain(WRAPPERS.iter()) {
                v.push(("after-lead", l.replace('X', &format!("{p} {r}"))));
            }
        }
        // position 3: an earlier piece changes variables (or is an assignment alone)
        for b in VAR_BUILTINS.iter().chain(ASSIGN.iter()) {
            for j in ["; ", " && ", "\n"] {
                v.push(("earlier-piece", format!("{b}{j}{r}")));
            }
        }
    }
    v
}

// Value: protects=coordinator decision «simple form only» (second review RECHECK c501bf669 blocking 1-3: `! X=… git`, `if PATH=… git`,
// `env -u ls PATH=x git`, `exec -a ls rm -rf build`, `printf -v PATH …; git`, `read PATH < f; git`) — every shape × position × path asks,
// with or without a stored answer; fails_when=one shape goes through without asking; seam=run_hook (the real hook verdict)
#[test]
fn only_a_simple_form_passes_without_asking() {
    let mut bad = Vec::new();
    let mut n = 0;
    let shapes = bash_shapes();
    for stored in [&[][..], &STORED[..]] {
        for (pos, cmd) in &shapes {
            n += 2;
            if passes("Bash", cmd, stored) {
                bad.push(format!("Bash [{pos}] {cmd:?} (stored {stored:?})"));
            }
            // third path: the same command inside `$( )` (the inner command is judged on its own)
            let sub = format!("echo $({cmd})");
            if passes("Bash", &sub, stored) {
                bad.push(format!("Bash $( ) [{pos}] {sub:?} (stored {stored:?})"));
            }
        }
        for r in READS {
            for s in PS_SHAPES {
                n += 1;
                let cmd = s.replace('X', r);
                if passes("PowerShell", &cmd, stored) {
                    bad.push(format!("PowerShell {cmd:?} (stored {stored:?})"));
                }
            }
        }
    }
    println!("simple-form table: {n} shapes");
    assert!(bad.is_empty(), "{} of {n} shapes went through without asking:\n{}", bad.len(), bad.join("\n"));
}

// Value: protects=coordinator condition on path B — a split the parser cannot be sure of asks: a backslash before a separator, a
// heredoc, curly quotes, and parentheses, braces, backticks or `$( )` anywhere (and `echo` is never a read since rev 17);
// fails_when=one of these passes without asking; seam=run_hook
// rev 17-2: a separator inside straight quotes no longer makes the split unsure (quoted text cannot hide a separator or an
// expansion), so `grep 'a|b' build`, `grep "a|b" build`, `grep 'a;b' build`, `grep "a|b" build && ls build`, `ls build | grep "a"`
// and the two PowerShell quoted reads moved to quoted_reads_pass_again.
#[test]
fn an_unsure_split_asks() {
    let mut bad = Vec::new();
    for cmd in [
        "echo \"x; y\"", "echo 'x && y'", "ls build; echo 'x'", "ls build\\; ls build", "ls build && cat <<EOF\nx\nEOF", "echo `ls build`",
        "echo $(ls build)", "(ls build)", "{ ls build; }", "ls build && (git status)", "grep \u{201c}a|b\u{201d} build", "echo 'a' && ls build && echo \"b\"",
    ] {
        for stored in [&[][..], &STORED[..]] {
            if passes("Bash", cmd, stored) {
                bad.push(format!("Bash {cmd:?} (stored {stored:?})"));
            }
        }
    }
    // `Get-ChildItem build; Write-Output 'a;b'` and `Get-ChildItem build | Select-String "a|b"`: moved to quoted_reads_pass_again (rev 17-2)
    let cmd = "Write-Output (Get-ChildItem build)";
    if passes("PowerShell", cmd, &[]) {
        bad.push(format!("PowerShell {cmd:?}"));
    }
    assert!(bad.is_empty(), "went through without asking: {bad:?}");
}

// Value: protects=rc4 rev 17-2: quoted text cannot hide a separator or an expansion — reads with a separator inside '...' or
// "..." (no `$` backtick backslash `!` inside), and sed/awk programs in single quotes that their own read rules accept
// (sed_print_only, awk_no_output), pass again; these used to assert «asks» (an_unsure_split_asks, reads_that_now_ask);
// fails_when=one of them asks again; seam=run_hook
#[test]
fn quoted_reads_pass_again() {
    let mut bad = Vec::new();
    for (tool, cmd) in [
        // from an_unsure_split_asks
        ("Bash", "grep 'a|b' build"),
        ("Bash", "grep \"a|b\" build"),
        ("Bash", "grep 'a;b' build"),
        ("Bash", "grep \"a|b\" build && ls build"),
        ("Bash", "ls build | grep \"a\""),
        ("PowerShell", "Get-ChildItem build; Write-Output 'a;b'"),
        ("PowerShell", "Get-ChildItem build | Select-String \"a|b\""),
        // from reads_that_now_ask MOVED_BASH (explore_reads R3B_READS and R012_READS)
        ("Bash", "sed -n '1,5p;8p' build/notes.txt"),
        ("Bash", "sed -n '1,30p;30q' build/notes.txt | cut -c1-200"),
        ("Bash", "awk '{print $1}' build/app.log"),
        ("Bash", "awk -F: '{print $1}' build/app.log"),
        ("Bash", "ls build | awk '{print $NF}'"),
        ("Bash", "awk 'END{print NR}' build/app.log build/notes.txt"),
    ] {
        if !passes(tool, cmd, &[]) {
            bad.push(format!("{tool} {cmd:?}"));
        }
    }
    assert!(bad.is_empty(), "a quoted read now asks: {bad:?}");
}

// Value: protects=path B keeps reads that are joined plainly (the ask-fatigue reason for B: about 216 new asks a day instead of 998);
// fails_when=a plain join of simple reads starts asking; seam=run_hook
#[test]
fn simple_pieces_joined_plainly_still_read() {
    let mut bad = Vec::new();
    for cmd in [
        "cd build && ls", "git status && git diff", "ls build | head -5", "git log -1; git status", "ls build || true", "cat a.txt 2>/dev/null | wc -l",
        // a quote whose span holds no separator does not make the split unsure
        "grep -n \"x\" a.txt | head -5", "git log -1 --format='%h' && git status",
    ] {
        if !passes("Bash", cmd, &[]) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a plain join of simple reads now asks: {bad:?}");
}

// Value: protects=the wrapper value hole that predates this PR (second review RECHECK c501bf669, «exec -a ls rm -rf build» passed as a read
// with no delete card); fails_when=a wrapper's flag value is taken as the program; seam=run_hook
#[test]
fn a_wrapper_flag_value_is_never_the_program() {
    let mut bad = Vec::new();
    for cmd in ["exec -a ls rm -rf build", "env -u ls rm -rf build", "sudo -u ls rm -rf build", "env -u ls PATH=x/bin git status"] {
        if passes("Bash", cmd, &[]) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "went through without asking: {bad:?}");
}

/// Bash reads that older tests asserted pass without asking; under rc4 rev 15 («simple form only», path B) they are not a simple form
/// (rev 17-2 groups: outside the read allow list, or test, [ or echo).
const MOVED_BASH: &[&str] = &[
    // from explore_reads R1_READS
    r#"for d in build; do du -sh "$d"; done"#,
    "if [ -d build ]; then ls build; fi",
    r#"while read f; do cat "$f"; done < list.txt"#,
    "{ ls; git status --short; }",
    // from explore_reads R2_READS (test r2_read_substitutions_show_no_card)
    r#"ls -la && git status --short | head; find . -maxdepth 3 -type d -name build -not -path "*/node_modules/*" 2>/dev/null; for d in $(find . -maxdepth 3 -type d -name build -not -path "*/node_modules/*"); do du -sh "$d"; git ls-files "$d" | head -3; git check-ignore "$d"; done"#,
    "du -sh $(git rev-parse --show-toplevel)",
    r#"echo "$(git status --short)""#,
    r#"for f in $(find build -type f); do wc -c "$f"; done"#,
    r#"echo "$(git log -1 --format="%h %s")""#,
    "cd $(echo /tmp) && ls",
    // from explore_reads R3_READS
    "echo $(sha256sum build/app.log | cut -c1-16)",
    // from explore_reads R3B_READS: `sed -n '1,5p;8p' build/notes.txt` and `sed -n '1,30p;30q' build/notes.txt | cut -c1-200` moved to
    // quoted_reads_pass_again (rev 17-2: quoted text cannot hide a separator)
    // from explore_reads R012_READS (rev 17-2: still ask, `NR==2`, `n=2` and `a=1` are read as NAME=value words after quote removal);
    // `awk '{print $1}' …`, `awk -F: '{print $1}' …`, `ls build | awk '{print $NF}'` and `awk 'END{print NR}' …` moved to quoted_reads_pass_again
    "awk -F ':' 'NR==2' build/app.log",
    "awk -v n=2 'NR==n' build/app.log",
    "awk -F: -v a=1 '{print $1}' build/app.log",
    // from explore_reads m1_bare_names_still_pass
    "sudo ls",
    // from falseask1 sleep_is_a_read_and_keeps_loops_readable
    "until grep -q DONE out.log; do sleep 20; done; tail -n 5 out.log",
    // from explore_reads R1_READS (rev 17-2: test, [ and echo are never a read)
    "test -d build && ls build",
    // from explore_reads R3_READS (rev 17-2: echo is never a read; `<` input is outside the allow list)
    "cmp -s build/app.log build/notes.txt && echo same",
    "echo ab | tr a-z A-Z",
    "tr -d x < build/app.log",
];

/// PowerShell reads that older tests asserted pass without asking (some only with the parser); under rc4 rev 15 they are not a simple form
/// (rev 17-2 groups: outside the read allow list, or echo).
const MOVED_PS: &[&str] = &[
    // from explore_reads READS
    r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Select-Object FullName; git status --short | Select-Object -First 20; if (Test-Path build) { (Get-ChildItem build -Recurse -File | Measure-Object -Property Length -Sum) | Select-Object Count, Sum; git ls-files build | Select-Object -First 5; git check-ignore build }"#,
    r#"Get-ChildItem -Directory -Force | Select-Object Name; git status --short | Select-Object -First 20; if (Test-Path build) { (Get-ChildItem build -Recurse -Force | Measure-Object -Property Length -Sum) | Select-Object Count, Sum; git ls-files build | Select-Object -First 5; git check-ignore build }"#,
    r#"Get-ChildItem -Directory -Force | Select-Object Name; if (Test-Path build) { "--- build ---"; Get-ChildItem build -Force | Select-Object -First 20 Name; (Get-ChildItem build -Recurse -File -Force | Measure-Object -Property Length -Sum) | Select-Object Count,Sum; git status --short --ignored build | Select-Object -First 5; git ls-files build | Select-Object -First 5 }"#,
    // from explore_reads PARSER_READS (test block_reads_need_the_parser_and_stay_fast)
    r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName; git status --short | Select-Object -First 20; if (Test-Path build) { (Get-ChildItem build -Recurse -File | Measure-Object -Sum Length) | Select-Object Count,Sum; git ls-files build | Select-Object -First 5 }"#,
    r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName, LastWriteTime; git status --short | Select-Object -First 20; git check-ignore build"#,
    r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum); "{0}  files={1}  MB={2:N1}" -f $_.FullName, $s.Count, ($s.Sum/1MB) }; git status --short -- build 2>$null | Select-Object -First 5; git ls-files build | Measure-Object | Select-Object -ExpandProperty Count"#,
    r#"Get-ChildItem -Directory -Recurse -Depth 3 -Filter build -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse -File -ErrorAction SilentlyContinue | Measure-Object Length -Sum); "{0} | files={1} | MB={2:N1}" -f $_.FullName, $s.Count, ($s.Sum/1MB) }; git status --short | Select-Object -First 10; git check-ignore build"#,
    // from ps_block c3_read_only_blocks_show_no_card
    "if (Test-Path build) { Get-ChildItem build -Recurse | Measure-Object }",
    "if (Test-Path build) { Get-ChildItem build } else { Test-Path .\\other }",
    "try { Get-ChildItem build } catch { Get-Date }",
    // from ps_block2 READS (test b3_interpreted_read_blocks_show_no_card)
    r#"Get-ChildItem build | Where-Object { $_.FullName -notmatch 'node_modules' } | Select-Object FullName"#,
    r#"Get-ChildItem build | ForEach-Object { $s = (Get-ChildItem $_.FullName -Recurse | Measure-Object Length -Sum); "{0} files={1}" -f $_.FullName, $s.Count }"#,
    r#"Get-ChildItem build | ForEach-Object { $_.Name.ToLower() }"#,
    r#"if (Test-Path build) { "--- build ---"; git ls-files build | ForEach-Object { $_ } }"#,
    // from ps_block2 p2_korean_and_spaces_in_command_text
    r#"Get-ChildItem build | Where-Object { $_.Name -ne '한글 파일.txt' } | Select-Object Name"#,
    // from ps_block2b passes_with_parser (f2, f3, f7)
    r#"Get-ChildItem build | ForEach-Object { [int]$_.Length }"#,
    r#"Get-ChildItem build | ForEach-Object { '1' -as [int] }"#,
    r#"Get-ChildItem build | ForEach-Object -Process:{ $_.Name }"#,
    r#"Get-ChildItem build | Where-Object -FilterScript:{ $_.Length -gt 0 }"#,
    r#"Get-ChildItem build | ForEach-Object { [IO.Path]::GetFileName($_.FullName) }"#,
    // from ps_parser_paths s1_block_command_is_a_parsed_read_or_written_off_without_junk and value_only_blocks_are_parsed_reads_or_written_off_without_junk
    r#"Get-ChildItem -Directory -Recurse -Depth 2 -Filter build | Where-Object { $_.FullName -notmatch 'node_modules' } | ForEach-Object { $s=(Get-ChildItem $_.FullName -Recurse -File | Measure-Object Length -Sum); "{0} files={1} MB={2:N1}" -f $_.FullName,$s.Count,($s.Sum/1MB) }; git status --short | Select-Object -First 10; git check-ignore build"#,
    "ForEach-Object { 1 }",
    // from ps_strlit t1_lone_string_statements_are_output_only
    r#"Get-ChildItem -Directory -Force | Select-Object Name; if (Test-Path build) { "--- build ---"; Get-ChildItem build -Force | Select-Object -First 20 Name; (Get-ChildItem build -Recurse -File -Force | Measure-Object -Property Length -Sum) | Select-Object Count,Sum; git status --short --ignored build | Select-Object -First 5; git ls-files build | Select-Object -First 5 }"#,
    // from explore_reads READS (rev 17-2: a backslash path is outside the allow list)
    r#"Get-ChildItem build -Recurse -Force | Select-Object FullName,Length,LastWriteTime; Get-Content build\notes.txt -TotalCount 30; git log --oneline -3 -- build"#,
    r#"Get-ChildItem build -Recurse -File | Select-Object FullName, Length, LastWriteTime; git ls-files build; "---notes.txt---"; Get-Content build\notes.txt -TotalCount 20"#,
    r#"Get-ChildItem build -Recurse -Force | Select-Object FullName, Length, LastWriteTime; Write-Output "--- notes.txt ---"; Get-Content build\notes.txt -TotalCount 20"#,
    r#"Get-ChildItem build -Recurse -Force | Select-Object FullName, Length, LastWriteTime; Get-Content build\notes.txt -TotalCount 30"#,
    r#"Get-ChildItem -Recurse -Force build | Select-Object FullName, Length, LastWriteTime; Get-Content build\notes.txt"#,
    // from explore_reads m1_bare_names_still_pass (rev 17-2: echo is never a read)
    "echo hi",
    // from ps_strlit t1_lone_string_statements_are_output_only (rev 17-2: a backslash path is outside the allow list)
    r#"Get-ChildItem build; "---notes.txt---"; Get-Content build\a.txt"#,
    // from ps_strlit c9_redirect_to_dollar_null_is_a_null_sink (rev 17-2: `$` outside quotes and `>` other than the four discard
    // words are outside the allow list)
    "git status --short -- build 2>$null",
    "Get-ChildItem build 2>$NULL | Select-Object Name",
    "Get-ChildItem build >$Null",
];

// Value: protects=rc4 rev 15 (coordinator decision «simple form only», path B) — reads that older tests asserted pass (grammar words, braces,
// `$( )`, a separator inside quotes, wrappers, `NAME=value` words, PowerShell blocks) now ask instead of being dropped silently from those
// tables; fails_when=one of them goes through without asking again (with or without the PowerShell parser); seam=run_hook
#[test]
fn reads_that_now_ask() {
    let mut bad = Vec::new();
    for (tool, list) in [("Bash", MOVED_BASH), ("PowerShell", MOVED_PS)] {
        for cmd in list {
            if passes(tool, cmd, &[]) {
                bad.push(format!("{tool} {cmd:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{} moved reads went through without asking:\n{}", bad.len(), bad.join("\n"));
}

// Value: protects=the simple form keeps working (no new asks for a plain read or a stored answer); fails_when=the new rule also stops
// a program name with arguments; seam=run_hook
#[test]
fn a_simple_form_still_passes() {
    let mut bad = Vec::new();
    for (tool, cmd, stored) in [
        ("Bash", "git status", &[][..]),
        ("Bash", "ls build", &[][..]),
        ("Bash", "git log -1 --format=x", &[][..]),
        ("Bash", "mytool run", &["mytool run"][..]),
        ("Bash", "ls build 2>/dev/null", &[][..]),
        ("PowerShell", "Get-ChildItem build", &[][..]),
    ] {
        if !passes(tool, cmd, stored) {
            bad.push(format!("{tool} {cmd:?} (stored {stored:?})"));
        }
    }
    assert!(bad.is_empty(), "a simple form now asks: {bad:?}");
}

// Value: protects=second review RECHECK rev 15 blocking 1 — an arithmetic assignment changes a variable without an assignment word
// (`test -v 'x[PATH=5]'` evaluates the array subscript, `$[PATH=5]` is old arithmetic), so a later piece may run another program;
// any `[` in the raw text means not a simple form and it asks (cost: `[ -d build ]` asks too); fails_when=one shape passes without
// asking; seam=run_hook
#[test]
fn arithmetic_assignment_asks() {
    let mut bad = Vec::new();
    let heads = ["test -v 'x[PATH=5]'", "test -v x[PATH=5]", "[ -v 'x[PATH=5]' ]", "echo $[PATH=5]", "echo x[PATH=5]", "[ -d build ]"];
    for h in heads {
        for j in ["; ", " || ", " && ", " | ", "\n"] {
            let cmd = format!("{h}{j}git status");
            for stored in [&[][..], &STORED[..]] {
                if passes("Bash", &cmd, stored) {
                    bad.push(format!("{cmd:?} (stored {stored:?})"));
                }
            }
        }
        if passes("Bash", h, &[]) {
            bad.push(format!("{h:?} alone"));
        }
    }
    assert!(bad.is_empty(), "went through without asking: {bad:?}");
}

