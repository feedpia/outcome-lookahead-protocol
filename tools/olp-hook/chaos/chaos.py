"""Chaos test for the olp-hook binary: 78 broken or hostile inputs.

The hook never runs commands; it only classifies them. A cell passes when there is no panic, the exit code is
0 or 2, and no delete/overwrite input ever comes back as exit 0 with empty output (a silent pass, a "leak").

Run: python -I tools/olp-hook/chaos/chaos.py --hook <olp-hook(.exe)> --out qual.json --os <key> --work <folder>
Writes one release-gate receipt (qual.json) and exits 0 only when all 78 cells pass.
"""
import argparse
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import sys
import threading
import time

EXPECTED_CELLS = 78
EXE = ""
WORK = ""
rows = []


def summarize(rows):
    """Counts for the receipt and whether the run passes (total 78, all ok, no panic, no leak)."""
    leaks = sum(1 for r in rows if r["risky"] and r["exit"] == 0 and r["decision"] == "pass")
    counts = {"total": len(rows), "ok": sum(1 for r in rows if r["ok"]), "panics": sum(1 for r in rows if r["panic"]), "leaks": leaks}
    ok = counts["total"] == EXPECTED_CELLS and counts["ok"] == EXPECTED_CELLS and counts["panics"] == 0 and counts["leaks"] == 0
    return counts, ok


def fresh(name):
    p = os.path.join(WORK, name)
    if os.path.exists(p):
        shutil.rmtree(p)
    os.makedirs(os.path.join(p, "build"))
    for n in ("a.log", "b.tmp", "c.txt"):
        open(os.path.join(p, "build", n), "w").write("x")
    open(os.path.join(p, "keep.txt"), "w").write("old")
    return p


def env_for(project):
    e = {k: os.environ[k] for k in ("SYSTEMROOT", "PATH", "TEMP", "TMP") if k in os.environ}
    e["HOME"] = os.path.join(WORK, "_home")
    e["USERPROFILE"] = e["HOME"]
    os.makedirs(e["HOME"], exist_ok=True)
    if project:
        e["CLAUDE_PROJECT_DIR"] = project
    return e


def call(stdin_bytes, project, cwd=None, timeout=60):
    t = time.time()
    p = subprocess.run([EXE], input=stdin_bytes, capture_output=True, env=env_for(project), cwd=cwd or project or WORK, timeout=timeout)
    return p.returncode, p.stdout.decode("utf-8", "replace"), p.stderr.decode("utf-8", "replace"), round((time.time() - t) * 1000)


def decision(out):
    try:
        return json.loads(out.strip())["hookSpecificOutput"]["permissionDecision"]
    except Exception:
        return ""


def pre(cwd, tool, tool_input, mode="default"):
    return json.dumps({"session_id": "s", "transcript_path": "t", "cwd": cwd, "hook_event_name": "PreToolUse",
                       "tool_name": tool, "tool_input": tool_input, "permission_mode": mode, "tool_use_id": "toolu_chaos"}).encode()


def check(cell, case, res, risky):
    code, out, err, ms = res
    panic = "panicked" in err or "internal error" in err
    d = decision(out)
    silent = code == 0 and d == ""
    ok = (not panic) and code in (0, 2) and not (risky and silent)
    rows.append({"cell": cell, "case": case, "exit": code, "decision": d or ("block" if code == 2 else "pass"), "ms": ms,
                 "panic": panic, "risky": risky, "ok": ok, "err1": err.strip().splitlines()[0][:90] if err.strip() else ""})


def rm_cmd(p):
    return pre(p, "Bash", {"command": "rm -rf build"})


