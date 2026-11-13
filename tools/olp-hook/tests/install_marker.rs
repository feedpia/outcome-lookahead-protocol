//! 0.1.3 ⑤ — uninstall removes the empty lists install created, and deletes a settings file only when install created it and nothing else is in it.
//! Design: OLP 0.1.3 install gate §5-0. All files live under CARGO_TARGET_TMPDIR (fake project, home, package).
// Value: protects=never deletes or empties a settings file, list or key that someone else made; fails_when=a file that existed before install is deleted,
// a user's own empty list or key disappears, a marker copied from elsewhere or broken triggers a delete, a symlink is followed, telemetry off or a dry run deletes;
// why_new=uninstall left `{"hooks":{"PreToolUse":[],"PostToolUse":[]}}` behind (e2e step 12 FACT, owner ledger 1107 ②); seam=install::run, install::edit_start; no_claim=.claude folder cleanup.
use olp_hook::install::{edit_start, run, InstallEnv};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static SEQ: AtomicUsize = AtomicUsize::new(0);

struct World {
    env: InstallEnv,
    project: PathBuf,
    home: PathBuf,
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn world(name: &str) -> World {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_marker_tests").join(format!("{name}_{}_{n}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let (project, home, src) = (root.join("proj"), root.join("home"), root.join("pkg"));
    for d in [&project, &home, &src] {
        fs::create_dir_all(d).unwrap();
    }
    fs::write(src.join(exe("olp-hook")), b"hook-bytes").unwrap();
    let env = InstallEnv { project: project.clone(), home: home.clone(), exe: src.join(exe("olp-hook")), now: 1_700_000_000 };
    World { env, project, home }
}

fn settings(base: &Path) -> PathBuf {
    base.join(".claude").join("settings.json")
}

fn marker(base: &Path) -> PathBuf {
    base.join(".olp").join("install-created-settings.json")
}

fn read(p: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
}

fn write(p: &Path, v: &Value) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, serde_json::to_string_pretty(v).unwrap() + "\n").unwrap();
}

fn install(w: &World) -> String {
    run("install", &[], &w.env).unwrap()
}

fn uninstall(w: &World) -> String {
    run("uninstall", &[], &w.env).unwrap()
}

fn backups(w: &World) -> usize {
    fs::read_dir(w.home.join("olp").join("backup")).map(|d| d.count()).unwrap_or(0)
}

// ---- N: never delete or empty what someone else made ----

#[test]
fn n1_uninstall_keeps_settings_file_that_existed_before_install() {
    let w = world("n1");
    write(&settings(&w.project), &json!({}));
    install(&w);
    uninstall(&w);
    assert!(settings(&w.project).exists(), "a file that existed before install is someone else's: keep it");
    assert_eq!(read(&settings(&w.project)), json!({}), "same meaning as before install");
}

#[test]
fn n2_uninstall_keeps_marked_file_when_user_added_keys() {
    let w = world("n2");
    install(&w);
    let mut v = read(&settings(&w.project));
    v["theme"] = json!("dark");
    write(&settings(&w.project), &v);
    uninstall(&w);
    assert_eq!(read(&settings(&w.project)), json!({ "theme": "dark" }));
}

#[test]
fn n3_uninstall_keeps_marked_file_when_user_added_own_hook() {
    let w = world("n3");
    install(&w);
    let mut v = read(&settings(&w.project));
    let other = json!({ "matcher": "Edit", "hooks": [ { "type": "command", "command": "my-check" } ] });
    v["hooks"]["PostToolUse"].as_array_mut().unwrap().push(other.clone());
    write(&settings(&w.project), &v);
    uninstall(&w);
    assert_eq!(read(&settings(&w.project)), json!({ "hooks": { "PostToolUse": [other] } }));
}

#[test]
fn n4_uninstall_keeps_users_own_empty_list() {
    let w = world("n4");
    write(&settings(&w.project), &json!({ "hooks": { "Stop": [], "PostToolUse": [] } }));
    install(&w);
    uninstall(&w);
    assert_eq!(read(&settings(&w.project)), json!({ "hooks": { "Stop": [], "PostToolUse": [] } }), "lists that were there before install stay, even empty");
}

