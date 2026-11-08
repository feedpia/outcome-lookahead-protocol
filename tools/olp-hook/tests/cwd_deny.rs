//! Cases where Claude Code shows its own critical-path dialog
//! (delete target = the working folder itself or a folder above it) get deny + card + guidance instead of ask; everything else is unchanged.
// Value: protects=the hole where the built-in dialog drops the OLP card and a single Yes deletes; the hook stops it first and shows the name;
//   fails_when=blocks a subfolder inside the working folder, paths outside it, or PowerShell, taking away the user's chance to approve; path-normalisation failure falls through to allow;
//   why_new=working-folder-root coverage; seam=run_hook (public entry); no_claim=how Claude Code renders the deny reason; when the folder is unknown the command is compared against every project ancestor name, so a same-named subfolder may be over-denied (fails closed).
use olp_hook::{run_hook, HookOutcome};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

/// <tmp>/olp_cwd_deny/<name>/proj (+ 3 files under proj/build, other/x.txt beside it). Returns proj.
fn proj(name: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_cwd_deny").join(format!("{name}_{}_{n}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let p = root.join("proj");
    fs::create_dir_all(p.join("build")).unwrap();
    for f in ["app.log", "cache.tmp", "notes.txt"] {
        fs::write(p.join("build").join(f), "x").unwrap();
    }
    fs::create_dir_all(root.join("other")).unwrap();
    fs::write(root.join("other").join("x.txt"), "x").unwrap();
    p
}

/// Path form used in command lines: forward slashes (also `D:/…` on Windows), no trailing slash.
fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn run(p: &Path, tool: &str, cmd: &str, mode: &str) -> HookOutcome {
    let v = json!({
        "session_id": "s", "transcript_path": "t", "cwd": p.to_string_lossy(),
        "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": cmd},
        "permission_mode": mode, "tool_use_id": "toolu_cwd",
    });
    run_hook(v.to_string().as_bytes(), Some(p))
}

fn decision(o: &HookOutcome) -> (String, String) {
    let v: Value = serde_json::from_str(o.stdout.trim()).unwrap_or(Value::Null);
    let h = &v["hookSpecificOutput"];
    (h["permissionDecision"].as_str().unwrap_or("").to_string(), h["permissionDecisionReason"].as_str().unwrap_or("").to_string())
}

// Deliberately duplicates card::CWD_GUIDE: this test pins the exact text and fails if the constant changes.
const GUIDE: &str = "Claude Code shows its own warning first for this shape, so OLP cannot ask here. Run it with a relative path (for example `rm -rf build`) or from your own terminal.";

fn assert_deny(p: &Path, cmd: &str) {
    let o = run(p, "Bash", cmd, "default");
    assert_eq!(o.code, 0, "{cmd}: exit 0 + deny JSON");
    let (d, reason) = decision(&o);
    assert_eq!(d, "deny", "{cmd}: working folder or above = deny\n{}", o.stdout);
    assert!(reason.starts_with("[OLP] Before this runs: delete files"), "{cmd}: card text unchanged (marker + what it does): {reason}");
    assert!(reason.contains("before engine check · no receipt"), "{cmd}: card marker unchanged: {reason}");
    assert!(reason.contains(GUIDE), "{cmd}: one-line guidance: {reason}");
    assert!(!reason.contains('\n'), "{cmd}: reason is one line: {reason:?}");
    assert!(p.join("build/notes.txt").exists(), "the hook deletes nothing");
}

fn assert_ask_unchanged(p: &Path, tool: &str, cmd: &str) {
    let o = run(p, tool, cmd, "default");
    let (d, reason) = decision(&o);
    assert_eq!(d, "ask", "{cmd}: stays ask\n{}", o.stdout);
    assert!(!reason.contains("so OLP cannot ask here"), "{cmd}: no guidance: {reason}");
}

// 1. absolute path = the working folder itself
#[test]
fn absolute_path_to_the_working_folder_is_denied() {
    let p = proj("abs_cwd");
    assert_deny(&p, &format!("rm -rf {}", slash(&p)));
    assert_deny(&p, &format!("rm -rf {}/", slash(&p)));
    assert_deny(&p, &format!("rm -rf \"{}\"", slash(&p)));
}

// 2. absolute path = a folder above the working folder
#[test]
fn absolute_path_to_a_parent_of_the_working_folder_is_denied() {
    let p = proj("abs_parent");
    assert_deny(&p, &format!("rm -rf {}", slash(p.parent().unwrap())));
    assert_deny(&p, &format!("rm -rf {}/proj/..", slash(p.parent().unwrap())));
}

// 3. `.`, `..`, `$PWD` forms
#[test]
fn dot_dotdot_and_pwd_forms_are_denied() {
    let p = proj("dots");
    for cmd in ["rm -rf .", "rm -rf ./", "rm -rf ..", "rm -rf ../", "rm -rf \"$PWD\"", "rm -rf $PWD"] {
        assert_deny(&p, cmd);
    }
}

