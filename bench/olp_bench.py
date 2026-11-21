"""olp_bench.py — public benchmark runner, slice 1: case/result format and runner skeleton.

    python olp_bench.py run --suite <folder> --hook <olp-hook executable> --out <results.jsonl> --work <new folder> [--judge-only]

For each case (one JSONL line in <suite>/cases.jsonl) the runner builds a fresh folder, asks the hook (Claude Code PreToolUse input),
and, only on a disposable CI runner, runs the command for real and compares the files before and after (= the answer).
Result: one JSONL line per case, sorted by id, with no time stamps and no local paths, so the same suite on the same OS gives the same bytes.

Safety (execution layer):
- runs only when all three hold: CI=true, RUNNER_TEMP is set and holds the mark file, and --work is inside RUNNER_TEMP. Otherwise it refuses
  (exit 2) before making any folder. On your own computer use --judge-only: the hook is asked, nothing is run.
- lane G (refusals, self-protection) is never run anywhere.
- HOME and USERPROFILE point at a fake folder per case; every command has a time limit.
The runner never changes the hook's answer. Standard library only."""
import argparse
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))  # python -I leaves the script folder out
import observer  # noqa: E402

LANES = ("D", "O", "S", "B", "N", "I", "G", "U")
SHELLS = ("bash", "powershell", "pwsh")
TOOLS = ("Bash", "PowerShell")
MARK_FILE = "olp-bench-disposable-runner"
MARK_TEXT = "this runner is thrown away after the job\n"
LOG_KEYS = ("verdict", "effect", "findings", "reversible", "recovery", "unchecked", "classified", "reasons", "why", "block_kind")
HOOK_TIMEOUT_S = 60
CMD_TIMEOUT_S = 30
GIT_ENV = {"GIT_AUTHOR_NAME": "bench", "GIT_AUTHOR_EMAIL": "bench@example.com", "GIT_COMMITTER_NAME": "bench",
           "GIT_COMMITTER_EMAIL": "bench@example.com", "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z", "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z"}


# Result line = OLP Record shape: one case = one line, five groups.
# Names are PROVISIONAL. Only fields already public in 0.1.x are used
# (hook verdict, count, counted or not).
RECORD_VERSION = "olp-record/provisional-0"
RECORD_GROUPS = ("action", "predicted", "knowledge", "judgment", "actual")
LANE_KIND = {"D": "file_delete", "O": "file_overwrite", "S": "same_command_other_state", "B": "no_change_expected",
             "N": "indirect_change", "I": "incident_form", "G": "refusal_or_self_protection", "U": "evasion"}


class SuiteError(Exception):
    pass


def knowledge(findings):
    """Knowledge: none (no finding) · counted (every finding counted directly) · not_counted."""
    if not findings:
        return "none"
    ok = all(isinstance(f, dict) and (f.get("count") or {}).get("source") == "counted" for f in findings)
    return "counted" if ok else "not_counted"


def reversible(d, tracked):
    """§4 answer tag ⓑ: True if every deleted/overwritten file was tracked and committed by git at setup; None if nothing changed."""
    changed = list(d.get("deleted") or []) + list(d.get("overwritten") or [])
    if not changed:
        return None
    return all(p in {"proj/" + t for t in (tracked or [])} for p in changed)


def check_record(r):
    """T10: every result line has all five groups (else the runner fails)."""
    missing = [k for k in ("record",) + RECORD_GROUPS + ("run",) if k not in r]
    if missing:
        raise SuiteError("result line is missing %s" % ", ".join(missing))


class Refused(Exception):
    pass


# ---- suite --------------------------------------------------------------------------------------------------------------

def _bad_rel(p):
    return (not isinstance(p, str) or not p or "\\" in p or p.startswith("/") or re.match(r"^[A-Za-z]:", p)
            or any(x in ("", ".", "..") for x in p.split("/")))