#[test]
fn n5_uninstall_ignores_marker_for_other_settings_path() {
    let a = world("n5a");
    install(&a);
    let b = world("n5b");
    install(&b);
    fs::copy(marker(&a.project), marker(&b.project)).unwrap();
    uninstall(&b);
    assert!(settings(&b.project).exists(), "a marker that names another settings file is not ours");
}

#[test]
fn n6_uninstall_ignores_broken_marker() {
    for (i, bad) in ["{oops", "", "{\"v\":99}"].iter().enumerate() {
        let w = world(&format!("n6_{i}"));
        install(&w);
        fs::write(marker(&w.project), bad).unwrap();
        let out = uninstall(&w);
        assert!(settings(&w.project).exists(), "broken marker {bad:?} = keep the file");
        assert!(!out.contains("install created it"), "no file-delete line: {out}");
    }
}

#[test]
fn n7_uninstall_never_deletes_through_symlink() {
    let w = world("n7");
    install(&w);
    let real = w.project.parent().unwrap().join("real-settings.json");
    fs::rename(settings(&w.project), &real).unwrap();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real, settings(&w.project)).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&real, settings(&w.project)).is_ok();
    if !made {
        eprintln!("SKIP n7: this machine cannot make a symlink (the e2e-env Linux container runs it)");
        return;
    }
    let target_before = fs::read(&real).unwrap();
    uninstall(&w);
    // What this checks: uninstall does not delete through the link, and the file the link pointed at is not touched.
    // It does not check that the link stays a link: the normal rewrite (temp file + rename) puts a plain file at the settings path.
    assert!(fs::symlink_metadata(settings(&w.project)).is_ok(), "something is still at the settings path (not deleted)");
    assert_eq!(fs::read(&real).unwrap(), target_before, "the link target is unchanged");
}

#[test]
fn n8_telemetry_off_never_deletes_settings_file() {
    let w = world("n8");
    install(&w);
    edit_start(&w.env, false, true).unwrap();
    let mut v = read(&settings(&w.project));
    v["hooks"].as_object_mut().unwrap().retain(|k, _| k == "SessionStart");
    write(&settings(&w.project), &v);
    edit_start(&w.env, false, false).unwrap();
    assert!(settings(&w.project).exists(), "only uninstall may delete the file");
}

#[test]
fn n9_uninstall_dry_run_deletes_nothing() {
    let w = world("n9");
    install(&w);
    let before = fs::read(settings(&w.project)).unwrap();
    let out = run("uninstall", &["--dry-run".to_string()], &w.env).unwrap();
    assert_eq!(fs::read(settings(&w.project)).unwrap(), before);
    assert!(marker(&w.project).exists());
    assert!(out.contains("would remove"), "{out}");
}

#[test]
fn n10_start_entry_created_file_is_kept() {
    let w = world("n10");
    edit_start(&w.env, false, true).unwrap();
    assert!(settings(&w.project).exists());
    uninstall(&w);
    assert!(settings(&w.project).exists(), "telemetry on wrote no marker, so uninstall keeps the file");
}

#[test]
fn n11_old_install_without_marker_prunes_only_lists_it_emptied() {
    // 0.1.1·0.1.2 installs have no marker: remove the lists this uninstall emptied (design review Q2), keep the file and every other list.
    let w = world("n11");
    write(&settings(&w.project), &json!({ "theme": "dark", "hooks": { "Stop": [] } }));
    install(&w);
    fs::remove_file(marker(&w.project)).unwrap();
    uninstall(&w);
    assert_eq!(read(&settings(&w.project)), json!({ "theme": "dark", "hooks": { "Stop": [] } }));
}

// ---- P: remove what install made ----

#[test]
fn p1_uninstall_removes_empty_lists_it_created() {
    let w = world("p1");
    install(&w);
    let mut v = read(&settings(&w.project));
    v["theme"] = json!("dark");
    write(&settings(&w.project), &v);
    uninstall(&w);
    let after = read(&settings(&w.project));
    assert!(after.get("hooks").is_none(), "hooks object install created is gone when empty: {after}");
}

#[test]
fn p2_uninstall_deletes_settings_file_install_created() {
    let w = world("p2");
    install(&w);
    assert!(marker(&w.project).exists());
    let out = uninstall(&w);
    assert!(!settings(&w.project).exists(), "install created it and nothing else is in it");
    assert!(!marker(&w.project).exists());
    assert_eq!(backups(&w), 1, "the original is backed up before deleting");
    assert!(out.contains("install created it") && out.contains("backup"), "{out}");
}