// Other verbs, forms and wrappers follow the same rule
#[test]
fn other_spellings_of_the_same_target_are_denied() {
    let p = proj("spell");
    for cmd in [
        "rmdir .".to_string(),
        "rm -rf ${PWD}".to_string(),
        "rm -rf -- .".to_string(),
        "sudo rm -rf .".to_string(),
        "rm -rf ../proj".to_string(),
        "cd build && rm -rf ..".to_string(),
        "cd build && rm -rf ../..".to_string(),
        "rm -rf ~nobody_olp".to_string(), // unknown whose home = fail closed
    ] {
        assert_deny(&p, &cmd);
    }
}

// With a leading `cd` and `;` (not `&&`) the folder is unknown: relative, `.`, `..`, `$PWD` targets are denied
#[test]
fn unknown_folder_after_cd_with_relative_target_is_denied() {
    let p = proj("cd_unknown");
    for cmd in ["cd .. ; rm -rf proj", "cd ..; rm -rf proj", "cd ..; rm -rf .", "cd /tmp; rm -rf $PWD", "cd ..; rm -rf ../x"] {
        assert_deny(&p, cmd);
    }
    // `;` chain without `cd` = folder unchanged, so the normal rule applies (subfolder = ask)
    assert_ask_unchanged(&p, "Bash", "echo hi; rm -rf build");
    // Field command (explore_reads DELETES): even with an unknown folder, `build` is not the name of the working folder or an ancestor -> ask (avoid over-blocking)
    assert_ask_unchanged(&p, "Bash", "cd sub && git ls-files build; rm -rf build && ls");
    // Plain dot-prefixed subfolder names (.cache, .next) and `./`-prefixed subfolders (./.cache, ./build) are not `.`, `..` or `../…` as a whole path -> ask
    for cmd in ["cd sub; rm -rf .cache", "cd sub; rm -rf .next", "cd sub; rm -rf .next/cache", "cd sub; rm -rf ./.cache", "cd sub; rm -rf ./build", "cd sub; rm -rf ././build"] {
        assert_ask_unchanged(&p, "Bash", cmd);
    }
}

// The baseline is not only the hook cwd but also the project folder (where settings.json lives) and above: deleting the project is blocked even when the shell is outside (above) the project
#[test]
fn project_folder_is_denied_even_when_the_shell_is_elsewhere() {
    let p = proj("projdir");
    let parent = p.parent().unwrap().to_path_buf();
    let run_from = |cwd: &Path, cmd: &str| {
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": cwd.to_string_lossy(), "hook_event_name": "PreToolUse",
            "tool_name": "Bash", "tool_input": {"command": cmd}, "permission_mode": "default", "tool_use_id": "toolu_proj"});
        decision(&run_hook(v.to_string().as_bytes(), Some(&p))).0
    };
    assert_eq!(run_from(&parent, &format!("rm -rf {}", slash(&p))), "deny", "shell = parent folder, target = project");
    assert_eq!(run_from(&parent, "rm -rf proj"), "deny", "shell = parent folder, relative name = project");
    assert_eq!(run_from(&parent, &format!("rm -rf {}/other", slash(&parent))), "ask", "sibling folder outside the project = unchanged");
    assert_eq!(run_from(&parent, &format!("rm -rf {}/build", slash(&p))), "ask", "subfolder inside the project = unchanged");
}

// `cd <project> && rm -rf build` = deleting a subfolder -> ask (normal card).
// Even if the hook cwd is inside build after earlier commands (shell state), the `cd` on the same line sets the folder, so build is not the working folder. Same for `;` chains (subfolder name).
#[test]
fn cd_to_the_project_then_delete_a_subfolder_still_asks() {
    let p = proj("cdproj");
    let s = slash(&p);
    let run_from = |cwd: &Path, cmd: &str| {
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": cwd.to_string_lossy(), "hook_event_name": "PreToolUse",
            "tool_name": "Bash", "tool_input": {"command": cmd}, "permission_mode": "default", "tool_use_id": "toolu_cdp"});
        decision(&run_hook(v.to_string().as_bytes(), Some(&p))).0
    };
    let mut spell = vec![s.clone(), format!("\"{s}\""), format!("{s}/")];
    if cfg!(windows) {
        let (d, rest) = s.split_at(1);
        spell.push(format!("/{}{}", d.to_lowercase(), &rest[1..])); // Git Bash /c/…
        spell.push(format!("'{}'", p.to_string_lossy())); // C:\…
        spell.push(format!("{}{rest}", d.to_lowercase())); // c:/…
    }
    for cwd in [p.clone(), p.join("build")] {
        for t in &spell {
            assert_eq!(run_from(&cwd, &format!("cd {t} && rm -rf build && ls -la && git status --short")), "ask", "cwd={} cd {t} &&", cwd.display());
            assert_eq!(run_from(&cwd, &format!("cd {t}; rm -rf build")), "ask", "cwd={} cd {t};", cwd.display());
        }
    }
    // Going to a parent folder with `cd` on the same line and then deleting the project = blocked (observed: the built-in dialog appears)
    assert_eq!(run_from(&p, &format!("cd {} && rm -rf proj", slash(p.parent().unwrap()))), "deny");
}

