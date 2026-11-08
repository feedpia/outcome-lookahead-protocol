//! Tests for `olp-hook install` / `uninstall`.
//! All file operations happen only in temporary folders under CARGO_TARGET_TMPDIR (project, home and binary locations are all fake).
// Value: protects=leaves other entries, key order and values in the user's settings.json untouched and only adds/removes our two hook entries; leaves a broken file byte-for-byte unchanged; copies the binary to a fixed location byte-for-byte; fails_when=other entries are deleted or reordered, running twice yields 4 entries, broken JSON is overwritten, a preview changes the file, or a backup is created inside the repo; why_new=one-line npx install; seam=install::add_entries, remove_entries, run; no_claim=does not stop package substitution (supply chain); only copy integrity.
use olp_hook::install::{add_entries, is_our_command, remove_entries, run, InstallEnv};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn tmp(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_install_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    fs::create_dir_all(&p).unwrap();
    p
}

const CMD: &str = "C:/Users/me/olp/olp-hook.exe";

/// Same value as the install guide's reference snippet (only the path differs).
/// Only the pre-execution (PreToolUse) entry gets `onFailure: block`: if the hook fails or times out, Claude Code blocks the call (per the primary docs). The post-execution entry has nothing to block and stays as is.
fn guide_1c_a(cmd: &str) -> Value {
    let pre = json!({ "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": cmd, "timeout": 10, "onFailure": "block" } ] });
    let post = json!({ "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": cmd, "timeout": 10 } ] });
    json!({ "hooks": { "PreToolUse": [pre], "PostToolUse": [post] } })
}

/// When install runs again over an older install (an entry without onFailure, e.g. a hand-made early install), it adds onFailure to the pre-execution entry, even with the same path.
#[test]
fn install_adds_on_failure_block_to_old_pre_entry() {
    let old_entry = json!({ "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": CMD, "timeout": 10 } ] });
    let old = json!({ "hooks": { "PreToolUse": [old_entry.clone()], "PostToolUse": [old_entry] } }).to_string();
    let c = add_entries(Some(&old), CMD, false).unwrap();
    assert!(c.changed, "an old entry gets updated");
    assert_eq!(parse(&c.text), guide_1c_a(CMD));
    assert!(c.lines.iter().any(|l| l.contains("PreToolUse") && l.contains("onFailure")), "{:?}", c.lines);
    let again = add_entries(Some(&c.text), CMD, false).unwrap();
    assert!(!again.changed, "second run is a no-op");
}

#[test]
fn replace_keeps_on_failure_block() {
    let old = add_entries(None, "D:/old/olp-hook.exe", false).unwrap();
    let r = add_entries(Some(&old.text), CMD, true).unwrap();
    assert_eq!(parse(&r.text)["hooks"]["PreToolUse"][0]["hooks"][0]["onFailure"], "block");
}

fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap()
}

/// Whether the parts first appear in this order within the text (serde_json::Value reorders keys, so order is checked on the text).
fn in_order(text: &str, parts: &[&str]) {
    let at: Vec<usize> = parts.iter().map(|p| text.find(p).unwrap_or_else(|| panic!("{p} not found:\n{text}"))).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "order {parts:?} -> {at:?}:\n{text}");
}

/// Original in the shape Claude Code writes (multi-line, 2-space indent): permissions plus someone else's Edit hook. Key order is z then a (reverse alphabetical, so any reordering is easy to spot).
const PRETTY_OTHER: &str = "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls:*)\"\n    ]\n  },\n  \"hooks\": {\n    \"PreToolUse\": [\n      {\n        \"matcher\": \"Edit\",\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"my-check\"\n          }\n        ]\n      }\n    ]\n  },\n  \"model\": \"opus\"\n}\n";

#[test]
fn missing_file_gets_guide_1c_a_values() {
    let c = add_entries(None, CMD, false).unwrap();
    assert!(c.changed);
    assert_eq!(parse(&c.text), guide_1c_a(CMD));
    in_order(&c.text, &["\"PreToolUse\"", "\"PostToolUse\""]);
    in_order(&c.text, &["\"matcher\"", "\"hooks\": [", "\"type\"", "\"command\"", "\"timeout\""]);
    assert!(c.text.ends_with("}\n"));
}

