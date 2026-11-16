//! Shared adversarial input list (DESIGN_OLP_RUNNER_DETECT1 2 section 3, DESIGN_OLP013_RC4_RUNNER_KEY 1-6 section 3).
//! One row = one form. The rc4 suite asserts `key` and `card_never`; the self-guard suite (0.2.0 U3a) asserts `guard` from the same file.
//! Every row is either asserted or counted as not applied (`key` = `skip`), and the two numbers must add up to the rows read.
//! All file operations happen only in throwaway folders under CARGO_TARGET_TMPDIR.
use olp_hook::classify::{self, family_is_name, interpreter_only, runner_only, Limits};
use olp_hook::{run_hook, store, HookOutcome};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const LIST: &str = include_str!("fixtures/runner_adversarial.jsonl");
const FIELDS: [&str; 9] = ["id", "shell", "cwd", "input", "stored", "key", "card_never", "guard", "from"];
const SEED_ROWS: usize = 191;

static SEQ: AtomicUsize = AtomicUsize::new(0);

fn rows() -> Vec<Value> {
    LIST.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad row {l}: {e}"))).collect()
}

fn s<'a>(r: &'a Value, f: &str) -> &'a str {
    r[f].as_str().unwrap_or_else(|| panic!("{}: field {f} is not text", r["id"]))
}

fn list<'a>(r: &'a Value, f: &str) -> Vec<&'a str> {
    r[f].as_array().unwrap_or_else(|| panic!("{}: field {f} is not a list", r["id"])).iter().map(|v| v.as_str().unwrap()).collect()
}

fn project(id: &str, cwd: &str, stored: &[&str]) -> (PathBuf, PathBuf) {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_runner_adversarial").join(format!("{id}_{}_{n}", std::process::id()));
    if p.exists() {
        fs::remove_dir_all(&p).unwrap();
    }
    let c = if cwd == "." { p.clone() } else { p.join(cwd) };
    fs::create_dir_all(&c).unwrap();
    fs::create_dir_all(p.join("x")).unwrap();
    // rev 17-3: answers live in the user's trust folder keyed by the project path `p` (the hook gets Some(p)), not in the project
    let t = store::trust_dir(&p);
    let _ = fs::remove_dir_all(&t);
    if !stored.is_empty() {
        let ro = json!({"effect": "read_only", "reversible": false, "recovery": "unknown", "declared_at": 1794579024});
        let entries: serde_json::Map<String, Value> = stored.iter().map(|k| (format!("bash:{k}"), ro.clone())).collect();
        fs::create_dir_all(&t).unwrap();
        fs::write(t.join("hook-map.json"), json!({"version": 1, "entries": entries}).to_string()).unwrap();
    }
    (p, c)
}

fn hook(p: &Path, cwd: &Path, tool: &str, cmd: &str) -> HookOutcome {
    let v = json!({"session_id": "s", "transcript_path": "t", "cwd": cwd.to_string_lossy(), "hook_event_name": "PreToolUse", "tool_name": tool,
                   "tool_input": {"command": cmd}, "permission_mode": "bypassPermissions", "tool_use_id": "toolu_adv"});
    run_hook(v.to_string().as_bytes(), Some(p))
}

// Value: protects=the shared list stays one form per line with the closed set of 9 fields, unique ids, a non-empty input and at least the seed rows; fails_when=a row adds an unknown field, repeats an id or drops below the seeds; seam=fixtures/runner_adversarial.jsonl
#[test]
fn adversarial_list_is_wellformed() {
    let rs = rows();
    assert!(rs.len() >= SEED_ROWS, "rows {} < seeds {SEED_ROWS}", rs.len());
    assert_eq!(rs.len(), LIST.lines().filter(|l| !l.trim().is_empty()).count(), "one row per line");
    let mut ids = HashSet::new();
    for r in &rs {
        let o = r.as_object().unwrap();
        let mut keys: Vec<&str> = o.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        let mut want = FIELDS.to_vec();
        want.sort_unstable();
        assert_eq!(keys, want, "{}: fields", r["id"]);
        assert!(ids.insert(s(r, "id").to_string()), "{}: id repeats", r["id"]);
        assert!(!s(r, "input").trim().is_empty(), "{}: empty input", r["id"]);
        assert!(["bash", "powershell"].contains(&s(r, "shell")), "{}: shell", r["id"]);
        assert!(["block", "ask", "none"].contains(&s(r, "guard")), "{}: guard", r["id"]);
        assert!(!s(r, "key").is_empty() && !s(r, "cwd").is_empty() && !s(r, "from").is_empty(), "{}: key/cwd/from", r["id"]);
        let _ = (list(r, "stored"), list(r, "card_never"));
    }
}

