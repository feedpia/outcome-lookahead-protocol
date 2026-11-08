//! Counter (on-device, read-only). It performs only these allowed reads:
//! (1) path expansion (`read_dir` enumeration, links not followed) (2) `git status --porcelain` (3) `git rev-list --count`.
//! The original command is never executed through any path. DBs are not counted (no connection; always unknown, user-declared counts are handled in classify).
//! The limit (`deadline`) comes from the settings file (no constant in code). Exceeding it gives Err = "could not verify".

pub mod spawn;

use std::path::{Path, PathBuf};
use std::time::Instant;

/// /// Number of files the argument paths point to (folders are enumerated and not counted themselves, a link counts as 1 and is not followed, a missing path = 0).
/// /// Walking uses the single `list_paths` implementation; with `cap == 0` it only counts and does not collect paths.
pub fn count_paths(cwd: &Path, args: &[&str], deadline: Instant) -> Result<u64, &'static str> {
    list_paths(cwd, args, deadline, 0).map(|(_, n)| n)
}

/// /// Walks the argument paths and collects up to `cap` file paths (only the picked ones). Returns (paths, total count).
/// /// Folder contents are in name order (same input gives same order), links are not followed, a missing path = 0. No writes.
/// /// With `cap == 0` only sorting is skipped (the time limit is still checked).
pub fn list_paths(cwd: &Path, args: &[&str], deadline: Instant, cap: usize) -> Result<(Vec<PathBuf>, u64), &'static str> {
    let mut out = Vec::new();
    let mut n: u64 = 0;
    let mut keep = |p: PathBuf, n: &mut u64| {
        *n += 1;
        if out.len() < cap {
            out.push(p);
        }
    };
    for a in args {
        if Instant::now() >= deadline {
            return Err("time limit reached");
        }
        let p = cwd.join(a);
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("could not read a path"),
        };
        if !m.is_dir() {
            keep(p, &mut n);
            continue;
        }
        let mut stack = vec![p];
        while let Some(d) = stack.pop() {
            let mut ents: Vec<(PathBuf, bool)> = Vec::new();
            for e in std::fs::read_dir(&d).map_err(|_| "could not list a folder")? {
                if Instant::now() >= deadline {
                    return Err("time limit reached");
                }
                let e = e.map_err(|_| "could not list a folder")?;
                let t = e.file_type().map_err(|_| "could not read a path")?;
                ents.push((e.path(), t.is_dir()));
            }
            if cap > 0 {
                ents.sort();
            }
            //             // Popped from the back, so subfolders are pushed in reverse to walk in name order
            for (path, _) in ents.iter().rev().filter(|(_, d)| *d) {
                stack.push(path.clone());
            }
            for (path, _) in ents.into_iter().filter(|(_, d)| !*d) {
                keep(path, &mut n);
            }
        }
    }
    Ok((out, n))
}

/// /// File category for one entry: `ignored` > (`recent` if modified within 24 hours) > `tracked` > `untracked`.
/// /// Two git calls (`ls-files` and `check-ignore`) in `project`, within the limit. git failure, timeout or a name outside the allowed shape = all `unreadable`.
/// /// `names` = project-relative names (`/` separated), `paths` = actual paths in the same order (for reading modification times).
/// /// Number of names (project-relative) that git does not track. Not a git repository, read failure or a name outside the allowed shape = None (unknown; nothing is invented).
pub fn not_in_git(project: &Path, names: &[String], deadline: Instant) -> Option<usize> {
    if names.is_empty() || !names.iter().all(|n| spawn::name_ok(n)) {
        return None;
    }
    let out = spawn::run(project, &spawn::Allowed::GitLsFiles(names.to_vec()), deadline).ok()?;
    let tracked: std::collections::HashSet<&str> = out.split('\0').filter(|x| !x.is_empty()).collect();
    Some(names.iter().filter(|n| !tracked.contains(n.as_str())).count())
}

pub fn natures(project: &Path, names: &[String], paths: &[PathBuf], deadline: Instant) -> Vec<&'static str> {
    let unreadable = vec!["unreadable"; names.len()];
    if names.is_empty() || !names.iter().all(|n| spawn::name_ok(n)) {
        return unreadable;
    }
    let split = |s: String| -> std::collections::HashSet<String> { s.split('\0').filter(|x| !x.is_empty()).map(|x| x.to_string()).collect() };
    let tracked = match spawn::run(project, &spawn::Allowed::GitLsFiles(names.to_vec()), deadline) {
        Ok(s) => split(s),
        Err(_) => return unreadable,
    };
    let ignored = match spawn::run(project, &spawn::Allowed::GitCheckIgnore(names.to_vec()), deadline) {
        Ok(s) => split(s),
        Err(_) => return unreadable,
    };
    let day = std::time::Duration::from_secs(24 * 3600);
    names
        .iter()
        .zip(paths)
        .map(|(n, p)| {
            if ignored.contains(n) {
                return "ignored";
            }
            let recent = std::fs::symlink_metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|age| age < day).unwrap_or(false);
            match (recent, tracked.contains(n)) {
                (true, _) => "recent",
                (false, true) => "tracked",
                (false, false) => "untracked",
            }
        })
        .collect()
}

/// /// Number of uncommitted changes that `git reset --hard` would lose (untracked `??` lines excluded).
pub fn count_uncommitted_changes(cwd: &Path, deadline: Instant) -> Result<u64, &'static str> {
    let out = spawn::run(cwd, &spawn::Allowed::GitStatusPorcelain, deadline)?;
    Ok(out.lines().filter(|l| !l.is_empty() && !l.starts_with("??")).count() as u64)
}

/// /// Number of commits in range `<a>..<b>` (commits a force push would drop from the remote = remote-tracking commits not present locally).
pub fn count_commits(cwd: &Path, range: &str, deadline: Instant) -> Result<u64, &'static str> {
    let out = spawn::run(cwd, &spawn::Allowed::GitRevListCount(range.to_string()), deadline)?;
    out.trim().parse::<u64>().map_err(|_| "unexpected git output")
}