// macOS `/tmp` -> `/private/tmp`: the hook cwd is the link text and `cd` the real path (or vice versa); deleting a subfolder is still ask
#[cfg(unix)]
#[test]
fn symlinked_spelling_of_the_project_still_asks_for_a_subfolder() {
    let p = proj("maclink");
    let link = p.parent().unwrap().join("link_proj");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    let run_from = |cwd: &Path, cmd: &str| {
        let v = json!({"session_id": "s", "transcript_path": "t", "cwd": cwd.to_string_lossy(), "hook_event_name": "PreToolUse",
            "tool_name": "Bash", "tool_input": {"command": cmd}, "permission_mode": "default", "tool_use_id": "toolu_ml"});
        decision(&run_hook(v.to_string().as_bytes(), Some(&p))).0
    };
    assert_eq!(run_from(&link, &format!("cd {} && rm -rf build", slash(&p))), "ask");
    assert_eq!(run_from(&p, &format!("cd {} && rm -rf build", slash(&link))), "ask");
}

// Root is blocked even when it is not above the working folder (home is in cwd_deny_home.rs, a separate binary because it changes an environment variable)
#[test]
fn filesystem_root_is_denied() {
    let p = proj("root");
    assert_deny(&p, "rm -rf /");
    assert_deny(&p, "rm -rf //");
}

// Deleting a subfolder itself after `cd` is *inside* the working folder; unchanged (avoid over-blocking)
#[test]
fn after_cd_into_a_subfolder_deleting_that_subfolder_still_asks() {
    let p = proj("cdsub");
    assert_ask_unchanged(&p, "Bash", "cd build && rm -rf .");
    assert_ask_unchanged(&p, "Bash", "cd build && rm -rf \"$PWD\"");
}

// 4. relative subfolder = unchanged (card inside the permission dialog)
#[test]
fn relative_subfolder_still_asks_with_the_card() {
    let p = proj("rel");
    assert_ask_unchanged(&p, "Bash", "rm -rf build");
    // an absolute path *inside* the working folder also shows no built-in dialog (observed) -> unchanged
    assert_ask_unchanged(&p, "Bash", &format!("rm -rf {}/build", slash(&p)));
}

// 5. absolute path outside the working folder = unchanged
#[test]
fn absolute_path_outside_the_working_folder_still_asks() {
    let p = proj("outside");
    assert_ask_unchanged(&p, "Bash", &format!("rm -rf {}/other", slash(p.parent().unwrap())));
}

// 6. mode without a dialog = blocked as before (exit 2)
#[test]
fn no_prompt_modes_still_block_these_forms() {
    let p = proj("modes");
    for mode in ["bypassPermissions", "dontAsk"] {
        for cmd in [format!("rm -rf {}", slash(&p)), format!("rm -rf {}", slash(p.parent().unwrap())), "rm -rf .".to_string()] {
            let o = run(&p, "Bash", &cmd, mode);
            assert_eq!(o.code, 2, "{mode} {cmd}");
        }
    }
    assert!(p.join("build/notes.txt").exists());
}

// 7. PowerShell Remove-Item shows no built-in dialog (observed on Windows; the OLP card is inside the dialog) -> unchanged
#[test]
fn powershell_remove_item_of_the_working_folder_still_asks() {
    let p = proj("ps");
    assert_ask_unchanged(&p, "PowerShell", &format!("Remove-Item -Recurse -Force '{}'", p.to_string_lossy()));
}

// Windows path forms (drive-letter case, `\`, Git Bash `/d/…`) resolve to the same place -> deny
#[cfg(windows)]
#[test]
fn windows_path_spellings_of_the_working_folder_are_denied() {
    let p = proj("win");
    let s = slash(&p);
    let (drive, rest) = s.split_at(1);
    assert_deny(&p, &format!("rm -rf {}{}", drive.to_lowercase(), rest));
    assert_deny(&p, &format!("rm -rf {}{}", drive.to_uppercase(), rest));
    assert_deny(&p, &format!("rm -rf '{}'", p.to_string_lossy()));
    assert_deny(&p, &format!("rm -rf /{}{}", drive.to_lowercase(), &rest[1..]));
}

// Deleting through a symlink that points at the working folder is also denied (compared after normalisation)
#[cfg(unix)]
#[test]
fn symlink_to_the_working_folder_is_denied() {
    let p = proj("link");
    let link = p.parent().unwrap().join("link_to_proj");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    assert_deny(&p, &format!("rm -rf {}/", slash(&link)));
    // `..` after a link follows the real folder (as in the shell): `..` of a link pointing to build = the working folder
    let to_build = p.parent().unwrap().join("link_to_build");
    std::os::unix::fs::symlink(p.join("build"), &to_build).unwrap();
    assert_deny(&p, &format!("rm -rf {}/..", slash(&to_build)));
}
