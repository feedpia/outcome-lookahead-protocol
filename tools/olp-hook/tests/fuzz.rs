//! 0.1.3 tag gate "fuzz" cell (DESIGN_OLP_013_FUZZ v2): feeds seeded random commands into the verdict engine and checks
//! (1) no panic (2) every hook call within its time limit (3) same input = same verdict (pure path) (4) delete disguises never leak as Read or QuietPass.
//! No new package: random = SplitMix64 (15 lines below). Default = 1/10 of seed 1301 (smoke test); `OLP_FUZZ_FULL=1` = all 3 seeds in full.
//! Output: when the `OLP_FUZZ_OUT` folder exists, `fuzz-<os>.json` (no command text, counts and digest only).
use olp_hook::classify::Limits;
use olp_hook::config;
use olp_hook::hook::{decide, run_hook, GuardFail, HookRequest, Route};
use olp_hook::store;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

const FUZZ_GEN: u32 = 1;
const SEEDS: [u64; 3] = [1301, 1302, 1303];
/// Cases per group per seed (design section 5): A 6,000 · B 2,000 · C 1,000 · D 300.
const FULL_COUNTS: [usize; 4] = [6000, 2000, 1000, 300];
const MODES: [&str; 3] = ["auto", "default", "bypassPermissions"];