#[test]
fn p3_uninstall_after_usage_yes_removes_start_list_too() {
    let w = world("p3");
    install(&w);
    edit_start(&w.env, false, true).unwrap();
    uninstall(&w);
    assert!(!settings(&w.project).exists());
}

#[test]
fn p4_install_writes_marker_and_created_file_flag() {
    let a = world("p4a");
    install(&a);
    assert_eq!(read(&marker(&a.project))["created_file"], json!(true));
    let b = world("p4b");
    write(&settings(&b.project), &json!({ "theme": "dark" }));
    install(&b);
    assert_eq!(read(&marker(&b.project))["created_file"], json!(false));
}

#[test]
fn p5_reinstall_keeps_marker() {
    let w = world("p5");
    install(&w);
    let first = fs::read(marker(&w.project)).unwrap();
    install(&w);
    assert_eq!(fs::read(marker(&w.project)).unwrap(), first);
}

#[test]
fn p6_install_created_file_has_only_our_keys() {
    let w = world("p6");
    install(&w);
    let v = read(&settings(&w.project));
    let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["hooks"], "if install ever writes another top-level key, uninstall could never find the file empty");
}

// ---- B review condition 1 (REVIEW_OLP013_MARKER.md): a marker left over from an earlier install must not claim someone else's things ----

#[test]
fn n12_stale_marker_does_not_claim_users_new_file() {
    // A: install (fresh) → person deletes the settings file by hand → person makes their own {} → install → uninstall ⇒ their file stays.
    let w = world("n12");
    install(&w);
    fs::remove_file(settings(&w.project)).unwrap();
    write(&settings(&w.project), &json!({}));
    install(&w);
    uninstall(&w);
    assert!(settings(&w.project).exists(), "the person's own file existed before this install: keep it");
    assert_eq!(read(&settings(&w.project)), json!({}));
}

#[test]
fn n13_stale_marker_does_not_claim_users_empty_list() {
    // B: install → person removes our entries and writes their own empty PreToolUse → uninstall (nothing to remove, marker left) → install → uninstall.
    let w = world("n13");
    install(&w);
    write(&settings(&w.project), &json!({ "theme": "dark", "hooks": { "PreToolUse": [] } }));
    uninstall(&w);
    install(&w);
    uninstall(&w);
    assert_eq!(read(&settings(&w.project)), json!({ "theme": "dark", "hooks": { "PreToolUse": [] } }), "their empty list and hooks object stay");
}

// ---- B recommendation (REVIEW_OLP013_MARKER RECHECK 1, non-blocking): the same stale-marker check on the `telemetry on` path ----

#[test]
fn n14_stale_marker_not_claimed_by_telemetry_on() {
    // install (creates the file) → person removes our entries by hand, keeps their own `{}` → telemetry on → uninstall ⇒ their file stays.
    let w = world("n14");
    install(&w);
    write(&settings(&w.project), &json!({}));
    edit_start(&w.env, false, true).unwrap();
    uninstall(&w);
    assert!(settings(&w.project).exists(), "the file held none of our entries before telemetry on: it is the person's now, keep it");
    assert_eq!(read(&settings(&w.project)), json!({}));
}

#[test]
fn n15_telemetry_on_after_install_still_cleaned_up() {
    // Not stale: install → telemetry on → uninstall ⇒ the file install created goes, as before (p3 keeps the list case).
    let w = world("n15");
    install(&w);
    edit_start(&w.env, false, true).unwrap();
    uninstall(&w);
    assert!(!settings(&w.project).exists(), "install created it and it held only our entries: uninstall removes it");
}

#[test]
fn n16_telemetry_on_says_when_old_marker_cannot_be_removed() {
    // Second review note: a stale marker that cannot be removed must be reported, not ignored. A directory in its place makes remove_file fail.
    let w = world("n16");
    install(&w);
    write(&settings(&w.project), &json!({}));
    let m = marker(&w.project);
    fs::remove_file(&m).unwrap();
    fs::create_dir_all(&m).unwrap();
    let out = edit_start(&w.env, false, true).unwrap();
    assert!(out.contains("could not remove the old install marker"), "telemetry on reports it: {out}");
    uninstall(&w);
    assert!(settings(&w.project).exists(), "no readable marker, so uninstall keeps the file");
}