def run_cases():
    # 1 empty stdin
    p = fresh("p1")
    check("1 empty stdin", "0 bytes", call(b"", p), True)
    check("1 empty stdin", "whitespace only", call(b"  \n\t", p), True)

    # 2 broken JSON
    p = fresh("p2")
    for case, b in [("half object", b'{"tool_name":"Bash","tool_input":{"command":"rm -rf build"'), ("null", b"null"), ("array", b"[]"),
                    ("number", b"42"), ("invalid UTF-8", b'{"tool_name":"Bash\xff\xfe"}'), ("NUL bytes", b"\x00" * 64),
                    ("random bytes", os.urandom(4096)), ("BOM + valid", b"\xef\xbb\xbf" + rm_cmd(p)),
                    ("no tool_name", json.dumps({"cwd": p, "hook_event_name": "PreToolUse", "tool_input": {"command": "rm -rf build"}}).encode()),
                    ("no cwd", json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}, "permission_mode": "default"}).encode()),
                    ("command is a number", pre(p, "Bash", {"command": 123})), ("command is null", pre(p, "Bash", {"command": None})),
                    ("tool_input is a string", pre(p, "Bash", "rm -rf build")), ("tool_name is an object", json.dumps({"cwd": p, "hook_event_name": "PreToolUse", "tool_name": {"x": 1}, "tool_input": {}}).encode()),
                    ("unknown event", json.dumps({"cwd": p, "hook_event_name": "Bogus", "tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}).encode()),
                    ("deep nesting 100k", b"[" * 100000 + b"]" * 100000),
                    ("Write path is a number", pre(p, "Write", {"file_path": 7, "content": "x"})),
                    ("mode unknown value", pre(p, "Bash", {"command": "rm -rf build"}, mode="weird"))]:
        check("2 broken JSON", case, call(b, p), True)

    # 3 very long command (1 MB)
    p = fresh("p3")
    big = "a" * (1 << 20)
    check("3 1MB command", "rm -rf build + 1MB arg", call(pre(p, "Bash", {"command": "rm -rf build " + big}), p), True)
    check("3 1MB command", "1MB of '; rm -rf build' repeats", call(pre(p, "Bash", {"command": "; ".join(["rm -rf build"] * 80000)}), p), True)
    check("3 1MB command", "1MB echo then rm", call(pre(p, "Bash", {"command": "echo " + big + " && rm -rf build"}), p), True)
    check("3 1MB command", "1MB Write content (overwrite)", call(pre(p, "Write", {"file_path": os.path.join(p, "keep.txt"), "content": big}), p), True)
    check("3 1MB command", "1MB read (cat x)", call(pre(p, "Bash", {"command": "cat " + big}), p), False)

    # 4 Korean / space / unicode paths
    for name in ("프로젝트 폴더", "sp ace", "üñí 😀 ğ", "ㄱ　全角"):
        p = fresh("p4_" + name)
        os.makedirs(os.path.join(p, "빌드 폴더"))
        open(os.path.join(p, "빌드 폴더", "파일 1.txt"), "w").write("x")
        check("4 unicode path", name + ": rm build", call(rm_cmd(p), p), True)
        check("4 unicode path", name + ": rm '빌드 폴더'", call(pre(p, "Bash", {"command": 'rm -rf "빌드 폴더"'}), p), True)
        check("4 unicode path", name + ": Write overwrite", call(pre(p, "Write", {"file_path": os.path.join(p, "keep.txt"), "content": "new"}), p), True)

    # 5 working folder that does not exist
    ghost = os.path.join(WORK, "no_such_dir_ghost")
    check("5 missing cwd", "cwd missing, project env missing too", call(pre(ghost, "Bash", {"command": "rm -rf build"}), ghost, cwd=WORK), True)
    check("5 missing cwd", "cwd missing, project env unset", call(pre(ghost, "Bash", {"command": "rm -rf build"}), None, cwd=WORK), True)
    p = fresh("p5")
    check("5 missing cwd", "cwd missing, project exists", call(pre(ghost, "Bash", {"command": "rm -rf build"}), p), True)
    check("5 missing cwd", "cwd missing, Write overwrite", call(pre(ghost, "Write", {"file_path": os.path.join(p, "keep.txt"), "content": "n"}), p), True)
    check("5 missing cwd", "cwd = '' ", call(pre("", "Bash", {"command": "rm -rf build"}), p), True)
    check("5 missing cwd", "drive that does not exist (Q:)", call(pre("Q:/nowhere", "Bash", {"command": "rm -rf build"}), p), True)

    # 6 config file missing / broken
    p = fresh("p6")
    cfgdir = os.path.join(p, ".olp")
    check("6 config", "no .olp folder", call(rm_cmd(p), p), True)
    for case, content in [("truncated JSON", "{"), ("empty file", ""), ("wrong types", '{"hook_budget_ms":"soon","quiet":7}'),
                          ("budget 0 ms", '{"hook_budget_ms":0}'), ("huge numbers", '{"hook_budget_ms":1e400}'), ("binary junk", None)]:
        os.makedirs(cfgdir, exist_ok=True)
        with open(os.path.join(cfgdir, "hook-config.json"), "wb") as f:
            f.write(os.urandom(512) if content is None else content.encode())
        check("6 config", case, call(rm_cmd(p), p), True)
    os.remove(os.path.join(cfgdir, "hook-config.json"))
    os.makedirs(os.path.join(cfgdir, "hook-config.json"))
    check("6 config", "config path is a folder", call(rm_cmd(p), p), True)
    shutil.rmtree(os.path.join(cfgdir, "hook-config.json"))
    check("6 config", "config removed again (recovers)", call(rm_cmd(p), p), True)

    # 7 20 calls at once on one project
    p = fresh("p7")
    res = [None] * 20

    def worker(i):
        cmd = pre(p, "Bash", {"command": "rm -rf build"}) if i % 2 == 0 else pre(p, "Write", {"file_path": os.path.join(p, "keep.txt"), "content": str(i)})
        res[i] = call(cmd, p)
    ts = [threading.Thread(target=worker, args=(i,)) for i in range(20)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    for i, r in enumerate(res):
        check("7 20 at once", f"#{i:02d} {'rm' if i % 2 == 0 else 'Write'}", r, True)
    logs = []
    for dp, _, fs in os.walk(os.path.join(p, ".olp")):
        logs += [os.path.join(dp, f) for f in fs if f.endswith(".jsonl")]
    bad_lines = 0
    total_lines = 0
    for lf in logs:
        for line in open(lf, encoding="utf-8", errors="replace"):
            if line.strip():
                total_lines += 1
                try:
                    json.loads(line)
                except Exception:
                    bad_lines += 1
    check("7 20 at once", f"log lines parse ({total_lines} lines in {len(logs)} files, {bad_lines} broken)", (0 if bad_lines == 0 else 1, "", "", 0), False)

    # 8 kill in the middle
    p = fresh("p8")
    kill_codes = []
    for i in range(10):
        pr = subprocess.Popen([EXE], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env_for(p), cwd=p)
        payload = pre(p, "Bash", {"command": "rm -rf build " + "a" * (1 << 20)})
        try:
            pr.stdin.write(payload[: len(payload) // (2 + i)])  # half-written input, then kill
            pr.stdin.flush()
        except OSError:
            pass
        time.sleep(0.01 * i)
        pr.kill()
        pr.wait()
        kill_codes.append(pr.returncode)
        for s in (pr.stdin, pr.stdout, pr.stderr):
            try:
                s.close()
            except Exception:
                pass
    # killed after the input is fully written (while it decides)
    for i in range(10):
        pr = subprocess.Popen([EXE], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env_for(p), cwd=p)
        try:
            pr.stdin.write(pre(p, "Bash", {"command": "; ".join(["rm -rf build"] * 20000)}))
            pr.stdin.close()
        except OSError:
            pass
        time.sleep(0.005 * i)
        pr.kill()
        pr.wait()
        kill_codes.append(pr.returncode)
        pr.stdout.close()
        pr.stderr.close()
    # a half-written input closed early (no kill): stdin EOF in the middle
    check("8 kill mid-call", "stdin closed half-way", call(rm_cmd(p)[:40], p), True)
    check("8 kill mid-call", f"next call after 20 kills (kill exit codes: {sorted(set(kill_codes))})", call(rm_cmd(p), p), True)
    check("8 kill mid-call", "next Write after kills", call(pre(p, "Write", {"file_path": os.path.join(p, "keep.txt"), "content": "n"}), p), True)
    bad = 0
    for dp, _, fs in os.walk(os.path.join(p, ".olp")):
        for f in fs:
            if f.endswith(".jsonl"):
                for line in open(os.path.join(dp, f), encoding="utf-8", errors="replace"):
                    if line.strip():
                        try:
                            json.loads(line)
                        except Exception:
                            bad += 1
    check("8 kill mid-call", f"log lines parse after kills ({bad} broken)", (0 if bad == 0 else 1, "", "", 0), False)
    check("8 kill mid-call", "files untouched (build has 3, keep.txt=old)",
          (0 if len(os.listdir(os.path.join(p, "build"))) == 3 and open(os.path.join(p, "keep.txt")).read() == "old" else 1, "", "", 0), False)


def main(argv=None):
    global EXE, WORK
    ap = argparse.ArgumentParser(description="olp-hook chaos test (78 cells) -> release-gate receipt")
    ap.add_argument("--hook", required=True, help="path to the olp-hook binary under test")
    ap.add_argument("--out", required=True, help="receipt file to write (qual.json)")
    ap.add_argument("--os", required=True, help="platform key, e.g. win32-x64")
    ap.add_argument("--work", required=True, help="scratch folder for the fake projects (created, may be wiped)")
    ap.add_argument("--head", default="", help="commit sha when GITHUB_SHA is not set")
    a = ap.parse_args(argv)
    # Case names hold non-ASCII text; a legacy console code page must not turn a passing run into a crash.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, ValueError):
            pass
    EXE = os.path.abspath(a.hook)
    WORK = os.path.abspath(a.work)
    os.makedirs(WORK, exist_ok=True)
    run_cases()
    counts, ok = summarize(rows)
    run_id = os.environ.get("GITHUB_RUN_ID")
    receipt = {
        "kind": "chaos",
        "os": a.os,
        "head_sha": os.environ.get("GITHUB_SHA") or a.head,
        "run_id": int(run_id) if run_id and run_id.isdigit() else None,
        "ok": ok,
        "counts": counts,
        "hook_sha256": hashlib.sha256(open(EXE, "rb").read()).hexdigest(),
        "made_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "rows": rows,
    }
    os.makedirs(os.path.dirname(os.path.abspath(a.out)) or ".", exist_ok=True)
    with open(a.out, "w", encoding="utf-8") as f:
        json.dump(receipt, f, ensure_ascii=False, indent=1)
    for r in rows:
        print(("OK  " if r["ok"] else "FAIL"), r["cell"], "|", r["case"][:60], "| exit", r["exit"], "|", r["decision"], "|", r["ms"], "ms")
    print("TOTAL", counts["total"], "OK", counts["ok"], "PANICS", counts["panics"], "LEAKS", counts["leaks"], "->", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
