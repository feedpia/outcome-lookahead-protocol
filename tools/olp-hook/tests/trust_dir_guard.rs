//! 0.1.3 rc4 rev 17-3 (B review of rel 49d7a9281, blocking 1): the user trust folder `<home>/olp/projects/<key>/` holds the files
//! that decide what passes without asking, so writing, copying over or deleting them is OLP's own state (self-protection), the same
//! as `.olp/` inside a project — whether the path is written absolutely, with the home variables or relative to a folder inside it.
//! Nothing here touches the real trust folder: the hook only judges the text.
use olp_hook::classify::{self, Limits};
use olp_hook::store::{self, DeviceMap};
use serde_json::json;
use std::path::PathBuf;

fn cwd() -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_trust_dir_guard");
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn guarded(tool: &str, input: serde_json::Value, at: &std::path::Path) -> bool {
    classify::assess(tool, &input, &DeviceMap::default(), at, Limits { counter: None, ps_parser: false }).self_guard
}

// Value: protects=a remembered answer cannot be planted by editing the trust folder with a plain overwrite card (B blocking 1);
// fails_when=one way of writing or deleting a file under <home>/olp/ is not self-protection; seam=classify::assess
#[test]
fn writing_the_trust_folder_is_self_protection() {
    let p = cwd();
    let t = store::trust_dir(&p);
    let map = t.join("hook-map.json");
    let cfg = t.join("hook-config.json");
    let (m, c) = (map.to_string_lossy().to_string(), cfg.to_string_lossy().to_string());
    let key = t.file_name().unwrap().to_string_lossy().to_string();
    let mut bad = Vec::new();
    for (tool, input) in [
        ("Write", json!({"file_path": m, "content": "{}"})),
        ("Edit", json!({"file_path": c, "old_string": "{", "new_string": "{ "})),
        ("Bash", json!({"command": format!("echo x > {c}")})),
        ("Bash", json!({"command": format!("cp a.json {m}")})),
        ("Bash", json!({"command": format!("rm {m}")})),
        ("Bash", json!({"command": format!("echo x > $HOME/olp/projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("echo x > ${{HOME}}/olp/projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("echo x > ~/olp/projects/{key}/hook-map.json")})),
        // (`%USERPROFILE%` is not expanded by Bash and `\` is an escape there, so that spelling writes a local file: not listed)
        ("PowerShell", json!({"command": format!("Set-Content $env:USERPROFILE\\olp\\projects\\{key}\\hook-map.json x")})),
        ("Bash", json!({"command": format!("cd {} && echo x > hook-map.json", t.to_string_lossy())})),
    ] {
        if !guarded(tool, input.clone(), &p) {
            bad.push(format!("{tool} {input}"));
        }
    }
    if !guarded("Write", json!({"file_path": "hook-map.json", "content": "{}"}), &t) {
        bad.push("Write hook-map.json with the working folder inside the trust folder".to_string());
    }
    assert!(bad.is_empty(), "not self-protection:\n{}", bad.join("\n"));
}

// Value: protects=no new asks for ordinary files whose names only look alike; fails_when=a path that merely contains `olp` in another
// word (olpx, myolp) becomes self-protection; seam=classify::assess
#[test]
fn similar_names_are_not_self_protection() {
    let p = cwd();
    for f in ["olpx/hook-map.json", "myolp/a.txt", "build/olp.txt"] {
        assert!(!guarded("Write", json!({"file_path": f, "content": "x"}), &p), "{f}");
    }
}

// Value: protects=B RECHECK of rel 78295393f — the target is made absolute against the working folder before the one comparison,
// so `../olp`, `cd ~/olp` then a relative write, `cd ..` then `olp/...` and `$USERPROFILE` cannot slip past self-protection;
// fails_when=one of these writes is not self-protection; seam=classify::assess
#[test]
fn relative_and_variable_targets_are_resolved_first() {
    let Some(home) = store::home() else { return };
    let cwd = home.join("olp_guard_probe_project");
    let key = "0123456789abcdef0123456789abcdef";
    let mut bad = Vec::new();
    for (tool, input) in [
        ("Write", json!({"file_path": format!("../olp/projects/{key}/hook-map.json"), "content": "{}"})),
        ("Bash", json!({"command": format!("echo x > ../olp/projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("cp a.json ../olp/projects/{key}/hook-config.json")})),
        ("Bash", json!({"command": format!("cd ~/olp && echo x > projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("cd ~/olp/projects/{key} && cp a.json hook-map.json")})),
        ("Bash", json!({"command": format!("cd .. && echo x > olp/projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("echo x > $USERPROFILE/olp/projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("cp a.json ${{USERPROFILE}}/olp/projects/{key}/hook-map.json")})),
        ("Bash", json!({"command": format!("echo x > ./../olp/./projects/{key}/hook-map.json")})),
    ] {
        if !guarded(tool, input.clone(), &cwd) {
            bad.push(format!("{tool} {input}"));
        }
    }
    assert!(bad.is_empty(), "not self-protection:\n{}", bad.join("\n"));
    // the same shapes aimed elsewhere stay ordinary writes
    for f in ["../other/hook-map.json", "olpx/hook-map.json"] {
        assert!(!guarded("Write", json!({"file_path": f, "content": "{}"}), &cwd), "{f}");
    }
}

// Value: protects=second review of rel b59d967e2 — a target that cannot be resolved for sure (glob, `~user`, a word built at run
// time, a link) never reaches the trust folder without self-protection; the trust folder itself is under `<home>/.olp/` so every
// literal mention hits the `.olp` check; fails_when=one of these writes is not self-protection; seam=classify::assess
#[test]
fn unsure_targets_never_reach_the_trust_folder() {
    let p = cwd();
    let t = store::trust_dir(&p);
    assert!(t.to_string_lossy().replace('\\', "/").contains("/.olp/projects/"), "the trust folder lives under <home>/.olp/");
    let mut bad = Vec::new();
    for (tool, cmd) in [
        ("Bash", "tee ~/o*/projects/*/hook-map.json"),
        ("Bash", "tee ~/.o*/projects/*/hook-map.json"),
        ("Bash", "echo x | tee ~/.ol?/projects/*/hook-config.json"),
        ("Bash", "echo x > ~User/olp/projects/k/hook-map.json"),
        ("Bash", "cp a.json ~User/.olp/projects/k/hook-map.json"),
        ("PowerShell", r"Set-Content (Join-Path $env:USERPROFILE 'olp\..') x"),
        ("PowerShell", r"Set-Content (Join-Path $env:USERPROFILE '.olp\projects\k\hook-map.json') x"),
    ] {
        if !guarded(tool, json!({ "command": cmd }), &p) {
            bad.push(format!("{tool} {cmd}"));
        }
    }
    assert!(bad.is_empty(), "not self-protection:\n{}", bad.join("\n"));
}

// Value: protects=a cloned repository cannot reach the trust folder through a link it carries (`cache -> <home>/.olp`): the link
// is resolved on disk before the comparison; fails_when=a write through the link is not self-protection; seam=classify::assess
#[cfg(unix)]
#[test]
fn a_link_into_the_trust_folder_is_resolved() {
    let Some(home) = store::home() else { return };
    let repo = cwd().join("linked_repo");
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(home.join(".olp")).unwrap();
    std::os::unix::fs::symlink(home.join(".olp"), repo.join("cache")).unwrap();
    std::os::unix::fs::symlink(repo.join("missing_target"), repo.join("broken")).unwrap();
    let mut bad = Vec::new();
    for (tool, input) in [
        ("Write", json!({"file_path": "cache/projects/k/hook-map.json", "content": "{}"})),
        ("Bash", json!({"command": "echo x > cache/projects/k/hook-map.json"})),
        ("Bash", json!({"command": "cp a.json broken/hook-map.json"})),
    ] {
        if !guarded(tool, input.clone(), &repo) {
            bad.push(format!("{tool} {input}"));
        }
    }
    assert!(bad.is_empty(), "a link into the trust folder was not resolved:\n{}", bad.join("\n"));
}

// Value: protects=second review RECHECK 1 of rel dd79e582c, blocking 2 — a redirect whose target is a glob (`>`, `>>`) takes the same
// path as `tee` and `cp`: the glob is matched against the trust roots; fails_when=a globbed redirect into the trust folder is not
// self-protection, or a globbed redirect elsewhere becomes one; seam=classify::assess
#[test]
fn a_globbed_redirect_is_matched_like_tee() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "echo x > ~/.o*/projects/*/hook-map.json",
        "echo x >> ~/.o*/projects/*/hook-config.json",
        "echo x > ~/.ol?/projects/*/hook-map.json",
        "echo x 2> ~/o*/projects/k/hook-map.json",
        "ls build && echo x >~/.o*/projects/*/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a globbed redirect into the trust folder is not self-protection: {bad:?}");
    for cmd in ["echo x > build/*.log", "echo x >> /tmp/o*/a.txt"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}


// Value: protects=second review RECHECK 2 of rel c9d34233a, blocking 1 — a globbed trust-folder target written in a command that
// also has a heredoc, `$( )` or `>( )` (the parser marks it as building itself) and a `~user` home are judged by the same
// self-protection check as `echo x > …`; fails_when=one of the four shapes is not self-protection; seam=classify::assess
#[test]
fn globbed_targets_in_built_commands_are_self_protection() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "cat > ~/.o*/projects/*/hook-map.json <<'EOF'\n{\"version\":1,\"entries\":{\"bash:make\":{\"effect\":\"read_only\",\"reversible\":false,\"recovery\":\"unknown\",\"declared_at\":1}}}\nEOF",
        "echo \"$(cat a.json)\" > ~/.o*/projects/*/hook-map.json",
        "cat a.json | tee >(cat > ~/.o*/projects/*/hook-map.json)",
        "echo x > ~User/.o*/projects/*/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a globbed trust-folder target in a built command is not self-protection: {bad:?}");
    // the same shapes aimed elsewhere stay ordinary writes
    for cmd in [
        "cat > build/*.json <<'EOF'\n{}\nEOF",
        "echo \"$(cat a.json)\" > /tmp/o*/a.txt",
        "cat a.json | tee >(cat > build/out.json)",
        "echo x > ~User/build/*.log",
    ] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=second review RECHECK 3 of rel 61983f0d3, blocking 1 and 1-b — a command that builds itself is judged on the same
// real words (quotes removed, `$` kept) and the same working folder as a plain piece, a heredoc body is content (never a target), and
// `~user/..` is not trusted; fails_when=one of the blocked shapes is not self-protection, or a heredoc with a `/*` comment is;
// seam=classify::assess
#[test]
fn built_commands_use_the_same_words_and_folder() {
    let Some(home) = store::home() else { return };
    let proj = home.join("olp_guard_probe_project");
    let mut bad = Vec::new();
    for cmd in [
        "tee \"$HOME\"/.o*/projects/*/hook-map.json <<'EOF'\n{}\nEOF",
        "tee $HOME/.o*/projects/*/hook-map.json <<'EOF'\n{}\nEOF",
        "cat > ../.o*/projects/*/hook-map.json <<'EOF'\n{}\nEOF",
        "echo \"$(cat a.json)\" > ../.o*/projects/*/hook-map.json",
        "echo x > ~User/../User/.o*/projects/*/hook-map.json",
        "cd sub && cat > ../../.o*/projects/*/hook-map.json <<'EOF'\n{}\nEOF",
        "echo \"$(cat a.json > ~/.o*/projects/*/hook-map.json)\"",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &proj) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a built command reaching the trust folder is not self-protection: {bad:?}");
    let p = cwd();
    for cmd in ["cat > src/x.rs <<'EOF'\n/* note */\nEOF", "cat > a.css <<'EOF'\n/* x */\n/** y */\nEOF", "cat > build/*.json <<'EOF'\n{}\nEOF"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=B RECHECK 6 of rel 622fe5781 — a heredoc whose end word the check cannot read the way bash does (quotes with blanks,
// escapes, no end line) never hides the commands after it: only a plain end word lets the body be left out; fails_when=a write into the
// trust folder after such a heredoc is not self-protection; seam=classify::assess
#[test]
fn heredoc_end_words_never_hide_later_commands() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "cat <<\"E O F\"\nx\nE O F\necho x > ~/.o*/projects/*/hook-map.json",
        "cat <<'E O F'\nx\nE O F\necho x > ~/.o*/projects/*/hook-map.json",
        "cat <<E\\ O\\ F\nx\nE O F\necho x > ~/.o*/projects/*/hook-map.json",
        "cat <<$'EOF'\nx\nEOF\necho x > ~/.o*/projects/*/hook-map.json\n$EOF",
        "cat <<EOF\nx\necho x > ~/.o*/projects/*/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a command after a heredoc is hidden from self-protection: {bad:?}");
    for cmd in ["cat > src/x.rs <<'EOF'\n/* note */\nEOF", "cat > src/x.rs <<\"EOF\"\n/* note */\nEOF\necho done"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=second review RECHECK 4 of rel 622fe5781 — no `<<` the check reads differently from bash (comment, escape,
// arithmetic, $'..', escaped quote) and no command inside double quotes (backticks, `$( )` with a quoted `)`) hides a write into
// the trust folder; fails_when=one of the seven shapes is not self-protection; seam=classify::assess
#[test]
fn built_command_parsing_never_hides_targets() {
    let p = cwd();
    let w = "cat > ~/.o*/projects/*/hook-map.json <<'EOF'\n{}\nEOF";
    let mut bad = Vec::new();
    for cmd in [
        format!("true # <<:\n{w}\n:"),
        format!("echo \\<<X\n{w}\nX"),
        format!("echo $((1<<2))\n{w}\n2"),
        format!("cat <<$'A'\na\nA\n{w}\n$A"),
        format!("echo \"\\\" <<X\"\n{w}\nX"),
        "echo \"`cat a.json > ~/.o*/projects/*/hook-map.json`\"".to_string(),
        "echo \"$(echo ')'; cat a.json > ~/.o*/projects/*/hook-map.json)\"".to_string(),
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a write into the trust folder is hidden by the built-command reading: {bad:?}");
}

// Value: protects=second review RECHECK 5 of rel 80db1a4cc — a lone quote in a heredoc body or a comment (`don't`, `say "hi`), and a
// `#` comment or a `case` pattern `)` inside `$( )`, never make a later write into the trust folder look quoted; fails_when=one of the
// five shapes is not self-protection; seam=classify::assess
#[test]
fn stray_quotes_and_parens_never_hide_targets() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "cat > notes.md <<'EOF'\ndon't\nEOF\ncat a.json > ~/.o*/projects/*/hook-map.json",
        "cat > x.txt <<EOF\na\nEOF\n# don't\necho x > ~/.o*/projects/*/hook-map.json",
        "cat > x.txt <<'EOF'\nsay \"hi\nEOF\ncat a.json > ~/.o*/projects/*/hook-map.json",
        "echo \"$(true # )\ncat a.json > ~/.o*/projects/*/hook-map.json\n)\"",
        "echo \"$(case a in a) cat a.json > ~/.o*/projects/*/hook-map.json;; esac)\"",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a stray quote or paren hides a write into the trust folder: {bad:?}");
    for cmd in ["cat > notes.md <<'EOF'\ndon't /* x */\nEOF", "echo \"it's fine\" > build/out.txt"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=B RECHECK 5 condition 1 — PowerShell wildcards (and Bash with dotglob) let `?` and `*` match a leading dot, so
// `?ol?` names `.olp`; fails_when=one of these writes is not self-protection; seam=classify::assess
#[test]
fn a_leading_dot_is_matched_by_wildcards_too() {
    let p = cwd();
    let mut bad = Vec::new();
    for (tool, cmd) in [
        ("PowerShell", r"Set-Content -Path $HOME\?ol?\projects\*\hook-map.json -Value x"),
        ("Bash", "echo x > ~/?ol?/projects/*/hook-map.json"),
        ("Bash", "echo x > ~User/?ol?/projects/*/hook-map.json"),
        ("Bash", "cd sub && echo x > ../../?ol?/projects/*/hook-map.json"),
    ] {
        if !guarded(tool, json!({ "command": cmd }), &p) {
            bad.push(format!("{tool} {cmd}"));
        }
    }
    assert!(bad.is_empty(), "a wildcard over the leading dot is not self-protection: {bad:?}");
    for cmd in ["echo x > build/*.log", "cd sub && echo x > ../build/*.log"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=B RECHECK 8 of rel e430fd0d5 — in a plain several-line command a lone quote after `#` (a comment for bash) never makes
// the next line look quoted; fails_when=a write into the trust folder after `# don't` / `# it's` is not self-protection; seam=classify::assess
#[test]
fn a_quote_in_a_comment_never_hides_the_next_line() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in ["echo a # don't\necho x > ~/.o*/projects/*/hook-map.json", "ls # it's\ncp a.json ~/.o*/projects/*/hook-map.json"] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a quote in a comment hides the next line from self-protection: {bad:?}");
    for cmd in ["ls # it's fine", "echo x > build/out.txt # don't"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=second review RECHECK 6 of rel a9b000c94 — a part of a target the check does not expand (`$'..'`, `$".."`, a brace
// list `{a,b}`) is treated as `*`, so the shell's own spellings of `.olp` never slip past self-protection; fails_when=one of the six
// shapes is not self-protection, or a plain brace copy / `$'\t'` echo becomes one; seam=classify::assess
#[test]
fn unexpanded_parts_of_a_target_count_as_a_wildcard() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "echo x > ~/$''.o*/projects/*/hook-map.json",
        "cat > n.md <<'EOF'\na\nEOF\necho x > ~/$''.o*/projects/*/hook-map.json",
        "cat a.json > ~/$'.o'*/projects/*/hook-map.json",
        "cat a.json > ~/$\".o\"*/projects/*/hook-map.json",
        "cat a.json | tee ~/.o{,}*/projects/*/hook-map.json",
        "cat a.json | tee ~/{.o,zz}*/projects/*/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "an unexpanded part of a target slips past self-protection: {bad:?}");
    for cmd in ["cp a.txt build/{x,y}.txt", "echo $'a\\tb' > out.txt"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=second review RECHECK 7 (coordinator side) and B RECHECK 10 of rel 922c6bf14 — a variable that is not a home
// variable (`$X`, `${X}`, and `$'o'` read back as `$olp` once quotes are gone) counts as `*`, so a variable set earlier to `.olp`
// never slips a write into the trust folder past self-protection; fails_when=one of the eight shapes is not self-protection, or a
// plain `$NAME` copy into a build folder becomes one; seam=classify::assess
#[test]
fn other_variables_in_a_target_count_as_a_wildcard() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "echo x > ~/.$'o'lp/projects/*/hook-map.json",
        "cat a.json | tee ~/.o$'lp'/projects/*/hook-map.json",
        "echo x > ~/.ol$\"p\"/projects/*/hook-map.json",
        "cat > n.md <<'EOF'\na\nEOF\necho x > ~/.$'o'lp/projects/*/hook-map.json",
        "echo x > ~/$X/projects/k/hook-map.json",
        "echo x > ~/${X}/projects/k/hook-map.json",
        "echo x > $HOME/$X/projects/k/hook-map.json",
        "cp a.json ~/$D/projects/*/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a variable in a target slips past self-protection: {bad:?}");
    for cmd in ["cp a.txt dist/$NAME.txt", "echo x > build/${NAME}.log"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=second review RECHECK 8 (coordinator side) of rel 71131c8ee — a target that starts with a variable that is not a
// home variable can be anywhere, so its trust-file name (hook-map.json, hook-config.json) alone makes it OLP's state, and a home
// variable with an operator (`${HOME%/}`) is still the home; fails_when=one of the five shapes is not self-protection, or a plain
// `$OUT/notes.txt` write becomes one; seam=classify::assess
#[test]
fn a_target_led_by_an_unknown_variable_is_judged_by_its_trust_file_name() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in [
        "X=~/.$'o'lp/projects/k; echo x > $X/hook-map.json",
        "X=~/.$'o'lp/projects/k; cp a.json $X/hook-config.json",
        "X=~/.$'o'lp/projects/k; echo x > \"$X\"/hook-map.json",
        "X=~/.$'o'lp/projects/k; echo x > ${X}/hook-map.json",
        "echo x > ${HOME%/}/.$'o'lp/projects/k/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a target led by an unknown variable slips past self-protection: {bad:?}");
    for cmd in ["echo x > $OUT/notes.txt", "cp a.json $DIST/app.json"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=B RECHECK 12 of rel c173f6176 — only `${HOME%/}` and the default forms (`${HOME:-x}` …) are the home; any other
// operation (`${HOME%/*}` = the home's parent, `${HOME#…}`, `${HOME/a/b}` …) can point anywhere and counts as `*`; fails_when=a write
// through the home's parent into the trust folder is not self-protection, or `${HOME%/}/notes.txt` becomes one; seam=classify::assess
#[test]
fn only_plain_home_forms_are_the_home() {
    let p = cwd();
    let mut bad = Vec::new();
    for cmd in ["echo x > ${HOME%/*}/ci/.o*/projects/*/hook-map.json", "echo x > ${HOME%/*}/ci/.o*/projects/k/hook-config.json"] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "a home operation slips past self-protection: {bad:?}");
    for cmd in ["echo x > ${HOME%/}/notes.txt", "echo x > ${HOME:-/tmp}/notes.txt"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
}

// Value: protects=second review RECHECK 9 (coordinator side) of rel 2f6d4fcf4 — the fixed probe set of that review (two input files, 41
// cases without the 3 notes): 31 writes that reach the trust folder through a value, a `cd` argument, a link source or joined strings are
// self-protection, 10 ordinary writes are not; the next version is judged on the same set; fails_when=one case differs; seam=classify::assess
#[test]
fn second_review_probe_set_rev5() {
    let p = cwd();
    let cases: &[(&str, bool, &str, &str)] = &[
        ("R8-1", true, "Bash", r#"X=~/.$'o'lp/projects/k; echo x > $X/hook-map.json"#),
        ("R8-2", true, "Bash", r#"X=~/.$'o'lp/projects/k; cp a.json $X/hook-map.json"#),
        ("R8-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; echo x > "$X"/hook-map.json"#),
        ("R8-4", true, "Bash", r#"X=~/.$'o'lp; echo x > ${X}/projects/k/hook-map.json"#),
        ("R8-5", true, "Bash", r#"echo x > ${HOME%/}/.$'o'lp/projects/k/hook-map.json"#),
        ("B12-1", true, "Bash", r#"echo x > ${HOME%/*}/h5/.$'o'lp/projects/k/hook-map.json"#),
        ("B12-2", true, "Bash", r#"echo x > ${HOME%/*}/h5/.o*/projects/*/hook-config.json"#),
        ("B12-3", true, "Bash", r#"echo x > ${HOME#x}/.$'o'lp/projects/k/hook-map.json"#),
        ("C2-1", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-map.json; echo x > $X"#),
        ("C2-2", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-map.json; cp a.json "$X""#),
        ("C2-3", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-map.json; cat a.json | tee $X"#),
        ("C2-4", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-config.json; echo x > ${X}"#),
        ("UP-1", true, "Bash", r#"X=~/.$'o'lp/projects/k; echo x > $X/HOOK-MAP.JSON"#),
        ("TP-1", true, "Bash", r#"cd ~ && echo x > ~+/.$'o'lp/projects/k/hook-map.json"#),
        ("TP-2", true, "Bash", r#"cd ~/.$'o'lp/projects/k && echo x > ~+/hook-map.json"#),
        ("TM-1", true, "Bash", r#"cd ~ && cd /tmp && echo x > ~-/.$'o'lp/projects/k/hook-map.json"#),
        ("LN-1", true, "Bash", r#"ln -s ~/.$'o'lp/projects/k/hook-map.json m; echo x > m"#),
        ("LN-2", true, "Bash", r#"X=~/.$'o'lp/projects/k; ln -s $X/hook-map.json m && echo x > m"#),
        ("LN-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; ln -s $X lnk && echo x > lnk/hook-map.json"#),
        ("EXE-1", true, "Bash", r#"X=~/$'o'lp; cp evil.exe $X/olp-hook.exe"#),
        ("EXE-2", true, "Bash", r#"X=~/$'o'lp; cp evil $X/olp-hook"#),
        ("PS-1", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json" -Value x"#),
        ("PS-DOT", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json." -Value x"#),
        ("PS-SP", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json " -Value x"#),
        ("PS-ADS", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json::`$DATA" -Value x"#),
        ("OK-1", false, "Bash", r#"echo x > $OUT/notes.txt"#),
        ("OK-2", false, "Bash", r#"cp a.json $DIST/app.json"#),
        ("OK-3", false, "Bash", r#"echo x > ${HOME%/}/notes.txt"#),
        ("OK-4", false, "Bash", r#"echo x > ${HOME:-/tmp}/notes.txt"#),
        ("OK-5", false, "Bash", r#"echo x > $TMPDIR/out.txt"#),
        ("OK-6", false, "Bash", r#"X=out/a.txt; echo x > $X"#),
        ("OK-7", false, "Bash", r#"echo x > build/out.txt"#),
        ("OK-8", false, "Bash", r#"echo x > $HOME/notes.txt"#),
        ("OK-9", false, "Bash", r#"echo x > $OUT/hook-map.json.bak"#),
        ("OK-10", false, "PowerShell", r#"Set-Content -Path "$env:TEMP\notes.txt" -Value x"#),
        ("CD-1", true, "Bash", r#"cd ~/.$'o'lp/projects/k && echo x > hook-map.json"#),
        ("CD-2", true, "Bash", r#"cd ~/.$'o'lp/projects/k && cp a.json hook-map.json"#),
        ("CD-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; cd $X && echo x > hook-map.json"#),
        ("LN-4", true, "Bash", r#"ln -s ~/.$'o'lp/projects/k lnk && echo x > lnk/hook-map.json"#),
        ("PS-DOT2", true, "PowerShell", r#"Set-Content -Path "$env:USERPROFILE\.olp\projects\k\hook-map.json." -Value x"#),
        ("PS-VAR", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k\hook-map.json"; Set-Content -Path $X -Value x"#),
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter(|(_, want, tool, cmd)| guarded(tool, json!({ "command": cmd }), &p) != *want)
        .map(|(id, want, _, _)| format!("{id} want self-protection={want}"))
        .collect();
    assert_eq!(cases.iter().filter(|c| c.1).count(), 31);
    assert!(bad.is_empty(), "probe set differs: {bad:?}");
}

// Value: protects=B RECHECK 14 condition 2 of rel d3aa80395 — the «whole command names .olp» rule is for writes only: reading the trust
// folder (`cat`, `ls`) is not self-protection; a write that names it anywhere still is; fails_when=a read of ~/.olp becomes self-protection
// or a write through a value stops being one; seam=classify::assess
#[test]
fn reading_the_trust_folder_is_not_self_protection() {
    let p = cwd();
    for cmd in ["cat ~/.olp/projects/k/hook-map.json", "ls ~/.olp", "ls -la ~/.olp/projects"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "{cmd}");
    }
    assert!(guarded("Bash", json!({ "command": "X=~/.$'o'lp/projects/k/hook-map.json; echo x > $X" }), &p));
}

// Value: protects=the design addendum of 11-15 (owner side) — one rule: the trust folder's name (`.olp`, or a
// glob that can name it such as `.o*` / `.ol?`) seen anywhere in the flattened command is self-protection unless the command is proven to
// be reads only (a closed read list, no redirect, assignment, cd, ln, cp, mv, tee or subshell); fails_when=a read-only command is caught,
// an over-capture the design accepts is not, or a program not on the read list slips past; seam=classify::assess
#[test]
fn the_trust_folder_name_is_self_protection_unless_reads_only() {
    let p = cwd();
    for cmd in [
        "cat ~/.olp/projects/k/hook-map.json",
        "ls ~/.olp",
        "head -n 3 ~/.olp/projects/k/hook-map.json",
        "grep bash ~/.olp/projects/k/hook-map.json",
        "cat ~/.olp/projects/k/hook-map.json | wc -l",
    ] {
        assert!(!guarded("Bash", json!({ "command": cmd }), &p), "reads only: {cmd}");
    }
    let mut bad = Vec::new();
    for cmd in [
        "ls .olp > out.txt",
        "echo \"see ~/.olp\" > notes.md",
        "python tool.py ~/.olp/projects/k/hook-map.json",
        "X=~/.o*/projects/k; cd $X && echo x > hook-map.json",
        "node -e 1 ~/.ol?/projects/k/hook-map.json",
    ] {
        if !guarded("Bash", json!({ "command": cmd }), &p) {
            bad.push(cmd);
        }
    }
    assert!(bad.is_empty(), "not proven reads only but not self-protection: {bad:?}");
}

// Value: protects=second review RECHECK 10 (coordinator side) of rel f72e5fa3a — the reviewer's five fixed input files (REV5 inputs,
// inputs2, REV6 inputs3, REV7 inputs4, inputs5; notes left out, duplicates once) as one set: 45 self-protection (including python3 -c,
// node -e, bash -c and git init writing through `$'o'` paths, which f72e5fa3a let down to a card) and 20 not (reads among them);
// fails_when=one case differs; seam=classify::assess
#[test]
fn second_review_probe_set_rev7() {
    let p = cwd();
    let cases: &[(&str, bool, &str, &str)] = &[
        ("R8-1", true, "Bash", r#"X=~/.$'o'lp/projects/k; echo x > $X/hook-map.json"#),
        ("R8-2", true, "Bash", r#"X=~/.$'o'lp/projects/k; cp a.json $X/hook-map.json"#),
        ("R8-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; echo x > "$X"/hook-map.json"#),
        ("R8-4", true, "Bash", r#"X=~/.$'o'lp; echo x > ${X}/projects/k/hook-map.json"#),
        ("R8-5", true, "Bash", r#"echo x > ${HOME%/}/.$'o'lp/projects/k/hook-map.json"#),
        ("B12-1", true, "Bash", r#"echo x > ${HOME%/*}/h5/.$'o'lp/projects/k/hook-map.json"#),
        ("B12-2", true, "Bash", r#"echo x > ${HOME%/*}/h5/.o*/projects/*/hook-config.json"#),
        ("B12-3", true, "Bash", r#"echo x > ${HOME#x}/.$'o'lp/projects/k/hook-map.json"#),
        ("C2-1", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-map.json; echo x > $X"#),
        ("C2-2", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-map.json; cp a.json "$X""#),
        ("C2-3", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-map.json; cat a.json | tee $X"#),
        ("C2-4", true, "Bash", r#"X=~/.$'o'lp/projects/k/hook-config.json; echo x > ${X}"#),
        ("UP-1", true, "Bash", r#"X=~/.$'o'lp/projects/k; echo x > $X/HOOK-MAP.JSON"#),
        ("TP-1", true, "Bash", r#"cd ~ && echo x > ~+/.$'o'lp/projects/k/hook-map.json"#),
        ("TP-2", true, "Bash", r#"cd ~/.$'o'lp/projects/k && echo x > ~+/hook-map.json"#),
        ("TM-1", true, "Bash", r#"cd ~ && cd /tmp && echo x > ~-/.$'o'lp/projects/k/hook-map.json"#),
        ("LN-1", true, "Bash", r#"ln -s ~/.$'o'lp/projects/k/hook-map.json m; echo x > m"#),
        ("LN-2", true, "Bash", r#"X=~/.$'o'lp/projects/k; ln -s $X/hook-map.json m && echo x > m"#),
        ("LN-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; ln -s $X lnk && echo x > lnk/hook-map.json"#),
        ("EXE-1", true, "Bash", r#"X=~/$'o'lp; cp evil.exe $X/olp-hook.exe"#),
        ("EXE-2", true, "Bash", r#"X=~/$'o'lp; cp evil $X/olp-hook"#),
        ("PS-1", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json" -Value x"#),
        ("PS-DOT", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json." -Value x"#),
        ("PS-SP", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json " -Value x"#),
        ("PS-ADS", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Set-Content -Path "$X\hook-map.json::`$DATA" -Value x"#),
        ("OK-1", false, "Bash", r#"echo x > $OUT/notes.txt"#),
        ("OK-2", false, "Bash", r#"cp a.json $DIST/app.json"#),
        ("OK-3", false, "Bash", r#"echo x > ${HOME%/}/notes.txt"#),
        ("OK-4", false, "Bash", r#"echo x > ${HOME:-/tmp}/notes.txt"#),
        ("OK-5", false, "Bash", r#"echo x > $TMPDIR/out.txt"#),
        ("OK-6", false, "Bash", r#"X=out/a.txt; echo x > $X"#),
        ("OK-7", false, "Bash", r#"echo x > build/out.txt"#),
        ("OK-8", false, "Bash", r#"echo x > $HOME/notes.txt"#),
        ("OK-9", false, "Bash", r#"echo x > $OUT/hook-map.json.bak"#),
        ("OK-10", false, "PowerShell", r#"Set-Content -Path "$env:TEMP\notes.txt" -Value x"#),
        ("CD-1", true, "Bash", r#"cd ~/.$'o'lp/projects/k && echo x > hook-map.json"#),
        ("CD-2", true, "Bash", r#"cd ~/.$'o'lp/projects/k && cp a.json hook-map.json"#),
        ("CD-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; cd $X && echo x > hook-map.json"#),
        ("LN-4", true, "Bash", r#"ln -s ~/.$'o'lp/projects/k lnk && echo x > lnk/hook-map.json"#),
        ("PS-DOT2", true, "PowerShell", r#"Set-Content -Path "$env:USERPROFILE\.olp\projects\k\hook-map.json." -Value x"#),
        ("PS-VAR", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k\hook-map.json"; Set-Content -Path $X -Value x"#),
        ("OB-1", false, "Bash", r#"cat ~/.olp/hook-log.jsonl"#),
        ("OB-2", false, "Bash", r#"grep olp README.md"#),
        ("OB-3", false, "Bash", r#"echo "see .olp docs" | less"#),
        ("OB-4", false, "Bash", r#"git log -- .olp"#),
        ("OB-5", false, "Bash", r#"ls ~/.olp"#),
        ("OB-6", false, "Bash", r#"echo hi # see ~/.olp notes"#),
        ("OB-7", false, "Bash", r#"grep -rn ".olp" docs/"#),
        ("OB-8", false, "Bash", r#"head -5 ~/.olp/hook-map.json"#),
        ("OB-9", false, "Bash", r#"wc -l ~/.olp/hook-log.jsonl"#),
        ("OB-10", false, "PowerShell", r#"Get-Content "$env:USERPROFILE\.olp\hook-log.jsonl""#),
        ("W4-1", true, "Bash", r#"cd ~/.$'o'lp/projects/k && sed -i s/a/b/ hook-map.json"#),
        ("W4-2", true, "Bash", r#"X=~/.$'o'lp/projects/k; dd if=a.json of=$X/hook-map.json"#),
        ("W4-3", true, "Bash", r#"X=~/.$'o'lp/projects/k; truncate -s 0 $X/hook-map.json"#),
        ("W4-4", true, "Bash", r#"X=~/.$'o'lp/projects/k; mv a.json $X/hook-map.json"#),
        ("W4-5", true, "Bash", r#"X=~/.$'o'lp/projects/k; install -m 644 a.json $X/hook-map.json"#),
        ("W4-6", true, "Bash", r#"X=~/.$'o'lp/projects/k; perl -pi -e s/a/b/ $X/hook-map.json"#),
        ("W4-7", true, "Bash", r#"X=~/.$'o'lp/projects/k; python3 -c "open('$X/hook-map.json','w').write('x')""#),
        ("W4-8", true, "Bash", r#"X=~/.$'o'lp/projects/k; rsync a.json $X/hook-map.json"#),
        ("W4-9", true, "Bash", r#"cd ~/.$'o'lp/projects/k && git init"#),
        ("W4-10", true, "Bash", r#"X=~/.$'o'lp/projects/k; rm $X/hook-map.json"#),
        ("W4-11", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; 'x' | Out-File "$X\hook-map.json""#),
        ("W4-12", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; Copy-Item a.json "$X\hook-map.json""#),
        ("W4-13", true, "PowerShell", r#"$X = "$env:USERPROFILE\.o" + "lp\projects\k"; [IO.File]::WriteAllText("$X\hook-map.json", 'x')"#),
        ("W4-14", true, "Bash", r#"X=~/.$'o'lp/projects/k; touch $X/hook-map.json"#),
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter(|(_, want, tool, cmd)| guarded(tool, json!({ "command": cmd }), &p) != *want)
        .map(|(id, want, _, _)| format!("{id} want self-protection={want}"))
        .collect();
    assert_eq!((cases.iter().filter(|c| c.1).count(), cases.iter().filter(|c| !c.1).count()), (45, 20));
    assert!(bad.is_empty(), "probe set differs: {bad:?}");
}

// Value: protects=second review RECHECK 11 (coordinator side) of rel 586486bb0 — the reviewer's next five input files (REV8 inputs6-9,
// REV9 inputs10; notes and cases already in the earlier sets left out): 39 self-protection (letterless dot globs `~/.*`, `~/.???` and
// ANSI-C escapes `$'\x6f'` that name the trust folder among them) and 8 not; fails_when=one case differs; seam=classify::assess
#[test]
fn second_review_probe_set_rev9() {
    let p = cwd();
    let cases: &[(&str, bool, &str, &str)] = &[
        ("R6-3", false, "Bash", r#"stat ~/.olp/projects/k/hook-map.json"#),
        ("R6-5", false, "Bash", r#"jq . ~/.olp/projects/k/hook-map.json"#),
        ("R6-6", false, "Bash", r#"cat ~/.olp/projects/k/hook-map.json"#),
        ("R6-7", false, "Bash", r#"ls -la ~/.olp/projects"#),
        ("R6-8", false, "Bash", r#"cat ~/.olp/hook-log.jsonl | grep deny | wc -l"#),
        ("R6-9", false, "Bash", r#"tail -n 20 ~/.olp/hook-log.jsonl 2>/dev/null"#),
        ("R6-12", false, "PowerShell", r#"Get-Content "$env:USERPROFILE\.olp\projects\k\hook-map.json""#),
        ("R6-13", false, "PowerShell", r#"Get-ChildItem $HOME\.olp -Recurse | Measure-Object"#),
        ("X6-1", true, "Bash", r#"rg --pre rm x ~/.olp/projects/k/hook-map.json"#),
        ("X6-2", true, "Bash", r#"rg --pre=./tools/fix.sh x ~/.olp/projects/k/hook-map.json"#),
        ("X6-3", true, "Bash", r#"cat notes.txt | less -O ~/.olp/projects/k/hook-map.json"#),
        ("X6-4", true, "Bash", r#"cat notes.txt | less --LOG-FILE=$HOME/.olp/projects/k/hook-map.json"#),
        ("X6-5", true, "Bash", r#"grep -r x ~/.$'o'lp/projects/k --pre rm"#),
        ("X6-6", true, "PowerShell", r#"Get-Content ([IO.File]::WriteAllText("$HOME\.olp\projects\k\hook-map.json", 'x'))"#),
        ("X6-7", true, "PowerShell", r#"Get-Item (Set-Content -Path "$HOME\.olp\projects\k\hook-map.json" -Value x)"#),
        ("X6-8", true, "PowerShell", r#"Get-Content $(Remove-Item "$HOME\.olp\projects\k\hook-map.json")"#),
        ("X6-9", true, "Bash", r#"file -C -m ~/.olp/projects/k/hook-map.json"#),
        ("X6-10", true, "Bash", r#"rg --pre rm x ~/.o*/projects/k/hook-map.json"#),
        ("E8-1", true, "Bash", r#"echo x > ~/.$'\x6f'lp/projects/k/hook-map.json"#),
        ("E8-2", true, "Bash", r#"echo x > ~/.$'\157'lp/projects/k/hook-map.json"#),
        ("E8-3", true, "Bash", r#"cd ~/.$'\x6f'lp/projects/k && echo x > hook-map.json"#),
        ("E8-4", true, "Bash", r#"X=~/.$'\x6f'lp/projects/k; python3 -c "open('$X/hook-map.json','w').write('x')""#),
        ("E8-5", true, "Bash", r#"cd ~/.$'\u006f'lp/projects/k && git init"#),
        ("E8-6", true, "Bash", r#"cp a.json ~/.$'\x6f\x6c\x70'/projects/k/hook-map.json"#),
        ("G8-1", true, "Bash", r#"echo x | tee ~/.{o,}lp/projects/k/hook-map.json"#),
        ("G8-2", true, "Bash", r#"for d in ~/.???; do echo x > $d/projects/k/hook-map.json; done"#),
        ("G8-3", true, "Bash", r#"echo x | tee ~/.???/projects/k/hook-map.json"#),
        ("G8-4", true, "Bash", r#"for d in ~/.*/projects/k; do cp a.json $d/hook-map.json; done"#),
        ("P8-2", true, "PowerShell", r#"Set-Content -Path "$env:USERPROFILE\.`olp\projects\k\hook-map.json" -Value x"#),
        ("P8-3", true, "PowerShell", r#"Set-Content -Path "$env:USERPROFILE\.?lp\projects\k\hook-map.json" -Value x"#),
        ("Q9-1", true, "Bash", r#"for d in ~/.???/projects/k; do cd $d && echo x > hook-map.json; done"#),
        ("Q9-2", true, "Bash", r#"for d in ~/.???/projects/k; do python3 -c "open('$d/hook-map.json','w').write('x')"; done"#),
        ("Q9-3", true, "Bash", r#"for d in ~/.*/projects/k; do python3 -c "open('$d/hook-map.json','w').write('x')"; done"#),
        ("Q9-4", true, "Bash", r#"for d in ~/.???/projects/k; do (cd $d; git init); done"#),
        ("Q9-5", true, "Bash", r#"cd ~/.[!a-n]?p/projects/k && git init"#),
        ("Q9-6", true, "Bash", r#"cd ~/.?l?/projects/k && git init"#),
        ("Q9-7", true, "Bash", r#"for d in ~/.*/projects/k; do cd $d && git init; done"#),
        ("T10-3", true, "Bash", r#"git diff --output=$HOME/.olp/projects/k/hook-map.json"#),
        ("T10-4", true, "Bash", r#"git log -p --output ~/.olp/projects/k/hook-map.json"#),
        ("T10-5", true, "Bash", r#"git -C ~/.olp/projects/k init"#),
        ("T10-6", true, "Bash", r#"git -c core.pager=touch log -- ~/.olp/projects/k/hook-map.json"#),
        ("T10-7", true, "Bash", r#"echo x > ~/.olp/projects/k/hook-map.json"#),
        ("T10-8", true, "Bash", r#"echo x | tee ~/.olp/projects/k/hook-map.json"#),
        ("T10-9", true, "Bash", r#"echo ok # ~/.olp
cp a.json ~/.olp/projects/k/hook-map.json"#),
        ("T10-10", true, "Bash", r#"git diff --no-index --output=/dev/stdout a ~/.olp/projects/k/hook-map.json"#),
        ("T10-12", true, "Bash", r#"git stash -- ~/.olp/projects/k/hook-map.json"#),
        ("T10-13", true, "Bash", r#"git checkout -- ~/.olp/projects/k/hook-map.json"#),
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter(|(_, want, tool, cmd)| guarded(tool, json!({ "command": cmd }), &p) != *want)
        .map(|(id, want, _, _)| format!("{id} want self-protection={want}"))
        .collect();
    assert_eq!((cases.iter().filter(|c| c.1).count(), cases.iter().filter(|c| !c.1).count()), (39, 8));
    assert!(bad.is_empty(), "probe set differs: {bad:?}");
}

// Value: protects=second review RECHECK 12 (coordinator side) of rel a772e253d — the reviewer's eleventh input file without the nine
// glob range/class/brace shapes that wait for the owner's call: «reads only» never clears self-protection for less with a bundled
// `o`/`O` option or a `+` command, or rg --hostname-bin (they write or run something); fails_when=one case differs; seam=classify::assess
#[test]
fn second_review_probe_set_rev10() {
    let p = cwd();
    let cases: &[(&str, bool, &str, &str)] = &[
        ("K11-9", true, "Bash", r#"cd ~/.$'\U0000006f'lp/projects/k && git init"#),
        ("K11-10", true, "Bash", r#"echo x 1<> ~/.olp/projects/k/hook-map.json"#),
        ("K11-11", true, "Bash", r#"echo x >| ~/.olp/projects/k/hook-map.json"#),
        ("K11-12", true, "Bash", r#"echo x &> ~/.olp/projects/k/hook-map.json"#),
        ("K11-13", true, "Bash", r#"echo x >/dev/null >~/.olp/projects/k/hook-map.json"#),
        ("K11-14", true, "Bash", r#"cat notes.txt | less "+s$HOME/.olp/projects/k/hook-map.json""#),
        ("K11-15", true, "Bash", r#"cat notes.txt | less -So ~/.olp/projects/k/hook-map.json"#),
        ("K11-16", true, "Bash", r#"cat notes.txt | less -SO ~/.olp/projects/k/hook-map.json"#),
        ("K11-17", true, "Bash", r#"rg --hostname-bin=./tools/fix.sh --hyperlink-format=default x ~/.olp/projects/k/hook-map.json"#),
        ("K11-18", true, "Bash", r#"cat ~/.olp/projects/k/hook-map.json;rm ~/.olp/projects/k/hook-map.json"#),
        ("K11-19", true, "Bash", r#"ls ~/.olp
rm ~/.olp/projects/k/hook-map.json"#),
        ("K11-20", true, "Bash", r#"cat ~/.olp/projects/k/hook-map.json |& tee ~/.olp/projects/k/x.json"#),
        ("K11-21", true, "Bash", r#"ls ~/.olp 2>~/.olp/projects/k/hook-map.json"#),
        ("K11-22", true, "Bash", r#"echo $(touch ~/.olp/projects/k/hook-map.json)"#),
        ("K11-23", true, "Bash", r#"cat <(cp a.json ~/.olp/projects/k/hook-map.json)"#),
        ("K11-24", true, "PowerShell", r#"Get-ChildItem $HOME\.olp\projects\k | Measure-Object -Property { Set-Content $_.FullName x; 1 } -Sum"#),
        ("K11-25", true, "PowerShell", r#"Get-Content "$(Remove-Item $HOME\.olp\projects\k\hook-map.json)""#),
        ("K11-26", true, "PowerShell", r#"Get-ChildItem $HOME\.olp\projects\k | % { Remove-Item $_ }"#),
        ("K11-28", false, "Bash", r#"cat < ~/.olp/projects/k/hook-map.json"#),
        ("K11-29", false, "Bash", r#"grep -rn deny ~/.olp --include=*.jsonl"#),
        ("K11-30", false, "Bash", r#"jq -n --rawfile a ~/.olp/projects/k/hook-map.json '$a'"#),
        ("K11-31", false, "PowerShell", r#"Get-Content $HOME\.olp\projects\k\hook-map.json -OutVariable x"#),
        ("K11-32", false, "Bash", r#"ls -la ~/.config ~/.cache"#),
        ("K11-33", false, "Bash", r#"echo x > ~/.env.local"#),
        ("K11-34", false, "Bash", r#"cp a.json ~/.npmrc"#),
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter(|(_, want, tool, cmd)| guarded(tool, json!({ "command": cmd }), &p) != *want)
        .map(|(id, want, _, _)| format!("{id} want self-protection={want}"))
        .collect();
    assert!(bad.is_empty(), "probe set differs: {bad:?}");
}

// Value: protects=design addendum 2 of 11-15 — every expansion the shell does from the text alone (glob ranges and classes, braces,
// extended globs, PowerShell wildcards) is matched by one function against the trust folder name, and a pattern it cannot read counts as
// reaching it; ordinary globs that cannot name a dot folder stay ordinary; fails_when=one of the shapes differs; seam=classify::assess
#[test]
fn text_only_expansions_reach_the_trust_folder() {
    let p = cwd();
    let reach: &[(&str, bool, &str, &str)] = &[
        ("K11-1", true, "Bash", r#"cd ~/.[n-p]lp/projects/k && git init"#),
        ("K11-2", true, "Bash", r#"echo x > ~/.[n-p]lp/projects/k/hook-map.json"#),
        ("K11-3", true, "Bash", r#"cd ~/.[[:lower:]]lp/projects/k && git init"#),
        ("K11-4", true, "Bash", r#"cd ~/.[a-z]l[a-z]/projects/k && git init"#),
        ("K11-5", true, "Bash", r#"for d in ~/.[a-z]lp/projects/k; do python3 -c "open('$d/hook-map.json','w').write('x')"; done"#),
        ("K11-6", true, "Bash", r#"cd ~/{.o,x}lp/projects/k && git init"#),
        ("K11-7", true, "Bash", r#"cd ~/.{o..o}lp/projects/k && git init"#),
        ("K11-8", true, "Bash", r#"shopt -s extglob
cd ~/.@(o)lp/projects/k && git init"#),
        ("K11-27", true, "PowerShell", r#"Get-ChildItem "$env:USERPROFILE\.[n-p]lp\projects\k" | Remove-Item"#),
        ("U-1", true, "Bash", r#"echo x > ~/.[n-p/projects/k/hook-map.json"#),
        ("U-2", true, "Bash", r#"echo x > ~/.@(o|@(x)lp/projects/k/hook-map.json"#),
        ("U-3", true, "Bash", r#"echo x > ~/.{a..zzzzzzzz}/projects/k/hook-map.json"#),
        // addendum 2-1 (a): an unreadable part at the very front of a path-place piece reaches (second review RECHECK 13, blocking 1)
        ("U-4", true, "Bash", r#"cd ~/[[:lower:]lp/projects/k && git init"#),
        ("U-5", true, "Bash", r#"echo x > ~/[.o/projects/k/hook-map.json"#),
        // more than the brace cap at the front of a piece reaches: bash gives 512 names, one is ~/.olp/projects/k (RECHECK 13, blocking 2)
        ("U-6", true, "Bash", r#"for d in ~/{.o,a}{l,b}{p,c}{,1}{,2}{,3}{,4}{,5}{,6}/projects/k; do cd $d && git init; done"#),
        // addendum 2-1 (b): letters before the unreadable part that cannot start the name do not reach
        ("U-7", false, "Bash", r#"echo x > ~/x[l/projects/k/hook-map.json"#),
        ("U-8", false, "Bash", r#"shopt -s extglob
cd ~/.x@(p/projects/k && git init"#),
        ("N-1", false, "Bash", r#"rm build/*.o"#),
        ("N-6", false, "Bash", r#"python3 -c "a=[a, 1]; print(a)" > out.txt"#),
        ("N-7", false, "Bash", r#"echo '[a,' > out.txt"#),
        ("N-8", false, "Bash", r#"cp a.txt out/file{1..300}.txt"#),
        ("N-9", false, "Bash", r#"cp a.txt out/{1..300}.txt"#),
        ("N-2", false, "Bash", r#"cp a.txt dist/{x,y}.txt"#),
        ("N-3", false, "Bash", r#"echo x > logs/[0-9]*.log"#),
        ("N-4", false, "Bash", r#"rm -f out/file?.tmp"#),
        ("N-5", false, "PowerShell", r#"Remove-Item build\*.obj"#),
        ("L-1", true, "Bash", r#"cat notes.txt | less -So ~/.olp/projects/k/hook-map.json"#),
        ("L-2", true, "Bash", r#"cat notes.txt | less -SO ~/.olp/projects/k/hook-map.json"#),
        ("L-3", true, "Bash", r#"cat notes.txt | less "+s$HOME/.olp/projects/k/hook-map.json""#),
    ];
    let bad: Vec<String> = reach
        .iter()
        .filter(|(_, want, tool, cmd)| guarded(tool, json!({ "command": cmd }), &p) != *want)
        .map(|(id, want, _, _)| format!("{id} want self-protection={want}"))
        .collect();
    assert!(bad.is_empty(), "text-only expansion check differs: {bad:?}");
}

// Value: protects=design addendum 2-2 — Windows and macOS compare names case-folded, and Windows drops a trailing dot or blank, so
// `.OLP`, `.Olp`, `.olp.` and `.olp ` name the trust folder too; fails_when=one of these writes is not self-protection; seam=classify::assess
#[test]
fn folded_and_trimmed_names_are_the_trust_folder() {
    let p = cwd();
    let mut bad = Vec::new();
    for (tool, cmd) in [
        ("Bash", "echo x > ~/.OLP/projects/k/hook-map.json"),
        ("Bash", "cp a.json ~/.Olp/projects/k/HOOK-MAP.JSON"),
        ("Bash", "echo x > \"$HOME/.olp./projects/k/hook-map.json\""),
        ("Bash", "echo x > \"$HOME/.olp /projects/k/hook-map.json\""),
        ("PowerShell", r"Set-Content -Path $HOME\.OLP\projects\k\hook-map.json -Value x"),
        ("PowerShell", r#"Set-Content -Path "$env:USERPROFILE\.olp.\projects\k\hook-map.json" -Value x"#),
    ] {
        if !guarded(tool, json!({ "command": cmd }), &p) {
            bad.push(format!("{tool} {cmd}"));
        }
    }
    assert!(bad.is_empty(), "a folded or trimmed name slips past self-protection: {bad:?}");
}

// Value: protects=an ordinary target is not OLP's state just because the working folder was given as a relative path (`.`): the target
// is made absolute before its links are resolved (rc CI run 43724580621 — `where-object x`, `[IO.File]::Delete('a')` and every
// relative target were judged as self-protection with cwd `.`, because the relative path ran out before a folder could be resolved);
// fails_when=`touch notes.txt` in `.` asks as self-protection, or a trust-folder write in `.` stops being one; seam=classify::assess
// (self_guard); no_claim=none beyond protects.
#[test]
fn relative_working_folder_does_not_make_every_target_olp_state() {
    let dot = std::path::Path::new(".");
    for cmd in ["touch notes.txt", "rm -f build/out.o", "cp a.txt b.txt"] {
        assert!(!guarded("Bash", json!({ "command": cmd }), dot), "{cmd} in `.`: an ordinary target must not be self-protection");
    }
    assert!(guarded("Bash", json!({ "command": "rm -f .olp/hook-map.json" }), dot), "the trust folder in `.` stays self-protection");
    assert!(guarded("Bash", json!({ "command": "cp x ~/.olp/projects/k/hook-map.json" }), dot));
}

// Value: protects=design addendum 4-3 — a Windows drive letter that no trust-folder root is on and that does not exist now (`x:`) can be
// shown not to reach the trust folder, so it is not self-protection unless the command names `olp` (the same command may still map the
// drive) (rc CI run 43724580621, bundle_no_memory `mytool run x:` lost its remembered answer on the Windows runner); fails_when=`x:` alone
// is self-protection, or `x:olp/...` is not; seam=classify::assess; no_claim=Windows only (on other systems `x:` is an ordinary file name).
#[cfg(windows)]
#[test]
fn missing_windows_drive_is_unsure_not_olp_state() {
    let free = ('D'..='Z').rev().find(|d| !std::path::Path::new(&format!("{d}:\\")).exists()).expect("a free drive letter");
    let at = cwd();
    assert!(!guarded("Bash", json!({ "command": format!("touch {free}:notes.txt") }), &at));
    assert!(guarded("Bash", json!({ "command": format!("rm -rf {free}:olp/bin") }), &at));
}

// Value: protects=design addendum 4-3 test 1 — a target the hook cannot resolve but the shell can (a variable that is not a home
// variable, a folder the parser cannot follow) still reaches the trust folder: the fix opened only targets shown not to be it;
// fails_when=an unresolvable target that can name the trust folder stops being self-protection in a relative working folder; seam=classify::assess;
// no_claim=none beyond protects.
#[test]
fn unresolvable_targets_that_can_name_the_trust_folder_stay_closed() {
    let dot = std::path::Path::new(".");
    for cmd in ["rm -rf \"$D/olp\"", "cp x \"$D\"/projects/k/hook-map.json", "cd \"$D\"; rm -rf olp/bin", "rm -rf ~other/.olp"] {
        assert!(guarded("Bash", json!({ "command": cmd }), dot), "{cmd} in `.`: the hook cannot resolve it, the shell can — must stay self-protection");
    }
}

// Value: protects=design addendum 4-3 test 2 — a drive-relative target on the trust folder's own drive (`c:.olp`, against that drive's
// working folder) is resolved, not let through as «another drive»; fails_when=`<home drive>:.olp` or `<home drive>:olp/bin` from the home
// folder is not self-protection; seam=classify::assess; no_claim=Windows only.
#[cfg(windows)]
#[test]
fn same_drive_relative_target_is_resolved() {
    let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let d = home.to_string_lossy().chars().next().expect("home drive");
    assert!(guarded("Bash", json!({ "command": format!("rm -rf {d}:.olp") }), &cwd()));
    assert!(guarded("PowerShell", json!({ "command": format!("Remove-Item -Recurse {d}:.olp/projects") }), &cwd()));
    assert!(guarded("Bash", json!({ "command": format!("rm -rf {d}:olp/bin") }), &home));
}

// Value: protects=design addendum 4-3 as written — a drive-relative target on a drive letter none of the trust-folder roots is on is
// not the trust folder, whether that drive exists now or not (B FAIL R26 of rel 8d914a1e0: 15 ordinary forms on an existing other
// drive — `Set-Location D:`, `Copy-Item a.txt D:`, `cp a.txt D:`, `touch D:notes.txt` … — had become self-protection); naming `olp`
// or a trust file keeps it closed; the same rule when the parser's folder is unknown after a folder move; fails_when=one of those
// forms asks as self-protection, or `D:olp/bin` / `D:hook-map.json` stops being one; seam=classify::assess; no_claim=Windows only, and
// only when this machine has an existing drive other than the home drive.
#[cfg(windows)]
#[test]
fn existing_other_drive_is_not_the_trust_folder() {
    let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let h = home.to_string_lossy().chars().next().expect("home drive").to_ascii_uppercase();
    let Some(o) = ('C'..='Z').find(|d| *d != h && std::path::Path::new(&format!("{d}:\\")).exists()) else {
        eprintln!("no existing drive other than {h}: — nothing to check here");
        return;
    };
    let on_o = std::path::PathBuf::from(format!("{o}:\\"));
    for at in [cwd(), home.clone(), on_o] {
        for (tool, cmd) in [
            ("PowerShell", format!("{o}:")),
            ("PowerShell", format!("Set-Location {o}:")),
            ("PowerShell", format!("Push-Location {o}:")),
            ("PowerShell", format!("Copy-Item a.txt {o}:")),
            ("PowerShell", format!("Remove-Item {o}:foo.txt")),
            ("PowerShell", format!("Set-Content {o}:notes.txt x")),
            ("PowerShell", format!("Out-File -FilePath {o}:log.txt")),
            ("Bash", format!("cp a.txt {o}:")),
            ("Bash", format!("touch {o}:notes.txt")),
            ("Bash", format!("rm -rf {o}:notes.txt")),
            ("Bash", format!("xcopy src {o}: /E")),
            ("Bash", format!("robocopy src {o}:")),
            ("Bash", format!(r"mytool run {o}:\x\y.txt")),
        ] {
            assert!(!guarded(tool, json!({ "command": cmd }), &at), "{tool} `{cmd}` at {}: another drive is not the trust folder", at.display());
        }
        // the closed pairs apply when that drive's current folder is unknown (the working folder is on another drive); on the drive itself
        // `{o}:olp/bin` is resolved against the working folder (a project folder named olp, shown not to be the trust folder)
        let at_drive = at.to_string_lossy().chars().next().map(|c| c.to_ascii_uppercase());
        if at_drive == Some(o) {
            continue;
        }
        for (tool, cmd) in [("Bash", format!("rm -rf {o}:olp/bin")), ("PowerShell", format!("Remove-Item {o}:hook-map.json")), ("Bash", format!("rm -rf {o}:.olp"))] {
            assert!(guarded(tool, json!({ "command": cmd }), &at), "{tool} `{cmd}` at {}: names olp or a trust file — stays closed", at.display());
        }
    }
}

// Value: protects=the Git Bash spelling of the home (`/c/Users/<me>/…`, the Bash tool's own `$HOME` and `pwd` on Windows) reaches
// the installed hook folder `<home>/olp` the same as `C:/Users/<me>/…` does (second review coord RECHECK 19 of rel 1a188ce02: since
// be8572fae `std::path::absolute` turned the rootless `\c\Users\…` into `<process drive>:\c\Users\…`, so six forms — two of them
// plain, one a trust file `olp/hook-config.json` — fell from self-protection to an ordinary card); fails_when=one of these forms is not
// self-protection while its `C:/` spelling is; seam=classify::assess; no_claim=Windows only (elsewhere `/c/` is an ordinary folder).
#[cfg(windows)]
#[test]
fn git_bash_home_spelling_reaches_the_install_folder() {
    let home = std::path::PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let h = home.to_string_lossy().replace('\\', "/");
    let gb = format!("/{}{}", h[..1].to_ascii_lowercase(), &h[2..]);
    for cmd in [
        format!("cp a {gb}/olp/hook-config.json"),
        format!("touch {gb}/olp/x.txt"),
        format!("touch {gb}/o?p/x.txt"),
        format!("cp a {gb}/o*/x.txt"),
        format!("cp evil {gb}/olp/bin/run.sh"),
        format!("touch {gb}/olp"),
        format!("cp a {gb}/.olp/projects/k/hook-map.json"),
    ] {
        assert!(guarded("Bash", json!({ "command": cmd }), &home), "{cmd}: the Git Bash spelling of the home must reach <home>/olp");
        let c = cmd.replace(&gb, &h);
        assert!(guarded("Bash", json!({ "command": c }), &home), "{c}: the C:/ spelling (control)");
    }
    assert!(!guarded("Bash", json!({ "command": format!("touch {gb}/notes.txt") }), &home), "an ordinary file in the home stays ordinary");
}