def check_case(c):
    if not isinstance(c, dict):
        raise SuiteError("a case must be an object")
    for k in ("id", "lane", "shell", "setup", "tool", "command"):
        if k not in c:
            raise SuiteError("case %r: missing %s" % (c.get("id"), k))
    if not isinstance(c["id"], str) or not re.fullmatch(r"[A-Z]-\d{3}", c["id"]):
        raise SuiteError("case id must look like D-001: %r" % (c["id"],))
    if not isinstance(c["lane"], str) or c["lane"] not in LANES:
        raise SuiteError("case %s: lane must be exactly one of %s" % (c["id"], "/".join(LANES)))
    if c["id"][0] != c["lane"]:
        raise SuiteError("case %s: id letter must match lane %s" % (c["id"], c["lane"]))
    if c["shell"] not in SHELLS or c["tool"] not in TOOLS or not isinstance(c["command"], str) or not c["command"]:
        raise SuiteError("case %s: shell/tool/command" % c["id"])
    s = c["setup"]
    files = s.get("files") if isinstance(s, dict) else None
    if not isinstance(files, dict) or not all(isinstance(v, str) for v in files.values()):
        raise SuiteError("case %s: setup.files must map paths to text" % c["id"])
    for p in list(files) + list(s.get("git_tracked") or []):
        if _bad_rel(p):
            raise SuiteError("case %s: path must stay inside the case folder: %r" % (c["id"], p))
    if not set(s.get("git_tracked") or []) <= set(files):
        raise SuiteError("case %s: git_tracked must be listed in files" % c["id"])
    outside = s.get("outside") or {}  # files outside the project folder: fake home or sibling folders (review condition 1)
    if not isinstance(outside, dict) or not all(isinstance(v, str) for v in outside.values()):
        raise SuiteError("case %s: setup.outside must map paths to text" % c["id"])
    for p in outside:
        if _bad_rel(p) or p.split("/")[0] not in ("home", "side") or "/" not in p:
            raise SuiteError("case %s: setup.outside paths start with home/ or side/: %r" % (c["id"], p))
    for k, v in sorted((s.get("vars") or {}).items()):  # extra variables; values point inside the fake root only
        if (not re.fullmatch(r"[A-Z][A-Z0-9_]*", k) or k in ("HOME", "USERPROFILE", "PATH") or not isinstance(v, str)
                or not v.startswith("{root}/") or _bad_rel(v[len("{root}/"):])):
            raise SuiteError("case %s: setup.vars %s must be {root}/<path inside the fake root>" % (c["id"], k))
    inc = c.get("incident")  # condition 3: source wins - a case that comes from a public incident is lane I, its pair too
    if (inc is not None) != (c["lane"] == "I"):
        raise SuiteError("case %s: an incident reference goes with lane I and only lane I" % c["id"])
    if inc is not None and (not isinstance(inc, dict) or not isinstance(inc.get("ref"), str) or not inc["ref"]
                            or inc.get("form") not in ("verbatim", "reconstructed")):
        raise SuiteError("case %s: incident needs ref and form verbatim|reconstructed" % c["id"])
    if inc is not None and ("text_n" not in inc or not (inc["text_n"] is None or (type(inc["text_n"]) is int and inc["text_n"] >= 0))):
        raise SuiteError("case %s: incident.text_n = files the command text names if read as written (int >= 0, or null)" % c["id"])
    if inc is not None and "pair" in inc and inc["pair"] is not True:
        raise SuiteError("case %s: incident.pair is true or absent (the pair = same incident, variable resolved as intended)" % c["id"])