struct Rng(u64);
impl Rng {
    fn new(seed: u64, bucket: u64, idx: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ bucket.wrapping_mul(0xBF58_476D_1CE4_E5B9) ^ idx.wrapping_mul(0x94D0_49BB_1331_11EB) ^ u64::from(FUZZ_GEN))
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

const WORDS: &[&str] = &[
    "ls", "cat", "echo", "grep", "rm", "mv", "cp", "touch", "mkdir", "git", "curl", "wget", "python", "node", "npm", "cargo", "make",
    "find", "xargs", "sed", "awk", "tar", "chmod", "chown", "ln", "dd", "tee", "printf", "cd", "pwd", "Get-ChildItem", "Remove-Item",
    "Copy-Item", "Move-Item", "Set-Content", "Write-Output", "Invoke-WebRequest", "cmd", "powershell", "bash",
];
const ARGS: &[&str] = &[
    "-rf", "-r", "-f", "-la", "--force", "-Recurse", "-Force", "x", "build", "src/a.txt", "~", "..", "/", "./x/../y", "C:\\Users\\a", "~/.ssh",
    "*.log", "-delete", "-exec", "{}", "\\;", "-name", "'a b'", "\"q\"", "/tmp/z", "D:\\", "--", "-", ".git", "node_modules",
];
const QUOTES: &[&str] = &["'", "\"", "'a b'", "\"$X\"", "\\\"", "`\"", "'it'\"'\"'s'", "\"unterminated"];
const VARS: &[&str] = &["$X", "${X}", "$env:X", "%X%", "${X:-/}", "$HOME", "$1", "$@"];
const SUBS: &[&str] = &["$(ls)", "`pwd`", "<(cat x)", "$(rm -rf x)", "$(echo $(pwd))"];
const JOINS: &[&str] = &[";", "&&", "||", "|", "&", "\n", " ; ", "|&"];
const REDIRS: &[&str] = &[">", ">>", "2>&1", "<", "> /dev/null", ">| x", "<<EOF"];
const ODD: &[&str] = &["한글", "🙂", "\u{0007}", "\u{202e}", "\t", "\r", "\u{200b}", "é", "\u{feff}"];

/// Delete cores (fixed tool names): only those the `b_hand_cases` test confirmed one by one as "not a pass".
const B_CORES: &[(&str, &str)] = &[
    ("Bash", "rm -rf build"),
    ("Bash", "rm -r build"),
    ("Bash", "rm -rf ./build/"),
    ("Bash", "find build -delete"),
    ("Bash", "git clean -fdx"),
    ("PowerShell", "Remove-Item -Recurse -Force build"),
];
/// Disguises that still run (design section 3 B). The core goes into `{}`.
const B_WRAP_BASH: &[&str] = &[
    "{}", "cd src && {}", "FOO=1 {}", "sudo {}", "env {}", "nohup {}", "{} ; ls", "{} && echo ok", "{} | cat", "( {} )", "$( {} )",
    "bash -c '{}'", "timeout 5 {}", "true; {}",
];
const B_WRAP_PS: &[&str] = &["{}", "Set-Location src; {}", "{}; Get-ChildItem", "& { {} }"];
/// Just text (design section 3 C): verdict values are only counted.
const C_FORMS: &[&str] = &["echo 'rm -rf build'", "# rm -rf build", "printf '%s' \"rm -rf build\"", "cat <<'EOF'\nrm -rf build\nEOF", "echo \"Remove-Item -Recurse build\""];

#[derive(Clone, Debug)]
struct Input {
    tool: String,
    command: String,
}

fn gen(seed: u64, bucket: usize, idx: usize) -> Input {
    let mut r = Rng::new(seed, bucket as u64, idx as u64);
    match bucket {
        0 => {
            let n = 1 + r.below(40);
            let mut s = String::new();
            for _ in 0..n {
                let piece = match r.below(8) {
                    0 | 1 => r.pick(WORDS).to_string(),
                    2 | 3 => r.pick(ARGS).to_string(),
                    4 => r.pick(QUOTES).to_string(),
                    5 => r.pick(VARS).to_string(),
                    6 => if r.below(2) == 0 { r.pick(SUBS).to_string() } else { r.pick(REDIRS).to_string() },
                    _ => if r.below(3) == 0 { r.pick(ODD).to_string() } else { r.pick(JOINS).to_string() },
                };
                s.push_str(&piece);
                s.push(' ');
            }
            if r.below(50) == 0 {
                s.push_str(&"a".repeat(20_000 + r.below(20_000)));
            }
            Input { tool: if r.below(3) == 0 { "PowerShell" } else { "Bash" }.into(), command: s }
        }
        1 => {
            let (tool, core) = B_CORES[r.below(B_CORES.len())];
            let wraps = if tool == "Bash" { B_WRAP_BASH } else { B_WRAP_PS };
            Input { tool: tool.into(), command: r.pick(wraps).replacen("{}", core, 1) }
        }
        2 => Input { tool: "Bash".into(), command: r.pick(C_FORMS).into() },
        _ => Input { tool: "Bash".into(), command: String::new() },
    }
}

/// Group D: PreToolUse bytes fed to run_hook (broken UTF-8, lone surrogate, empty bytes, wrong-shape command, 1 MB).
fn gen_payload(seed: u64, idx: usize, cwd: &Path) -> Vec<u8> {
    let mut r = Rng::new(seed, 3, idx as u64);
    let cwd = cwd.to_string_lossy().replace('\\', "\\\\");
    let pre = format!("{{\"hook_event_name\":\"PreToolUse\",\"cwd\":\"{cwd}\",\"permission_mode\":\"auto\",\"tool_name\":\"Bash\",\"tool_input\":");
    match r.below(9) {
        0 => vec![0xff, 0xfe, b'{', 0x80],
        1 => format!("{pre}{{\"command\":\"rm -rf \\ud800build\"}}}}").into_bytes(),
        2 => Vec::new(),
        3 => b"\n\n\n".to_vec(),
        4 => format!("{pre}{{\"command\":12345}}}}").into_bytes(),
        5 => format!("{pre}{{\"command\":null}}}}").into_bytes(),
        6 => format!("{pre}{{\"command\":[\"rm\",\"-rf\",\"build\"]}}}}").into_bytes(),
        7 => format!("{pre}{{}}}}").into_bytes(),
        _ => {
            let mut b = format!("{pre}{{\"command\":\"rm -rf build ").into_bytes();
            b.extend(std::iter::repeat_n(b'a', 1 << 20));
            b.extend_from_slice(b"\"}}");
            b
        }
    }
}

fn payload_kind(seed: u64, idx: usize) -> usize {
    Rng::new(seed, 3, idx as u64).below(9)
}

#[derive(Clone, Debug, PartialEq)]
struct Verdict {
    route: Option<Route>,
    kinds: Vec<String>,
    panicked: bool,
    timed_out: bool,
}

type Judge<'a> = dyn Fn(&Input, &str, &Path, Limits) -> Verdict + 'a;

fn real_judge(inp: &Input, mode: &str, project: &Path, limits: Limits) -> Verdict {
    let cfg = config::load(project).unwrap();
    let map = store::load_map(project).unwrap();
    let req = HookRequest { tool: inp.tool.clone(), input: json!({ "command": inp.command }), cwd: project.to_path_buf(), mode: Some(mode.into()) };
    match decide(req, project, &cfg, Arc::new(map), limits, cfg.hook_budget()) {
        Ok(d) => Verdict {
            route: Some(d.route),
            kinds: d.a.findings.iter().map(|f| serde_json::to_value(f.kind).unwrap().as_str().unwrap_or("").to_string()).collect(),
            panicked: false,
            timed_out: false,
        },
        Err(GuardFail::Panic) => Verdict { route: None, kinds: vec![], panicked: true, timed_out: false },
        Err(GuardFail::Timeout) => Verdict { route: None, kinds: vec![], panicked: false, timed_out: true },
    }
}

fn is_pass(r: Option<Route>) -> bool {
    matches!(r, Some(Route::Read) | Some(Route::QuietPass))
}

fn pure() -> Limits {
    Limits { counter: None, ps_parser: false }
}

fn project(tag: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("olp_fuzz").join(format!("{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("build")).unwrap();
    std::fs::create_dir_all(p.join("src")).unwrap();
    for i in 0..3 {
        std::fs::write(p.join("build").join(format!("f{i}.txt")), "x").unwrap();
    }
    p
}

/// sha256 (FIPS 180-4): 64-character digest (the shape in DESIGN_OLP_013_TAG_GATE section 4), no new package.
fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be,
        0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa,
        0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85,
        0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f,
        0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut m = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&bits.to_be_bytes());
    for c in m.chunks(64) {
        let mut w = [0u32; 64];
        for (i, wi) in w.iter_mut().enumerate().take(16) {
            *wi = u32::from_be_bytes([c[4 * i], c[4 * i + 1], c[4 * i + 2], c[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [t1.wrapping_add(t2), v[0], v[1], v[2], v[3].wrapping_add(t1), v[4], v[5], v[6]];
        }
        for (hi, vi) in h.iter_mut().zip(v) {
            *hi = hi.wrapping_add(vi);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[derive(Default, Debug)]
struct Report {
    inputs: [usize; 4],
    panics: usize,
    timeouts: usize,
    nondeterministic: usize,
    b_pass: usize,
    d_not_blocked: usize,
    max_ms: u128,
    c_routes: std::collections::BTreeMap<String, usize>,
    /// Input number -> verdict line (no command text): material for the sha256 digest.
    lines: Vec<u8>,
    failures: Vec<String>,
}

fn run(seeds: &[u64], counts: [usize; 4], judge: &Judge, proj: &Path) -> Report {
    let mut rep = Report::default();
    let budget_ms = config::load(proj).unwrap().hook_budget().map(|d| d.as_millis()).unwrap_or(5000);
    for &seed in seeds {
        for bucket in 0..3 {
            for idx in 0..counts[bucket] {
                let inp = gen(seed, bucket, idx);
                let mode = MODES[idx % MODES.len()];
                let t = Instant::now();
                let v1 = judge(&inp, mode, proj, pure());
                let ms = t.elapsed().as_millis();
                rep.max_ms = rep.max_ms.max(ms);
                let _between = judge(&gen(seed, bucket, idx + 7919), mode, proj, pure());
                let v2 = judge(&inp, mode, proj, pure());
                rep.inputs[bucket] += 1;
                let id = format!("seed {seed} bucket {} idx {idx} tool {} mode {mode} cmd {:?}", ["A", "B", "C"][bucket], inp.tool, inp.command.chars().take(160).collect::<String>());
                if v1.panicked || v2.panicked {
                    rep.panics += 1;
                    rep.failures.push(format!("panic: {id}"));
                }
                if v1.timed_out || ms > budget_ms {
                    rep.timeouts += 1;
                    rep.failures.push(format!("time {ms} ms: {id}"));
                }
                if v1 != v2 {
                    rep.nondeterministic += 1;
                    rep.failures.push(format!("different verdicts {:?} / {:?}: {id}", v1.route, v2.route));
                }
                if bucket == 1 && is_pass(v1.route) {
                    rep.b_pass += 1;
                    rep.failures.push(format!("delete passed as {:?}: {id}", v1.route));
                }
                if bucket == 2 {
                    *rep.c_routes.entry(format!("{mode}:{:?}", v1.route)).or_default() += 1;
                }
                rep.lines.extend_from_slice(format!("{seed}/{bucket}/{idx}/{:?}/{:?}\n", v1.route, v1.kinds).as_bytes());
            }
        }
        for idx in 0..counts[3] {
            let bytes = gen_payload(seed, idx, proj);
            let out = std::panic::catch_unwind(|| run_hook(&bytes, Some(proj)));
            rep.inputs[3] += 1;
            match out {
                Err(_) => {
                    rep.panics += 1;
                    rep.failures.push(format!("panic: seed {seed} bucket D idx {idx}"));
                }
                // allowed = exit 0 with no permission decision; exit 2 (block) or exit 0 + «deny»/«ask» JSON is not allowed
                Ok(o) if o.code != 2 && !o.stdout.contains("\"permissionDecision\"") => {
                    rep.d_not_blocked += 1;
                    rep.failures.push(format!("allowed (code {}): seed {seed} bucket D idx {idx} kind {} bytes {:?}", o.code, payload_kind(seed, idx), String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(60)..])));
                }
                Ok(o) => rep.lines.extend_from_slice(format!("{seed}/D/{idx}/{}/{}\n", o.code, o.stdout.contains("\"deny\"")).as_bytes()),
            }
        }
    }
    rep
}

fn full() -> bool {
    std::env::var("OLP_FUZZ_FULL").ok().as_deref() == Some("1")
}

// Value: protects=the digest sha256 equals the standard value (the 64 characters the gate reads); fails_when=a hand-written sha256 is wrong; seam=sha256_hex.
#[test]
fn sha256_known_vectors() {
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(sha256_hex(&[b'a'; 1000]), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
}

// Value: protects=same (version, seed, group, number) -> same text (the condition to reproduce a failure); fails_when=same seed gives different text; seam=gen pure function.
#[test]
fn gen_is_reproducible() {
    for b in 0..3 {
        for i in [0usize, 1, 99, 5999] {
            assert_eq!(gen(1301, b, i).command, gen(1301, b, i).command);
        }
    }
    assert_ne!(gen(1301, 0, 1).command, gen(1302, 0, 1).command);
    let p = project("gen");
    assert_eq!(gen_payload(1301, 4, &p), gen_payload(1301, 4, &p));
}

// Value: protects=every B core x disguise is "not a pass" under the current rules (design section 6 hand cases; rules are not widened to fit the test);
// fails_when=a disguise in B that the rules miss keeps the gate red; seam=decide (pure path). Cells: delete finding present / everything else (ask, block).
#[test]
fn b_hand_cases() {
    let p = project("hand");
    let mut finding = 0;
    let mut other = 0;
    for (tool, core) in B_CORES {
        let wraps = if *tool == "Bash" { B_WRAP_BASH } else { B_WRAP_PS };
        for w in wraps {
            let inp = Input { tool: tool.to_string(), command: w.replacen("{}", core, 1) };
            for mode in MODES {
                let v = real_judge(&inp, mode, &p, pure());
                assert!(!is_pass(v.route), "{tool} {:?} mode {mode} passed as {:?}", inp.command, v.route);
                if v.kinds.iter().any(|k| k.contains("delete")) { finding += 1 } else { other += 1 }
            }
        }
    }
    eprintln!("b_hand_cases: delete finding {finding} · other (ask/block) {other}");
}

// Value: protects=the checking code really catches (a test of the test); fails_when=green even for a fake judge that maps "delete -> Read" or differs only on the second call; seam=run.
#[test]
fn planted_bugs_are_caught() {
    let p = project("planted");
    let leak = |inp: &Input, m: &str, pr: &Path, l: Limits| {
        let mut v = real_judge(inp, m, pr, l);
        if inp.command.contains("rm -r") {
            v.route = Some(Route::Read);
        }
        v
    };
    assert!(run(&[1301], [0, 50, 0, 0], &leak, &p).b_pass > 0);
    let flip = std::cell::Cell::new(0u32);
    let flaky = |inp: &Input, m: &str, pr: &Path, l: Limits| {
        let mut v = real_judge(inp, m, pr, l);
        flip.set(flip.get() + 1);
        if flip.get().is_multiple_of(3) {
            v.route = Some(Route::Ask);
            v.kinds.push("planted".into());
        }
        v
    };
    assert!(run(&[1301], [30, 0, 0, 0], &flaky, &p).nondeterministic > 0);
}

// Value: protects=the four in design section 4 (no panic, time limit, same verdict, zero delete passes) and that D blocks; fails_when=any random input breaks one; seam=run(real_judge).
#[test]
fn fuzz_pure_lane() {
    let p = project("run");
    let (seeds, counts): (Vec<u64>, [usize; 4]) =
        if full() { (SEEDS.to_vec(), FULL_COUNTS) } else { (vec![SEEDS[0]], FULL_COUNTS.map(|n| n / 10)) };
    let t = Instant::now();
    let rep = run(&seeds, counts, &real_judge, &p);
    let ps = ps_lane(full());
    let secs = t.elapsed().as_secs_f64();
    // receipt = DESIGN_OLP_013_TAG_GATE section 4, matched letter for letter with the FUZZ design in its review. No command text.
    // Judged keys: panics 0 · timeouts 0 · nondeterministic_pure 0 · full true · fuzz_gen, seeds, cases_pure at the threshold values. The rest is recorded only.
    let os_key = std::env::var("OLP_FUZZ_OS_KEY").unwrap_or_else(|_| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
    let c_false_positive: usize = rep.c_routes.iter().filter(|(k, _)| !k.ends_with("Some(Read)")).map(|(_, n)| n).sum();
    let all_ok = rep.failures.is_empty() && ps.fails.is_empty();
    let out: Value = json!({
        "kind": "fuzz", "os": os_key, "head_sha": std::env::var("GITHUB_SHA").ok(),
        "run_id": std::env::var("GITHUB_RUN_ID").ok().and_then(|v| v.parse::<u64>().ok()),
        "ok": all_ok && full(),
        "counts": {
            "fuzz_gen": FUZZ_GEN, "seeds": seeds, "full": full(),
            "cases_pure": rep.inputs.iter().sum::<usize>(), "cases_ps": ps.cases,
            "panics": rep.panics + ps.panics, "timeouts": rep.timeouts + ps.timeouts,
            "nondeterministic_pure": rep.nondeterministic, "nondeterministic_ps": ps.differ,
            "max_ms": rep.max_ms.max(ps.max_ms), "digest_pure": sha256_hex(&rep.lines), "c_false_positive": c_false_positive,
            "b_pass": rep.b_pass + ps.b_pass, "d_not_blocked": rep.d_not_blocked,
            "inputs": {"A": rep.inputs[0], "B": rep.inputs[1], "C": rep.inputs[2], "D": rep.inputs[3]},
            "c_routes": rep.c_routes, "seconds": (secs * 10.0).round() / 10.0, "failures": rep.failures.len() + ps.fails.len(),
        },
    });
    if let Ok(dir) = std::env::var("OLP_FUZZ_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(Path::new(&dir).join("qual.json"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }
    eprintln!("{out}");
    assert!(rep.failures.is_empty(), "{} failures, first: {:?}", rep.failures.len(), rep.failures.iter().take(5).collect::<Vec<_>>());
    assert!(ps.fails.is_empty(), "hook-limits lane: {:?}", ps.fails);
}

#[derive(Default)]
struct PsLane {
    cases: usize,
    panics: usize,
    timeouts: usize,
    b_pass: usize,
    differ: usize,
    max_ms: u128,
    fails: Vec<String>,
}

/// Hook-limit path (design section 5, win32 only): the PowerShell parser path. Panic, time and delete passes are gated; same-verdict is recorded only.
/// 0 when not on Windows (that OS has no powershell.exe): receipt `cases_ps` 0.
fn ps_lane(full: bool) -> PsLane {
    let mut out = PsLane::default();
    if !cfg!(windows) {
        return out;
    }
    let p = project("hooklane");
    let limits = || Limits { counter: Some(std::time::Duration::from_millis(3000)), ps_parser: true };
    let n = if full { 100 } else { 10 };
    for (bucket, count) in [(0usize, n), (1usize, n)] {
        for idx in 0..count {
            let mut inp = gen(1301, bucket, idx);
            inp.tool = "PowerShell".into();
            if bucket == 1 {
                inp.command = "Remove-Item -Recurse -Force build".into();
            }
            let t = Instant::now();
            let v1 = real_judge(&inp, "auto", &p, limits());
            out.max_ms = out.max_ms.max(t.elapsed().as_millis());
            let v2 = real_judge(&inp, "auto", &p, limits());
            out.cases += 1;
            if v1.panicked || v2.panicked {
                out.panics += 1;
                out.fails.push(format!("panic: ps bucket {bucket} idx {idx}"));
            }
            if v1.timed_out {
                out.timeouts += 1;
                out.fails.push(format!("time: ps bucket {bucket} idx {idx}"));
            }
            if bucket == 1 && is_pass(v1.route) {
                out.b_pass += 1;
                out.fails.push(format!("delete passed: ps idx {idx}"));
            }
            if v1 != v2 {
                out.differ += 1;
            }
        }
    }
    out
}
