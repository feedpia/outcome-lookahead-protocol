//! 0.1.3 rc4 (DESIGN_OLP013_RC4_RUNNER_KEY §3 + REVIEW_DESIGN_OLP013_RC4_RUNNER_KEY blocking 1·2): a package runner's group key is
//! «runner + package name». One package's answer never covers another, a key with only the runner is never stored, read or offered.
//! Measured before the fix (0.1.2 and 0.1.3 rc3): `npx @feedpia/olp stats` was stored as bare `npx`, the block card itself
//! suggested `classify --bash "npx"`, and afterwards `npx -y rimraf x` passed silently.
//! All file operations happen only in throwaway folders under CARGO_TARGET_TMPDIR.
use olp_hook::classify::{family_is_name, family_of};
use olp_hook::{classify_cli, run_hook, HookOutcome};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_runner_family").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(p.join("x")).unwrap();
    for i in 0..12 {
        fs::write(p.join("x").join(format!("f{i}.txt")), "x").unwrap();
    }
    // rev 17-3: answers live in the user's trust folder for this project path; drop leftovers from an earlier run
    let _ = fs::remove_dir_all(olp_hook::store::trust_dir(&p));
    p
}

/// rev 17-3: the answer file in the user's trust folder for this project (never inside the project).
fn map_file(p: &Path) -> PathBuf {
    olp_hook::store::trust_dir(p).join("hook-map.json")
}

/// Write a pre-existing answer file where the hook reads it.
fn write_map(p: &Path, map: &str) {
    fs::create_dir_all(olp_hook::store::trust_dir(p)).unwrap();
    fs::write(map_file(p), map).unwrap();
}

fn fam(cmd: &str) -> String {
    let words: Vec<String> = olp_hook::shell::parse(cmd).segments[0].words.iter().map(|w| w.text.clone()).collect();
    family_of(&words).unwrap()
}

/// A key that takes a group answer: shaped like a name and not a runner on its own.
fn named(cmd: &str) -> Option<String> {
    let f = fam(cmd);
    family_is_name(&f).then_some(f)
}

fn hook(p: &Path, cmd: &str, mode: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "Bash",
                   "tool_input": {"command": cmd}, "permission_mode": mode, "tool_use_id": "toolu_r"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn classify(p: &Path, cmd: &str) -> Result<String, String> {
    classify_cli(p, &["--bash", cmd, "read_only"].map(String::from))
}

const BARE: [&str; 13] = ["npx", "pnpx", "bunx", "npm exec", "npm x", "pnpm dlx", "yarn dlx", "bun x", "npm", "pnpm", "yarn", "bun", "npx.cmd"];

fn offers_bare(out: &str) -> Option<&'static str> {
    BARE.iter().copied().find(|b| out.contains(&format!("--bash \"{b}\"")))
}

// Value: protects=§2-1/2-2 4: each of the 8 runners keys on runner + package, scoped and plain names; fails_when=a runner call keys on the runner alone; seam=classify::family_of
#[test]
fn runner_key_includes_package() {
    for r in ["npx", "pnpx", "bunx", "npm exec", "npm x", "pnpm dlx", "yarn dlx", "bun x"] {
        assert_eq!(named(&format!("{r} @scope/tool stats")).as_deref(), Some(format!("{r} @scope/tool").as_str()), "{r}");
        assert_eq!(named(&format!("{r} cowsay hi")).as_deref(), Some(format!("{r} cowsay").as_str()), "{r}");
    }
}

// Value: protects=§2-2 2: value-less flags right after the runner are skipped; fails_when=`npx -y cowsay` keys on bare npx; seam=classify::family_of
#[test]
fn runner_flags_skipped_for_key() {
    for cmd in ["npx -y cowsay", "npx --yes -- cowsay", "npx -q --no-install cowsay moo", "npm exec --yes cowsay"] {
        assert_eq!(named(cmd).as_deref(), Some(if cmd.starts_with("npm") { "npm exec cowsay" } else { "npx cowsay" }), "{cmd}");
    }
}