def load_suite(folder):
    cases, seen = [], set()
    with open(os.path.join(folder, "cases.jsonl"), encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            if not line.strip():
                continue
            try:
                c = json.loads(line)
            except ValueError as e:
                raise SuiteError("line %d: %s" % (n, e))
            check_case(c)
            if c["id"] in seen:
                raise SuiteError("duplicate case id %s (one case, one lane)" % c["id"])
            seen.add(c["id"])
            cases.append(c)
    return sorted(cases, key=lambda c: c["id"])


def suite_version(folder):
    with open(os.path.join(folder, "SUITE_VERSION"), encoding="utf-8") as f:
        return f.read().strip()


# ---- safety -------------------------------------------------------------------------------------------------------------

def _inside(child, parent):
    c, p = os.path.normcase(os.path.realpath(child)), os.path.normcase(os.path.realpath(parent))
    return c.startswith(p.rstrip(os.sep) + os.sep)


def sandbox_ok(env, work):
    """(ok, why). All three marks: CI=true · RUNNER_TEMP with the mark file · work folder inside RUNNER_TEMP."""
    if env.get("CI") != "true":
        return False, "CI is not true"
    rt = env.get("RUNNER_TEMP") or ""
    if not rt or not os.path.isdir(rt):
        return False, "RUNNER_TEMP is not set"
    try:
        with open(os.path.join(rt, MARK_FILE), encoding="utf-8") as f:
            if f.read() != MARK_TEXT:
                return False, "mark file has other text"
    except OSError:
        return False, "no mark file in RUNNER_TEMP"
    if not _inside(work, rt):
        return False, "work folder is not inside RUNNER_TEMP"
    return True, ""


_MARKS = sandbox_ok  # run_command checks the marks with this, so a test that swaps sandbox_ok still runs nothing for real


def exec_plan(case, exec_ok):
    if case["lane"] == "G":
        return "skip:lane_G"
    if not exec_ok:
        return "skip:judge_only"
    return "run"


# ---- before / after ------------------------------------------------------------------------------------------------------

snapshot = observer.snapshot
diff = observer.diff


# ---- running a process with a time limit that ends the whole tree ----------------------------------------------------------

def run_tree(argv, cwd, env, timeout, stdin=None):
    """{"exit", "timed_out", "stdout"}. On time out the whole process tree is ended (not only the first process):
    POSIX = own session + kill the group · Windows = own process group + taskkill /T /F."""
    kw = {"cwd": cwd, "env": env, "stdin": subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
          "stdout": subprocess.PIPE, "stderr": subprocess.PIPE}
    if os.name == "nt":
        kw["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
    else:
        kw["start_new_session"] = True
    p = subprocess.Popen(argv, **kw)
    try:
        out, _ = p.communicate(input=stdin, timeout=timeout)
        return {"exit": p.returncode, "timed_out": False, "stdout": out}
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            subprocess.run(["taskkill", "/F", "/T", "/PID", str(p.pid)], capture_output=True)
        else:
            import signal
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except OSError:
                pass
        p.kill()
        p.communicate()
        return {"exit": None, "timed_out": True, "stdout": b""}


# ---- one case ------------------------------------------------------------------------------------------------------------

def _env(root, case):
    keep = {k: os.environ[k] for k in ("PATH", "SYSTEMROOT", "SystemRoot", "COMSPEC", "PATHEXT", "TEMP", "TMP", "LANG") if k in os.environ}
    home = os.path.join(root, "home")
    tmp = os.path.join(root, "tmp")  # temp files land inside the observed fake root, not in the runner's own temp
    keep.update({"HOME": home, "USERPROFILE": home, "OLP_BENCH": "1", "TEMP": tmp, "TMP": tmp, "TMPDIR": tmp})
    for k, v in sorted((case["setup"].get("vars") or {}).items()):
        keep[k] = root.replace(os.sep, "/") + v[len("{root}"):]
    return keep


def _write(base, files):
    for rel, text in sorted(files.items()):
        p = os.path.join(base, *rel.split("/"))
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(text.encode("utf-8"))


def build(case, root):
    """Make the fake root of one case and return the environment the hook and the command get."""
    proj = os.path.join(root, "proj")
    for d in (proj, os.path.join(root, "home"), os.path.join(root, "side"), os.path.join(root, "tmp")):
        os.makedirs(d)
    _write(proj, case["setup"]["files"])
    _write(root, case["setup"].get("outside") or {})
    env = _env(root, case)
    tracked = case["setup"].get("git_tracked") or []
    if tracked:
        genv = dict(env, **GIT_ENV)
        for args in (["init", "-q"], ["add", "--"] + tracked, ["-c", "commit.gpgsign=false", "commit", "-q", "-m", "setup"]):
            subprocess.run(["git", "-c", "init.defaultBranch=main"] + args, cwd=proj, env=genv,
                           check=True, capture_output=True, timeout=CMD_TIMEOUT_S)
    return env


def ask_hook(hook, case, folder, env):
    stdin = json.dumps({"session_id": "olp-bench", "transcript_path": "", "cwd": folder, "hook_event_name": "PreToolUse",
                        "tool_name": case["tool"], "tool_input": {"command": case["command"]}, "permission_mode": "default"})
    argv = [sys.executable, "-I", hook] if hook.endswith(".py") else [hook]
    r = run_tree(argv, folder, env, HOOK_TIMEOUT_S, stdin=stdin.encode("utf-8"))
    if r["timed_out"]:
        return {"exit": None, "decision": "hook_timeout", "log": None}
    decision = "pass"
    out = r["stdout"].decode("utf-8", errors="replace").strip()
    if out:
        try:
            decision = (json.loads(out).get("hookSpecificOutput") or {}).get("permissionDecision") or "pass"
        except (ValueError, AttributeError):
            decision = "unreadable"
    if r["exit"] == 2:
        decision = "deny"
    return {"exit": r["exit"], "decision": decision, "log": read_log(folder)}


def read_log(folder):
    last = None
    try:
        with open(os.path.join(folder, ".olp", "hook-log.jsonl"), encoding="utf-8") as f:
            for line in f:
                try:
                    e = json.loads(line)
                except ValueError:
                    continue
                if isinstance(e, dict) and e.get("event") == "decision":
                    last = e
    except OSError:
        return None
    return None if last is None else {k: last[k] for k in LOG_KEYS if k in last}


def run_command(case, root, env):
    """Run the case command in proj/ and let the independent observer compare the whole fake root before and after."""
    argv = {"bash": ["bash", "-c", case["command"]],
            "powershell": ["powershell", "-NoProfile", "-NonInteractive", "-Command", case["command"]],
            "pwsh": ["pwsh", "-NoProfile", "-NonInteractive", "-Command", case["command"]]}[case["shell"]]
    ok, why = _MARKS(os.environ, root)
    if not ok:
        raise Refused("execution layer refused in run_command (%s)" % why)
    before = observer.snapshot(root)
    r = run_tree(argv, os.path.join(root, "proj"), env, CMD_TIMEOUT_S)
    if r["timed_out"]:
        return {"executed": True, "why": "command_timeout"}
    return dict({"executed": True, "exit": r["exit"]}, **observer.diff(before, observer.snapshot(root)))


def _sha(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def _prediction(c, h):
    findings = list((h.get("log") or {}).get("findings") or [])
    action = {"case": c["id"], "lane": c["lane"], "kind": LANE_KIND[c["lane"]], "shell": c["shell"]}
    if c["lane"] == "I":  # review condition 1: the text's count, so the table can tell "said the text" from "said the real target"
        action["text_n"] = c["incident"]["text_n"]
    judgment = {"verdict": h["decision"], "hook_exit": h["exit"]}
    know = knowledge(findings)
    if h.get("log") is None:  # no decision line in the hook log: not "knew nothing", but "cannot tell"
        know, judgment["why"] = "not_counted", "no_hook_log"
    return {"record": RECORD_VERSION, "action": action,
            "predicted": [{"kind": f.get("kind"), "n": (f.get("count") or {}).get("n"), "unit": (f.get("count") or {}).get("unit")}
                          for f in findings if isinstance(f, dict)],
            "knowledge": know, "judgment": judgment}


def _counts(a, c):  # T11: counts only, no file names in the result line
    if "deleted" not in a:
        return a
    return {"executed": True, "exit": a["exit"], "deleted_n": len(a["deleted"]), "overwritten_n": len(a["overwritten"]),
            "created_n": len(a["created"]), "changed": a["changed"], "outside_deleted_n": a["outside_deleted_n"],
            "outside_overwritten_n": a["outside_overwritten_n"], "reversible": reversible(a, c["setup"].get("git_tracked"))}


def _dump(rows):
    return "".join(json.dumps(r, ensure_ascii=False, sort_keys=True) + chr(10) for r in rows).encode("utf-8")


def run(suite, hook, out, work, judge_only, env=None):
    env = os.environ if env is None else env
    cases = load_suite(suite)
    exec_ok = False
    if not judge_only:
        ok, why = sandbox_ok(env, work)
        if not ok:
            raise Refused("execution layer refused (%s) - on your own computer use --judge-only" % why)
        exec_ok = True
    if os.path.exists(work):
        raise Refused("work folder already exists - give a new one")
    with tempfile.TemporaryDirectory() as t:  # T12: the observer's own miss count, published next to the table
        oc = observer.self_check(t)
    meta = {"suite": suite_version(suite), "suite_sha256": _sha(os.path.join(suite, "cases.jsonl")),
            "runner": _sha(os.path.abspath(__file__))[:12], "observer": _sha(os.path.abspath(observer.__file__))[:12],
            "hook_sha256": _sha(hook), "os": platform.system(), "observer_planted": oc["planted"], "observer_missed": oc["missed"],
            "observer_limits": list(observer.LIMITS), "observer_skipped": oc["skipped"]}
    os.makedirs(work)
    # phase 1 - ask the hook about every case, then seal all predictions before the first command runs (T13)
    preds, envs = [], {}
    for c in cases:
        root = os.path.join(work, c["id"])
        envs[c["id"]] = build(c, root)
        preds.append(_prediction(c, ask_hook(hook, c, os.path.join(root, "proj"), envs[c["id"]])))
    sealed = _dump(preds)
    with open(out + ".sealed", "wb") as f:
        f.write(sealed)
    meta["sealed_sha256"] = hashlib.sha256(sealed).hexdigest()
    # phase 2 - run (only on a disposable runner), the observer gives the answer
    rows, ran = [], 0
    for c, p in zip(cases, preds):
        plan = exec_plan(c, exec_ok)
        if plan == "run":
            actual = _counts(run_command(c, os.path.join(work, c["id"]), envs[c["id"]]), c)
            ran += 1
        else:
            actual = {"executed": False, "why": plan.split(":", 1)[1]}
        row = dict(p, actual=actual, run=meta)
        check_record(row)
        rows.append(row)
    if _sha(hook) != meta["hook_sha256"]:  # a case touched the hook: every later answer is suspect, so write no results
        raise Refused("hook file changed during the run - results not written")
    if _sha(out + ".sealed") != meta["sealed_sha256"]:  # the predictions changed after sealing
        raise Refused("sealed predictions changed during the run - results not written")
    with open(out, "wb") as f:
        f.write(_dump(rows))
    return len(rows), ran


def main(argv=None):
    ap = argparse.ArgumentParser(prog="olp_bench")
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run")
    for k in ("--suite", "--hook", "--out", "--work"):
        r.add_argument(k, required=True)
    r.add_argument("--judge-only", action="store_true")
    a = ap.parse_args(argv)
    try:
        n, ran = run(a.suite, a.hook, a.out, a.work, a.judge_only)
    except (Refused, SuiteError) as e:
        sys.stderr.write("olp_bench: %s\n" % e)
        return 2
    print("results: %d cases (executed %d, not executed %d) -> %s" % (n, ran, n - ran, os.path.basename(a.out)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