// Value: protects=for every row the key the hook computes matches the list (exact text, `unnamed` = no group answer and no silent pass over the stored answers, `same` = an ordinary name), the card never offers a bare program, runner-only or interpreter-only answer, and asserted + not applied = rows read; fails_when=any adversarial form takes a group answer or a row is skipped silently; seam=classify::assess + run_hook over fixtures/runner_adversarial.jsonl
#[test]
fn adversarial_list_keys_and_cards() {
    let rs = rows();
    let (mut asserted, mut not_applied) = (0usize, 0usize);
    for r in &rs {
        let id = s(r, "id");
        let key = s(r, "key");
        if key == "skip" {
            not_applied += 1;
            continue;
        }
        let input = s(r, "input");
        let tool = if s(r, "shell") == "powershell" { "PowerShell" } else { "Bash" };
        let stored = list(r, "stored");
        let (p, cwd) = project(id, s(r, "cwd"), &stored);
        let map = store::load_map(&p).unwrap();
        classify::set_project_anchor(Some(p.clone()));
        let a = classify::assess(tool, &json!({ "command": input }), &map, &cwd, Limits { counter: Some(Duration::from_secs(3)), ps_parser: true });
        classify::set_project_anchor(None);
        match key {
            // The row's program is the first word of its last piece (`cd x; node a.js` -> node); an earlier piece such as
            // PowerShell `Set-Location x` may still be its own candidate.
            "unnamed" => {
                let quoted = input.contains(['\'', '"']);
                let last = if quoted { input } else { input.rsplit([';', '&']).next().unwrap_or(input).trim() };
                // The program after wrappers (`env --chdir=x node a.js` -> node), read from the piece's key.
                let words: Vec<String> = last.trim_start_matches('(').split_whitespace().map(String::from).collect();
                let fam = classify::family_of(&words).unwrap_or_default();
                let prog = fam.split(' ').next().unwrap_or("").to_string();
                assert!(
                    !a.to_classify.iter().any(|k| k.split(' ').next() == Some(prog.as_str())),
                    "{id} {input}: expected no group answer for {prog:?}, got {:?}",
                    a.to_classify
                );
                // The piece itself is marked as taking no group answer, unless a danger or self-guard already decided it.
                assert!(
                    a.unnamed || !a.findings.is_empty() || a.self_guard,
                    "{id} {input}: not marked unnamed (to_classify {:?})",
                    a.to_classify
                );
            }
            "same" => assert!(
                a.to_classify.iter().all(|k| family_is_name(k) && !runner_only(k) && !interpreter_only(k)),
                "{id} {input}: {:?}",
                a.to_classify
            ),
            k => assert!(a.to_classify.iter().any(|t| t == k), "{id} {input}: expected key {k:?}, got {:?}", a.to_classify),
        }
        let o = hook(&p, &cwd, tool, input);
        if key != "same" && !stored.is_empty() {
            assert_ne!(o.code, 0, "{id} {input}: an answer stored for {stored:?} let it pass");
        }
        let first = input.split_whitespace().next().unwrap_or("").to_lowercase();
        let mut never: Vec<String> = list(r, "card_never").iter().map(|x| x.to_string()).collect();
        if never.is_empty() {
            never.push(classify::family_of(std::slice::from_ref(&first)).unwrap_or(first));
            never.extend(stored.iter().filter(|k| !k.contains(' ') || runner_only(k)).map(|k| k.to_string()));
        }
        // An `unnamed` row offers no stored key at all (B RECHECK 2: one list check instead of one test per form).
        if key == "unnamed" {
            never.extend(stored.iter().map(|k| k.to_string()));
        }
        let out = format!("{}{}", o.stderr, o.stdout);
        // Ledger 1321 ①: a command with any bundle sign never offers a remembered answer.
        if classify::bundled(input, tool == "PowerShell") {
            assert!(!out.contains("classify --bash"), "{id} {input}: bundled command offers a remembered answer: {out}");
        }
        for n in &never {
            assert!(!out.contains(&format!("--bash \"{n}\"")), "{id} {input}: card offers {n:?}: {out}");
        }
        asserted += 1;
    }
    println!("runner_adversarial rc4: read {} asserted {asserted} not applied {not_applied}", rs.len());
    assert_eq!(asserted + not_applied, rs.len());
}