// Value: protects=§2-2 3: a version spec after @ is dropped and the name is lowercased; fails_when=two versions of one package need two answers or keep the spec; seam=classify::family_of
#[test]
fn runner_version_suffix_stripped() {
    assert_eq!(named("npx @scope/tool@0.1.3 stats").as_deref(), Some("npx @scope/tool"));
    assert_eq!(named("npx cowsay@latest").as_deref(), Some("npx cowsay"));
    assert_eq!(named("npx cowsay@^1.2 x").as_deref(), Some("npx cowsay"));
    assert_eq!(named("npx Cowsay").as_deref(), Some("npx cowsay"));
}

// Value: protects=review blocking 1: an alias, file, git or URL spec after @ names another package, so the call takes no group answer; fails_when=`npx cowsay@npm:other` keys on `npx cowsay` (mutation: dropping everything after @ must turn this red); seam=classify::family_of
#[test]
fn runner_alias_or_url_spec_is_unnamed() {
    for cmd in ["npx cowsay@npm:x", "npx cowsay@file:../x", "npx cowsay@github:u/r", "npx cowsay@https://e.x/a.tgz", "npx @a/b@npm:c", "npx cowsay@"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
}

// Value: protects=§2-2 5: a package slot that is missing, a path, URL, variable or behind a flag with a value takes no group answer; fails_when=any of these gets a key that can be stored; seam=classify::family_of
#[test]
fn runner_unnameable_package_takes_no_group_answer() {
    for cmd in ["npx ./x", "npx $P", "npx -p a b", "npx -c 'x'", "npx https://e.x/a.tgz", "npx", "npx git+https://e.x/r.git", "npx --package=a b", "pnpm dlx", "npx -w ws x"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
}

// Value: protects=review blocking 2 ⓐ: a global flag before the runner word (`npm --yes exec`) takes no group answer, and the one-word program never becomes a key; fails_when=the card offers `--bash "npm"`; seam=classify::family_of + run_hook card
#[test]
fn runner_behind_global_flag_takes_no_group_answer() {
    for cmd in ["npm --yes exec x", "npm --prefix tools exec x", "pnpm --silent dlx x", "yarn --cwd a dlx x", "bun --bun x y", "npm --version"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
    let p = tmp("global_flag");
    for cmd in ["npm --yes exec x", "pnpm --silent dlx x"] {
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(offers_bare(&o.stderr), None, "{cmd}: {}", o.stderr);
    }
}

// Value: protects=review blocking 2 ⓑ: `.cmd`/`.ps1` runner names are the runner (Windows); fails_when=`npx.cmd @a/b` keys on `npx.cmd` and the card offers it; seam=classify::family_of + run_hook card
#[test]
fn windows_runner_suffix_is_runner() {
    assert_eq!(named("npx.cmd @a/b run").as_deref(), Some("npx @a/b"));
    assert_eq!(named("npx.ps1 cowsay").as_deref(), Some("npx cowsay"));
    assert_eq!(named("pnpm.cmd dlx create-vite").as_deref(), Some("pnpm dlx create-vite"));
    let p = tmp("win_suffix");
    let o = hook(&p, "npx.cmd @a/b run", "bypassPermissions");
    assert_eq!(offers_bare(&o.stderr), None, "{}", o.stderr);
}

// Value: protects=§2-5: no input ever makes the card offer a runner-only answer; fails_when=any card line names only a runner; seam=run_hook card
#[test]
fn card_never_offers_bare_runner() {
    let p = tmp("card");
    for cmd in ["npx @feedpia/olp stats", "npx -y rimraf x", "npx ./x", "npx $P", "npx -p a b", "npx -c 'rm -rf x'", "npx https://e.x/a.tgz", "npx",
                "npx cowsay@npm:x", "npm exec @a/b", "pnpm dlx create-vite", "bun x foo", "npm --yes exec x", "npx.cmd @a/b"] {
        for mode in ["bypassPermissions", "default"] {
            let o = hook(&p, cmd, mode);
            assert_eq!(offers_bare(&format!("{}{}", o.stderr, o.stdout)), None, "{cmd} [{mode}]: {} {}", o.stderr, o.stdout);
        }
    }
    assert!(hook(&p, "npx tool stats", "bypassPermissions").stderr.contains("classify --bash \"npx tool\""));
    // Design 1-20: `@` is outside the allowed characters (PowerShell `@name` splats a variable), so a scoped package never gets a remembered answer offered
    assert!(!hook(&p, "npx @scope/tool stats", "bypassPermissions").stderr.contains("classify --bash"));
}

// Value: protects=§2-4 + review blocking 2 ③: a runner-only or one-word program key is refused with the add-the-package line, and the map file is not created or changed; fails_when=classify stores `bash:npx` or `bash:npm`; seam=classify_cli
#[test]
fn classify_refuses_bare_runner() {
    let p = tmp("refuse");
    for cmd in BARE {
        let e = classify(&p, cmd).unwrap_err();
        assert!(e.contains("Add the package name: olp-hook classify --bash \"npx <package>\" read_only"), "{cmd}: {e}");
    }
    assert!(!map_file(&p).exists());
}

// Value: protects=§2-3: typing exactly what the card offers stores the same key the call looks up, and the long form stores it too; fails_when=the card's line stores a key the next call does not find; seam=classify_cli + run_hook
#[test]
fn classify_key_is_idempotent() {
    for typed in ["npx tool", "npx tool stats", "npx tool@0.1.3 stats"] {
        let p = tmp("idem");
        let msg = classify(&p, typed).unwrap();
        assert!(msg.contains("bash:npx tool "), "{typed}: {msg}");
        assert_eq!(hook(&p, "npx tool stats", "bypassPermissions").code, 0, "{typed}");
        // Design 1-20: the same answer never covers a command with `@` (asks every time)
        assert_ne!(hook(&p, "npx tool@0.1.3 stats", "bypassPermissions").code, 0, "{typed}");
    }
}

// Value: protects=§2-6 + review blocking 2 ③: old runner-only entries (`bash:npx`, `bash:npm`, `bash:npm exec`) in a map written by 0.1.2/0.1.3 rc1-rc3 are never read, the call asks again and the file stays byte-for-byte; fails_when=an old map keeps covering every npx call; seam=run_hook over a pre-existing hook-map.json
#[test]
fn old_bare_runner_entry_is_ignored() {
    let p = tmp("old_map");
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    let map = json!({"version": 1, "entries": {"bash:npx": ro, "bash:npm": ro, "bash:npm exec": ro, "bash:pnpm dlx": ro}}).to_string();
    write_map(&p, &map);
    for cmd in ["npx -y rimraf x", "npx @some/cleaner .", "npx @feedpia/olp stats", "npm -y exec x", "npm exec cowsay", "pnpm dlx x"] {
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
    assert!(hook(&p, "npx -y rimraf x", "bypassPermissions").stderr.contains("classify --bash \"npx rimraf\""));
    assert_eq!(fs::read_to_string(map_file(&p)).unwrap(), map);
}

// Value: protects=the measured gap itself: answering one npx package leaves every other npx call asking (default) and blocked (no prompt), with the 4 forms that passed in 0.1.2/0.1.3 rc3; fails_when=any of them passes silently again; seam=classify_cli + run_hook
#[test]
fn one_package_answer_does_not_cover_another() {
    let p = tmp("measured4");
    classify(&p, "npx tool stats").unwrap();
    assert_eq!(hook(&p, "npx tool stats", "bypassPermissions").code, 0);
    for cmd in ["npx @some/cleaner .", "npx @feedpia/olp-evil clear", "npx -y rimraf x", "npx --yes @x/rm-tool x", "npx cowsay"] {
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
        assert!(hook(&p, cmd, "default").stdout.contains("\"ask\""), "{cmd}");
    }
    assert_eq!(fs::read_dir(p.join("x")).unwrap().count(), 12);
}

// Value: protects=§2-7: keys outside the runners are exactly what 0.1.x made (design 1-6: `pnpm exec` is now a runner, so its key carries the program) (incl. one-word programs with a plain second word); fails_when=the fix changes any other key; seam=classify::family_of
#[test]
fn non_runner_keys_unchanged() {
    for (cmd, want) in [("npm run build", "npm run"), ("npm test", "npm test"), ("cargo test --all", "cargo test"), ("7z a x.7z d", "7z a"), ("2to3 f.py", "2to3"),
                        ("pnpm exec tsc", "pnpm exec tsc"), ("yarn build", "yarn build"), ("git status", "git status"), ("ls -la", "ls")] {
        assert_eq!(named(cmd).as_deref(), Some(want), "{cmd}");
    }
}

// Value: protects=the four forms B found in U3a review (`npm --yes exec`, `npx -p other <bin>`, `npm exec -c '<cmd>'`, `npx.cmd <pkg>`) never take an answer given to a whole runner; fails_when=an old runner-only entry or a stored runner key lets any of them run silently; seam=classify::family_of + run_hook over a pre-existing hook-map.json
#[test]
fn u3a_adversarial_forms_take_no_group_answer() {
    for cmd in ["npm --yes exec rimraf x", "npx -p other-pkg rimraf x", "npm exec -c 'rimraf x'"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
    assert_eq!(named("npx.cmd rimraf x").as_deref(), Some("npx rimraf"));
    let p = tmp("u3a_forms");
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    let map = json!({"version": 1, "entries": {"bash:npx": ro, "bash:npm": ro, "bash:npm exec": ro, "bash:npx other-pkg": ro}}).to_string();
    write_map(&p, &map);
    for cmd in ["npm --yes exec rimraf x", "npx -p other-pkg rimraf x", "npm exec -c 'rimraf x'", "npx.cmd rimraf x"] {
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(o.code, 2, "{cmd}");
        assert_eq!(offers_bare(&o.stderr), None, "{cmd}: {}", o.stderr);
    }
}

// ---- Interpreter keys (coordinator decision 11-14, design 1-3 line; measured on rc3: NOTE.md addendum 2) ----
// Before the fix, `node ./scripts/check.js` was stored as bare `node`, the card offered `classify --bash "node"`, and afterwards
// `node ./scripts/wipe.js` passed silently. Same for `python` and `bash`.

const BARE_INTERP: [&str; 6] = ["node", "python", "python3", "bash", "sh", "py"];

fn offers_bare_interp(out: &str) -> Option<&'static str> {
    BARE_INTERP.iter().copied().find(|b| out.contains(&format!("--bash \"{b}\"")))
}

// Value: protects=interpreter key = interpreter + script path, `./` dropped and `\` read as `/`; fails_when=`node ./a.js` keys on bare `node`; seam=classify::family_of
#[test]
fn interpreter_key_includes_script() {
    for (cmd, want) in [("node ./scripts/check.js", "node scripts/check.js"), (r"node 'scripts\check.js'", "node scripts/check.js"), ("python ./a.py", "python a.py"),
                        ("python3 tools/x.py --n 2", "python3 tools/x.py"), ("bash ./a.sh", "bash a.sh"), ("sh a.sh", "sh a.sh"), ("node.exe ./a.js", "node a.js")] {
        assert_eq!(named(cmd).as_deref(), Some(want), "{cmd}");
    }
}

// Value: protects=no script path, inline code (-e/-c/-m), a flag before the script, a variable or a glob takes no group answer; fails_when=any of these gets a key that can be stored; seam=classify::family_of
#[test]
fn interpreter_inline_or_no_script_is_unnamed() {
    for cmd in ["node", "node -e 'x'", "python -c 'x'", "python -m pytest", "bash -c 'rm -rf x'", "sh", "node $S", "python -u a.py", "bash *.sh", "node --version"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
}

// Value: protects=one script's answer never covers another script run by the same interpreter, and the same script is not asked again; fails_when=after classifying a.js, wipe.js runs silently (rc3 measured); seam=classify_cli + run_hook
#[test]
fn one_script_answer_does_not_cover_another() {
    for (a, b) in [("node ./scripts/check.js", "node ./scripts/wipe.js"), ("python ./a.py", "python ./b.py"), ("bash ./a.sh", "bash ./b.sh")] {
        let p = tmp("interp_ab");
        classify(&p, a).unwrap();
        assert_eq!(hook(&p, a, "bypassPermissions").code, 0, "{a}");
        assert_eq!(hook(&p, b, "bypassPermissions").code, 2, "{b}");
        assert!(hook(&p, b, "default").stdout.contains("\"ask\""), "{b}");
        assert_eq!(fs::read_dir(p.join("x")).unwrap().count(), 12);
    }
}

// Value: protects=a bare interpreter key is refused with the add-the-script line and nothing is stored; fails_when=classify stores `bash:node`; seam=classify_cli
#[test]
fn classify_refuses_bare_interpreter() {
    let p = tmp("refuse_interp");
    for cmd in ["node", "python", "python -m pytest", "bash -c 'x'", "sh"] {
        let e = classify(&p, cmd).unwrap_err();
        assert!(e.contains("Add the script path: olp-hook classify --bash \"node <script>\" read_only"), "{cmd}: {e}");
    }
    assert!(!map_file(&p).exists());
}

// Value: protects=the block card offers `node <script>`, never a bare interpreter, and old bare entries are never read; fails_when=the card offers `--bash "node"` or an old `bash:node` still covers wipe.js; seam=run_hook card over a pre-existing hook-map.json
#[test]
fn card_offers_script_and_old_bare_interpreter_is_ignored() {
    let p = tmp("interp_card");
    let o = hook(&p, "node ./scripts/wipe.js", "bypassPermissions");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("classify --bash \"node scripts/wipe.js\""), "{}", o.stderr);
    for cmd in ["node ./scripts/wipe.js", "python ./b.py", "bash ./b.sh", "python -m pytest"] {
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(offers_bare_interp(&format!("{}{}", o.stderr, o.stdout)), None, "{cmd}: {}", o.stderr);
    }
    let q = tmp("interp_old_map");
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    let map = json!({"version": 1, "entries": {"bash:node": ro, "bash:python": ro, "bash:bash": ro}}).to_string();
    write_map(&q, &map);
    for cmd in ["node ./scripts/wipe.js", "python ./b.py", "bash ./b.sh"] {
        assert_eq!(hook(&q, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
    assert_eq!(fs::read_to_string(map_file(&q)).unwrap(), map);
}

// ---- Design 1-5 §2-1c (REVIEW_OLP013_RC4_RUNNER correction, blocking 1) ----

const DESIGN_INTERPRETERS: [&str; 19] =
    ["node", "nodejs", "deno", "bun", "tsx", "ts-node", "python", "python3", "py", "ruby", "perl", "php", "bash", "sh", "zsh", "dash", "pwsh", "powershell", "cmd"];

// Value: protects=§2-1c closed list of 18 (name read without .exe/.cmd/.ps1, versioned python3.x): each keys on interpreter + script and never on the bare name; fails_when=`tsx a.ts` keeps the one-word key `tsx`; seam=classify::family_of
#[test]
fn interpreter_list_matches_design() {
    for n in DESIGN_INTERPRETERS {
        assert_eq!(named(&format!("{n} x.js")).as_deref(), Some(format!("{n} x.js").as_str()), "{n}");
        assert_eq!(named(n), None, "{n}");
    }
    for (cmd, want) in [("node.cmd x.js", "node x.js"), ("tsx.cmd a.ts", "tsx a.ts"), ("pwsh.exe a.ps1", "pwsh a.ps1"), ("powershell.ps1 a.ps1", "powershell a.ps1"), ("python3.12 a.py", "python3.12 a.py")] {
        assert_eq!(named(cmd).as_deref(), Some(want), "{cmd}");
    }
}

// Value: protects=§2-1c rule 2: only a relative path inside the project names a script (leading /, drive letter, network path, any .. piece and ~ do not); fails_when=`node ../x.js` is stored as a named key; seam=classify::family_of
#[test]
fn interpreter_path_outside_project_is_unnamed() {
    for cmd in ["node ../x.js", "node /abs/x.js", "node C:/x/a.js", "node ~/x.js", "node sub/../../out.js", r"node '\\srv\x.js'", r"node 'C:\x\a.js'", "python a/../b.py"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
}

fn hook_ps(p: &Path, cmd: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": "PowerShell",
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_r"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn project_with(name: &str, keys: &[&str]) -> PathBuf {
    let p = tmp(name);
    fs::create_dir_all(p.join("sub")).unwrap();
    let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
    let entries: serde_json::Map<String, serde_json::Value> = keys.iter().map(|k| (format!("bash:{k}"), ro.clone())).collect();
    write_map(&p, &json!({"version": 1, "entries": entries}).to_string());
    p
}

// Value: protects=design 1-8 §2-1c rule 4 (closed): a folder move anywhere earlier in the same command (the 8 CD_VERBS, any joiner, a subshell) or a wrapper folder flag (-C, --chdir, --chdir=, -D) takes no group answer, so the root's answer does not cover another file; fails_when=`node a.js` answered at the root lets `(cd x; node a.js)` or `env -Cx node a.js` run silently; seam=run_hook over a pre-existing hook-map.json
#[test]
fn interpreter_after_dir_change_is_unnamed() {
    let p = project_with("interp_cd", &["node a.js", "node scripts/check.js"]);
    assert_eq!(hook(&p, "node a.js", "bypassPermissions").code, 0);
    for cmd in ["cd other && node scripts/check.js", "cd sub && node a.js", "cd x; node a.js", "(cd x; node a.js)", "pushd x && node a.js", "popd; node a.js",
                "env --chdir=x node a.js", "env -Cx node a.js", "env -C x node a.js", "cd sub; node ./a.js",
                "if cd sub; then node a.js; fi", "{ cd sub; }; node a.js", "! cd sub; node a.js"] {
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
    assert_eq!(hook_ps(&p, "node a.js").code, 0);
    for cmd in ["Set-Location other; node scripts/check.js", "sl x; node a.js", "cd..; node a.js"] {
        assert_eq!(hook_ps(&p, cmd).code, 2, "{cmd}");
    }
}

// Value: protects=design 1-8 §2-1c path condition: a word is a path only if it has `/`, `\` or `.` and still has `.` or `/` after normalizing, so `bun ./test` never takes the `bun test` answer and `node server` takes no group answer; fails_when=`bun ./test` normalizes to `bun test` and runs silently; seam=family_of + run_hook
#[test]
fn bun_deno_file_never_shares_subcommand_key() {
    let p = project_with("bun_deno_sub", &["bun test", "deno test", "node"]);
    assert_eq!(hook(&p, "bun test", "bypassPermissions").code, 0);
    assert_eq!(hook(&p, "deno test", "bypassPermissions").code, 0);
    for cmd in ["bun ./test", "deno ./test", "node server"] {
        assert_eq!(named(cmd), None, "{cmd}");
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
}

// Value: protects=design 1-8: normalizing comes first and the inside-project check reads the normalized value, so PowerShell `node \x.js` (leading `/` after normalizing) takes no group answer; fails_when=it keys on `node x.js` or any named key; seam=run_hook (PowerShell)
#[test]
fn interpreter_leading_backslash_is_outside() {
    let p = project_with("lead_bs", &["node x.js"]);
    let o = hook_ps(&p, r"node \x.js");
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(!o.stderr.contains("--bash \"node"), "{}", o.stderr);
}

// Value: protects=§2-1c rule 6: `deno run <path>`, `bun run <file.ext>` and `bun <file>` key on the script, other two-word forms stay as in 0.1.x; fails_when=`deno run ./a.ts` keys on `deno run` and one answer covers every deno script; seam=classify::family_of
#[test]
fn deno_bun_run_path_key() {
    for (cmd, want) in [("deno run ./a.ts", Some("deno run a.ts")), ("bun run ./a.ts", Some("bun run a.ts")), ("bun ./a.ts", Some("bun a.ts")),
                        ("deno run -A a.ts", None), ("deno run ../a.ts", None), ("deno run", None), ("bun run ../a.ts", None),
                        ("deno test", Some("deno test")), ("bun test", Some("bun test")), ("php artisan migrate", None), ("bun run build", Some("bun run"))] {
        assert_eq!(named(cmd).as_deref(), want, "{cmd}");
    }
}

// Value: protects=design 1-9 §2-4b: a runner key whose package is olp or @feedpia/olp, and an interpreter key whose script file is olp, olp.js or olp-hook, take no group answer, so a read answer never covers `npx @feedpia/olp classify ...`; fails_when=`bash:npx @feedpia/olp` read_only lets the classify write sub-command run silently; seam=family_is_name + run_hook + classify_cli
#[test]
fn olp_via_runner_takes_no_group_answer() {
    for cmd in ["npx @feedpia/olp stats", "npx olp stats", "pnpm dlx @feedpia/olp stats", "pnpm exec olp stats", "node node_modules/@feedpia/olp/bin/olp.js stats", "node bin/olp stats"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
    let p = project_with("olp_self", &["npx @feedpia/olp", "npx olp"]);
    for cmd in ["npx @feedpia/olp classify --bash x read_only", "npx @feedpia/olp stats", "npx olp stats"] {
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(o.code, 2, "{cmd}");
        assert!(!o.stderr.contains("--bash \"npx @feedpia/olp\"") && !o.stderr.contains("--bash \"npx olp\""), "{cmd}: {}", o.stderr);
    }
    let q = tmp("olp_self_refuse");
    for typed in ["npx @feedpia/olp", "npx @feedpia/olp stats", "npx olp"] {
        assert!(classify(&q, typed).is_err(), "{typed}");
    }
    assert!(!map_file(&q).exists());
}

fn hook_in(p: &Path, cwd: &Path, tool: &str, cmd: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": cwd.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_r"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

// Value: protects=review RECHECK 1 blocking 1 (design 1-9 §2-1c rule 4): a folder move or wrapper folder flag after a leading syntax word ({, if, then, while, do, !) still means no group answer; fails_when=`{ cd sub; node a.js; }` takes the root's `node a.js` answer; seam=run_hook over a pre-existing hook-map.json
#[test]
fn interpreter_after_lead_word_dir_change_is_unnamed() {
    let p = project_with("lead_cd", &["node a.js"]);
    assert_eq!(hook(&p, "node a.js", "bypassPermissions").code, 0);
    for cmd in ["{ cd sub; node a.js; }", "if cd sub; then node a.js; fi", "! cd sub; node a.js", "while cd sub; do node a.js; done",
                "! env -Cx node a.js", "if true; then env --chdir=x node a.js; fi"] {
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
    assert_eq!(hook_ps(&p, "if ($x) { Set-Location sub }; node a.js").code, 2);
}

// Value: protects=review RECHECK 1 blocking 2 (design 1-9 §2-3, §2-4b, §2-5): a runner key never gets the folder path, so in `sub` the card offers `bun x cowsay` (the same key classify stores) and offers nothing for `bun x @feedpia/olp`; fails_when=the card offers `bun x sub/cowsay`; seam=run_hook card + classify_cli
#[test]
fn runner_key_in_subfolder_has_no_path() {
    let p = tmp("runner_sub");
    fs::create_dir_all(p.join("sub")).unwrap();
    let o = hook_in(&p, &p.join("sub"), "Bash", "bun x cowsay hello");
    assert_eq!(o.code, 2);
    assert!(o.stderr.contains("classify --bash \"bun x cowsay\""), "{}", o.stderr);
    assert!(!o.stderr.contains("sub/"), "{}", o.stderr);
    assert!(classify(&p, "bun x cowsay").unwrap().contains("bash:bun x cowsay "));
    let o = hook_in(&p, &p.join("sub"), "Bash", "bun x @feedpia/olp stats");
    assert!(!o.stderr.contains("--bash \"bun x"), "{}", o.stderr);
}

// Value: protects=second review blocking 2: a wrapper flag that takes a value (`env -C x`, `sudo -D x`, `sudo -u bob`, `env -u VAR`) never turns its value into a key like `x node`, so no card offers it and a stored `x node` answer covers nothing; fails_when=the card offers `classify --bash "x node"` and `env -C x node <any script>` then passes silently; seam=family_of + run_hook card
#[test]
fn wrapper_value_flag_never_names_interpreter() {
    let p = project_with("wrap_value", &["x node", "bob node", "var node", "corepack pnpm"]);
    for cmd in ["env -C x node a.js", "env --chdir x node a.js", "sudo -D x node a.js", "sudo -u bob node a.js", "env -u VAR node x.js", "corepack pnpm dlx cowsay"] {
        assert_eq!(named(cmd), None, "{cmd}");
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(o.code, 2, "{cmd}");
        for k in ["x node", "bob node", "var node", "corepack pnpm"] {
            assert!(!o.stderr.contains(&format!("--bash \"{k}\"")), "{cmd}: {}", o.stderr);
        }
    }
}

// Value: protects=second review blocking 3 (coordinator: in rc4): a runner whose package is one of the interpreters (`npx tsx`, `npx node`, `pnpm exec node`, `bunx ts-node`, `yarn tsx`) takes no group answer, so one answer never covers every script; fails_when=`bash:npx tsx` read_only lets `npx tsx wipe.ts` run silently; seam=family_of + run_hook + classify_cli
#[test]
fn runner_interpreter_package_takes_no_group_answer() {
    for cmd in ["npx tsx a.ts", "npx node a.js", "pnpm exec node a.js", "bunx ts-node a.ts", "yarn tsx a.ts", "yarn node a.js", "bun tsx a.ts", "npx tsx.cmd a.ts"] {
        assert_eq!(named(cmd), None, "{cmd}");
    }
    let p = project_with("runner_interp", &["npx tsx", "yarn tsx"]);
    for cmd in ["npx tsx wipe.ts", "yarn tsx wipe.ts", "npx node a.js"] {
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(o.code, 2, "{cmd}");
        assert!(!o.stderr.contains("--bash \"npx tsx\"") && !o.stderr.contains("--bash \"yarn tsx\""), "{cmd}: {}", o.stderr);
    }
    let q = tmp("runner_interp_refuse");
    for typed in ["npx tsx", "npx node", "yarn tsx"] {
        assert!(classify(&q, typed).is_err(), "{typed}");
    }
}

// Value: protects=design 1-11 §2-4d closed grammar: an interpreter or runner key gets a name only when every word before the program is understood (wrapper in the table, or exactly `sudo -n`/`-E`/`-H`, `env -i`); unknown, bundled or shortened flags, any leading `NAME=value`, or a wrapper outside the table take no group answer; since rc4 rev 15 (simple form only) even an understood wrapper (`sudo -n`, `env -i`, `sudo`) asks; fails_when=`NODE_OPTIONS=--require=./w.js node a.js`, `env -vCx node a.js` or `sudo node a.js` runs under the stored `node a.js` answer; seam=run_hook over a pre-existing hook-map.json
#[test]
fn closed_grammar_unknown_shape_is_unnamed() {
    let p = project_with("closed_grammar", &["node a.js", "npx cowsay", "bash s.sh", "python a.py"]);
    for cmd in ["env -vCx node a.js", "env --ch=x node a.js", "sudo -nDx node a.js", "sudo -nE node a.js", "NODE_OPTIONS=--require=./w.js node a.js",
                "BASH_ENV=x bash s.sh", "PYTHONPATH=x python a.py", "NODE_ENV=test node a.js", "env FOO=1 node a.js", "CI=1 npx cowsay", "frobnicate node a.js"] {
        let o = hook(&p, cmd, "bypassPermissions");
        assert_eq!(o.code, 2, "{cmd}");
        for k in ["node a.js", "npx cowsay", "bash s.sh", "python a.py"] {
            assert!(!o.stderr.contains(&format!("--bash \"{k}\"")), "{cmd}: {}", o.stderr);
        }
    }
    // The key is still worked out the same way, but rc4 rev 15 (simple form only) means a wrapper in front never uses the stored
    // answer: the call asks (exit 2 in bypass mode) even with `node a.js` stored.
    for cmd in ["sudo -n node a.js", "env -i node a.js", "sudo node a.js"] {
        assert_eq!(named(cmd).as_deref(), Some("node a.js"), "{cmd}");
        assert_ne!(hook(&p, cmd, "bypassPermissions").code, 0, "{cmd}: a wrapper is not a simple form, so it asks (rc4 rev 15)");
    }
    // Design 1-12 §2-4d ⑤: an earlier command on the same line must be a proven read from the closed table.
    for cmd in ["export NODE_OPTIONS=--require=./w.js; node a.js", "export NODE_OPTIONS=--require=./w.js && node a.js", "NODE_OPTIONS=--require=./w.js; node a.js",
                "declare -x X=1; node a.js", "source ./e.sh && node a.js", ". ./env.sh; node a.js", "cat x | node a.js", "npm run build && node a.js",
                "git -c x=y status && node a.js"] {
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
    for cmd in ["$env:NODE_OPTIONS='--require=./w.js'; node a.js", "Set-Item env:NODE_OPTIONS x; node a.js"] {
        assert_eq!(hook_ps(&p, cmd).code, 2, "{cmd}");
    }
    // Ledger 1321 ① replaces the §2-4d ⑤ exception: a bundle never uses a remembered answer, even after a proven read.
    for cmd in ["git status && node a.js", "echo hi; node a.js"] {
        assert_eq!(hook(&p, cmd, "bypassPermissions").code, 2, "{cmd}");
    }
}