#[test]
fn empty_object_gets_both_entries() {
    let c = add_entries(Some("{}"), CMD, false).unwrap();
    assert_eq!(parse(&c.text), guide_1c_a(CMD));
}

#[test]
fn other_hook_and_key_order_are_kept() {
    let c = add_entries(Some(PRETTY_OTHER), CMD, false).unwrap();
    let v = parse(&c.text);
    in_order(&c.text, &["\n  \"permissions\"", "\n  \"hooks\"", "\n  \"model\""]);
    let pre = v["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre.len(), 2);
    assert_eq!(pre[0]["matcher"], "Edit", "the other entry stays first, unchanged");
    assert_eq!(pre[1], guide_1c_a(CMD)["hooks"]["PreToolUse"][0]);
    assert_eq!(v["hooks"]["PostToolUse"], guide_1c_a(CMD)["hooks"]["PostToolUse"]);
    assert_eq!(v["permissions"], parse(PRETTY_OTHER)["permissions"]);
}

#[test]
fn pretty_original_rest_bytes_unchanged_after_install_then_uninstall() {
    let added = add_entries(Some(PRETTY_OTHER), CMD, false).unwrap();
    assert!(added.lines.iter().all(|l| !l.contains("layout")), "a 2-space original produces no layout notice: {:?}", added.lines);
    // Bytes outside our entries equal the original: removing only our entries from the added text gives back the original byte for byte.
    let removed = remove_entries(&added.text).unwrap();
    assert!(removed.changed);
    let expected = PRETTY_OTHER;
    let with_empty_post = parse(&removed.text);
    assert_eq!(with_empty_post["hooks"]["PostToolUse"], json!([]), "an empty list is kept");
    let mut back = with_empty_post.clone();
    back["hooks"].as_object_mut().unwrap().remove("PostToolUse");
    assert_eq!(back, parse(expected));
    // When the original already had PostToolUse, the result must match byte for byte
    let with_post = PRETTY_OTHER.replace("    ]\n  },\n  \"model\"", "    ],\n    \"PostToolUse\": []\n  },\n  \"model\"");
    let a2 = add_entries(Some(&with_post), CMD, false).unwrap();
    let r2 = remove_entries(&a2.text).unwrap();
    assert_eq!(r2.text, with_post, "bytes outside our entries equal the original");
}

#[test]
fn one_line_original_keeps_values_and_says_layout_changes() {
    let one = r#"{"model":"opus","hooks":{"PreToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"my-check"}]}]}}"#;
    let c = add_entries(Some(one), CMD, false).unwrap();
    let v = parse(&c.text);
    in_order(&c.text, &["\n  \"model\"", "\n  \"hooks\""]);
    assert_eq!(v["hooks"]["PreToolUse"][0], parse(one)["hooks"]["PreToolUse"][0]);
    assert!(c.lines.iter().any(|l| l.contains("layout")), "layout change notice: {:?}", c.lines);
}

#[test]
fn running_twice_adds_nothing() {
    let first = add_entries(None, CMD, false).unwrap();
    let second = add_entries(Some(&first.text), CMD, false).unwrap();
    assert!(!second.changed);
    assert_eq!(second.text, first.text);
}

#[test]
fn our_entry_at_other_path_is_left_without_replace() {
    let old = add_entries(None, "D:/old/olp-hook.exe", false).unwrap();
    let c = add_entries(Some(&old.text), CMD, false).unwrap();
    assert!(!c.changed);
    assert_eq!(c.text, old.text);
    assert!(c.lines.iter().any(|l| l.contains("--replace") && l.contains("D:/old/olp-hook.exe")), "{:?}", c.lines);
    let r = add_entries(Some(&old.text), CMD, true).unwrap();
    assert!(r.changed);
    assert_eq!(parse(&r.text), guide_1c_a(CMD), "--replace swaps our entries for new ones, entry count unchanged");
}

/// Our hook inside the same item as someone else's hook: on --replace only our hook entry changes; the other hook and the matcher stay.
#[test]
fn replace_inside_a_shared_entry_changes_only_our_hook() {
    let shared = json!({ "hooks": {
        "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "my-check" }, { "type": "command", "command": "D:/old/olp-hook.exe", "timeout": 10 } ] } ],
        "PostToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "D:/old/olp-hook.exe", "timeout": 10 } ] } ]
    } })
    .to_string();
    let r = add_entries(Some(&shared), CMD, true).unwrap();
    let v = parse(&r.text);
    assert_eq!(
        v["hooks"]["PreToolUse"],
        json!([ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "my-check" }, { "type": "command", "command": CMD, "timeout": 10, "onFailure": "block" } ] } ]),
        "same item: only our hook entry changes (pre-execution = onFailure block), matcher and the other hook unchanged"
    );
    assert_eq!(v["hooks"]["PostToolUse"], guide_1c_a(CMD)["hooks"]["PostToolUse"], "item with only our hook: the whole item is replaced");
}

#[test]
fn our_command_is_found_by_file_name() {
    for c in ["olp-hook", "olp-hook.exe", "OLP-HOOK.EXE", "C:/Users/me/olp/olp-hook.exe", "C:\\Users\\me\\olp\\Olp-Hook.exe", "\"C:/Users/John Smith/olp/olp-hook.exe\"", "/home/me/olp/olp-hook"] {
        assert!(is_our_command(c), "{c}");
    }
    for c in ["my-check", "olp-gui.exe", "npx olp-hook-other", "C:/x/olp-hook.exe.bak", "olp-hooks"] {
        assert!(!is_our_command(c), "{c}");
    }
    let upper = add_entries(None, "C:/X/OLP-HOOK.EXE", false).unwrap();
    assert!(!add_entries(Some(&upper.text), "C:/X/OLP-HOOK.EXE", false).unwrap().changed);
}

#[test]
fn broken_json_or_wrong_shape_is_refused() {
    assert!(add_entries(Some("{\"hooks\": "), CMD, false).is_err());
    assert!(add_entries(Some("[1,2]"), CMD, false).is_err());
    assert!(add_entries(Some(r#"{"hooks": 3}"#), CMD, false).is_err());
    assert!(add_entries(Some(r#"{"hooks": {"PreToolUse": {}}}"#), CMD, false).is_err());
    assert!(remove_entries("{oops").is_err());
}

#[test]
fn uninstall_removes_only_our_hook_objects() {
    let mixed = json!({ "hooks": { "PreToolUse": [
        { "matcher": "Bash", "hooks": [ { "type": "command", "command": "my-check" }, { "type": "command", "command": CMD, "timeout": 10 } ] },
        { "matcher": "Bash|PowerShell|mcp__.*", "hooks": [ { "type": "command", "command": CMD, "timeout": 10 } ] }
    ] } })
    .to_string();
    let r = remove_entries(&mixed).unwrap();
    let pre = parse(&r.text)["hooks"]["PreToolUse"].clone();
    assert_eq!(pre, json!([ { "matcher": "Bash", "hooks": [ { "type": "command", "command": "my-check" } ] } ]));
    let none = remove_entries(PRETTY_OTHER).unwrap();
    assert!(!none.changed);
    assert_eq!(none.text, PRETTY_OTHER);
}

// --- File side (run) ---

struct World {
    env: InstallEnv,
    project: PathBuf,
    home: PathBuf,
    src: PathBuf,
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn world(name: &str, home_name: &str) -> World {
    let root = tmp(name);
    let project = root.join("proj");
    let home = root.join(home_name);
    let src = root.join("pkg");
    for d in [&project, &home, &src] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(src.join(exe("olp-hook")), b"hook-bytes-1").unwrap();
    fs::write(src.join(exe("olp-gui")), b"gui-bytes-1").unwrap();
    let env = InstallEnv { project: project.clone(), home: home.clone(), exe: src.join(exe("olp-hook")), now: 1_700_000_000 };
    World { env, project, home, src }
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn settings(p: &Path) -> PathBuf {
    p.join(".claude").join("settings.json")
}

fn installed_cmd(home: &Path) -> String {
    let s = home.join("olp").join(exe("olp-hook")).to_string_lossy().replace('\\', "/");
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s
    }
}

#[test]
fn install_copies_both_programs_and_writes_project_settings() {
    let w = world("install_basic", "home");
    let out = run("install", &args(&[]), &w.env).unwrap();
    assert_eq!(fs::read(w.home.join("olp").join(exe("olp-hook"))).unwrap(), b"hook-bytes-1");
    assert_eq!(fs::read(w.home.join("olp").join(exe("olp-gui"))).unwrap(), b"gui-bytes-1");
    let v = parse(&fs::read_to_string(settings(&w.project)).unwrap());
    assert_eq!(v, guide_1c_a(&installed_cmd(&w.home)));
    assert!(out.contains("uninstall"), "one line explaining how to undo: {out}");
    // Second run = nothing changed
    let before = fs::read(settings(&w.project)).unwrap();
    run("install", &args(&[]), &w.env).unwrap();
    assert_eq!(fs::read(settings(&w.project)).unwrap(), before);
}

#[test]
fn backup_goes_outside_the_project() {
    let w = world("install_backup", "home");
    fs::create_dir_all(w.project.join(".claude")).unwrap();
    fs::write(settings(&w.project), PRETTY_OTHER).unwrap();
    run("install", &args(&[]), &w.env).unwrap();
    let backups: Vec<_> = fs::read_dir(w.home.join("olp").join("backup")).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read_to_string(&backups[0]).unwrap(), PRETTY_OTHER);
    let in_project: Vec<_> = fs::read_dir(w.project.join(".claude")).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(in_project, ["settings.json"], "no backup or temp files inside the project");
}

#[test]
fn dry_run_touches_nothing() {
    let w = world("install_dry", "home");
    fs::create_dir_all(w.project.join(".claude")).unwrap();
    fs::write(settings(&w.project), PRETTY_OTHER).unwrap();
    let out = run("install", &args(&["--dry-run"]), &w.env).unwrap();
    assert!(out.contains("PreToolUse"), "{out}");
    assert_eq!(fs::read_to_string(settings(&w.project)).unwrap(), PRETTY_OTHER);
    assert!(!w.home.join("olp").exists(), "no copy or backup");
    let u = run("uninstall", &args(&["--dry-run"]), &w.env).unwrap();
    assert!(!u.is_empty());
    assert_eq!(fs::read_to_string(settings(&w.project)).unwrap(), PRETTY_OTHER);
}

#[test]
fn broken_settings_file_is_left_byte_for_byte() {
    let w = world("install_broken", "home");
    fs::create_dir_all(w.project.join(".claude")).unwrap();
    let broken = "{ \"hooks\": { \"PreToolUse\": [ ,, ] }\n";
    fs::write(settings(&w.project), broken).unwrap();
    let e = run("install", &args(&[]), &w.env).unwrap_err();
    assert!(e.contains("nothing changed"), "{e}");
    assert_eq!(fs::read_to_string(settings(&w.project)).unwrap(), broken);
    assert!(!w.home.join("olp").exists(), "no copy when the file is broken");
}

/// If writing settings fails after the binary was copied, the output says plainly that the copy happened and the hooks were not added.
#[test]
fn settings_write_failure_says_programs_were_copied_but_hook_not_added() {
    let w = world("install_writefail", "home");
    // Put a directory where the temp file would go to make the write fail (the settings file itself does not exist)
    fs::create_dir_all(w.project.join(".claude").join("settings.json.olp-tmp")).unwrap();
    let e = run("install", &args(&[]), &w.env).unwrap_err();
    assert!(e.contains("copied") && e.contains("not added"), "{e}");
    assert!(e.contains(&*w.home.join("olp").to_string_lossy()), "copied location: {e}");
    assert!(w.home.join("olp").join(exe("olp-hook")).exists());
    assert!(!settings(&w.project).exists());
}

#[test]
fn uninstall_restores_the_original_and_keeps_programs() {
    let w = world("install_undo", "home");
    fs::create_dir_all(w.project.join(".claude")).unwrap();
    let with_post = PRETTY_OTHER.replace("    ]\n  },\n  \"model\"", "    ],\n    \"PostToolUse\": []\n  },\n  \"model\"");
    fs::write(settings(&w.project), &with_post).unwrap();
    run("install", &args(&[]), &w.env).unwrap();
    let out = run("uninstall", &args(&[]), &w.env).unwrap();
    assert_eq!(fs::read_to_string(settings(&w.project)).unwrap(), with_post);
    assert!(w.home.join("olp").join(exe("olp-hook")).exists(), "the binary is not deleted");
    assert!(out.contains(&*w.home.join("olp").to_string_lossy()), "reports the folder left behind: {out}");
}

#[test]
fn user_flag_writes_user_settings() {
    let w = world("install_user", "home");
    run("install", &args(&["--user"]), &w.env).unwrap();
    assert!(settings(&w.home).exists());
    assert!(!settings(&w.project).exists());
}

#[test]
fn home_with_space_gets_quoted_command() {
    let w = world("install_space", "John Smith");
    run("install", &args(&[]), &w.env).unwrap();
    let cmd = parse(&fs::read_to_string(settings(&w.project)).unwrap())["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_string();
    assert!(cmd.starts_with('"') && cmd.ends_with('"') && cmd.contains("John Smith"), "{cmd}");
    assert!(is_our_command(&cmd));
}

#[test]
fn missing_gui_is_skipped_quietly_and_unknown_args_refused() {
    // The bundle has no olp-gui, so the install text has no olp-gui line (no "not found" shown every time)
    let w = world("install_nogui", "home");
    fs::remove_file(w.src.join(exe("olp-gui"))).unwrap();
    let out = run("install", &args(&[]), &w.env).unwrap();
    assert!(!out.contains("olp-gui"), "{out}");
    assert!(w.home.join("olp").join(exe("olp-hook")).exists());
    assert!(run("install", &args(&["--force"]), &w.env).is_err());
    assert!(run("uninstall", &args(&["--replace"]), &w.env).is_err());
}

#[test]
fn already_in_place_program_is_not_copied_onto_itself() {
    let w = world("install_inplace", "home");
    let dir = w.home.join("olp");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(exe("olp-hook")), b"same").unwrap();
    let env = InstallEnv { exe: dir.join(exe("olp-hook")), ..w.env };
    run("install", &args(&[]), &env).unwrap();
    assert_eq!(fs::read(dir.join(exe("olp-hook"))).unwrap(), b"same");
}

// --- Running install from the home folder without --user means user-global settings -> no change, refused ---

/// Current folder = home (a trailing separator makes the text differ; on Windows also upper case; comparison uses same_file).
fn home_as_project(w: &World) -> InstallEnv {
    let mut p = format!("{}{}", w.home.display(), std::path::MAIN_SEPARATOR);
    if cfg!(windows) {
        p = p.to_uppercase();
    }
    at(w, PathBuf::from(p))
}

fn at(w: &World, project: PathBuf) -> InstallEnv {
    InstallEnv { project, home: w.home.clone(), exe: w.env.exe.clone(), now: w.env.now }
}

#[test]
fn install_in_home_without_user_refuses_and_writes_nothing() {
    let w = world("home_refuse", "home");
    fs::create_dir_all(w.home.join(".claude")).unwrap();
    fs::write(settings(&w.home), PRETTY_OTHER).unwrap();
    let e = run("install", &args(&[]), &home_as_project(&w)).unwrap_err();
    assert_eq!(e, "this folder is your home folder — a hook here applies to every project. Run install inside a project folder, or add --user if you want that.");
    assert!(!e.contains("What OLP does now") && !e.contains("Send anonymous"), "no closing text or question after the refusal: {e}");
    assert_eq!(fs::read_to_string(settings(&w.home)).unwrap(), PRETTY_OTHER, "settings bytes unchanged");
    assert!(!w.home.join("olp").exists(), "no copy or backup");
}

#[test]
fn install_in_home_with_user_still_installs() {
    let w = world("home_user", "home");
    run("install", &args(&["--user"]), &home_as_project(&w)).unwrap();
    assert!(settings(&w.home).exists());
}

#[test]
fn install_in_subfolder_of_home_unchanged() {
    let w = world("home_sub", "home");
    let sub = w.home.join("proj");
    fs::create_dir_all(&sub).unwrap();
    run("install", &args(&[]), &at(&w, sub.clone())).unwrap();
    assert!(settings(&sub).exists());
    assert!(!settings(&w.home).exists());
}

#[test]
fn uninstall_in_home_still_removes() {
    let w = world("home_undo", "home");
    fs::create_dir_all(w.home.join(".claude")).unwrap();
    fs::write(settings(&w.home), PRETTY_OTHER).unwrap();
    run("install", &args(&["--user"]), &w.env).unwrap();
    run("uninstall", &args(&[]), &home_as_project(&w)).unwrap();
    let after = fs::read_to_string(settings(&w.home)).unwrap();
    assert!(!after.contains("olp-hook") && after.contains("my-check"), "a way to remove a mistaken install (other entries untouched): {after}");
}

#[test]
fn dry_run_in_home_refuses_too() {
    let w = world("home_dry", "home");
    let e = run("install", &args(&["--dry-run"]), &home_as_project(&w)).unwrap_err();
    assert!(e.contains("--user"), "{e}");
    assert!(!settings(&w.home).exists() && !w.home.join("olp").exists());
}

// Value: protects=installing from a renamed binary (e.g. olp-hook-windows-x64.exe) never produces settings pointing at a missing file (so no project shell command is blocked by onFailure block);
// fails_when=settings are written or exit is 0 although the olp-hook to register exists neither at the source nor at the install location; why_new=seen with a renamed release file; seam=install::run; no_claim=release bundle naming (belongs to e2e tests).
#[test]
fn install_with_a_renamed_program_changes_nothing_and_fails() {
    let w = world("renamed", "home");
    fs::remove_file(w.src.join(exe("olp-hook"))).unwrap();
    let renamed = w.src.join(exe("olp-hook-windows-x64"));
    fs::write(&renamed, b"hook-bytes-1").unwrap();
    let env = InstallEnv { exe: renamed, ..w.env };
    let r = run("install", &[], &env);
    let e = r.expect_err("a renamed binary must fail (not exit 0)");
    assert!(e.contains(&exe("olp-hook")) && e.contains("nothing changed"), "one-line reason: {e}");
    assert!(!settings(&w.project).exists(), "settings must not be written");
    assert!(!w.home.join("olp").join(exe("olp-hook")).exists(), "must not register a missing file");
}

/// If olp-hook is already installed in ~/olp (installed earlier under the right name), the registration target exists, so install succeeds.
#[test]
fn install_with_a_renamed_program_is_fine_when_already_installed() {
    let w = world("renamed_ok", "home");
    fs::create_dir_all(w.home.join("olp")).unwrap();
    fs::write(w.home.join("olp").join(exe("olp-hook")), b"hook-bytes-0").unwrap();
    fs::remove_file(w.src.join(exe("olp-hook"))).unwrap();
    let renamed = w.src.join(exe("olp-hook-windows-x64"));
    fs::write(&renamed, b"hook-bytes-1").unwrap();
    let env = InstallEnv { exe: renamed, ..w.env };
    run("install", &[], &env).expect("already present at the install location = succeeds");
    assert!(fs::read_to_string(settings(&w.project)).unwrap().contains(&installed_cmd(&w.home)));
}
