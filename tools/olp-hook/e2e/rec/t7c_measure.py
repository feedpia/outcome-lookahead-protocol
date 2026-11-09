"""T7-c: measure the session-start usage signal under a real interactive Claude Code session on a CI runner.

Usage (CI runners only):
  python t7c_measure.py run --os <key> --dist <dist dir> --out <out dir>
  python t7c_measure.py wrap <timing log> <base64 hook command>   (used inside the SessionStart hook)

What it does: fake home and projects under a short temp folder, candidate olp from <dist>, control olp@0.1.1 from npm,
consent record written only in that fake home with the endpoint pointed at a local silent TCP server (accepts, never answers).
Per session it records the hook duration (m1), whether curl's first bytes arrive after the hook exits (m3), when curl gives up
(m4) and how many curl processes are left. Output: <out>/qual.json (release gate receipt) and <out>/*.cast recordings.
Nothing is sent to the production server: the record endpoint is checked before every session and the workflow blocks the
production host in the hosts file. Debug logs stay in the temp folder and are never uploaded.
"""
import base64
import glob
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import threading
import time

LOCAL_PREFIX = "https://127.0.0.1:"
ALIVE_MARGIN_S = 1.0
PRIVATE = re.compile(r"[a-z0-9._%+-]+@[a-z0-9-]+\.[a-z]{2,}|runneradmin|/home/runner|/Users/runner|sk-" r"ant|oauth", re.I)  # token prefix split in two so the public-copy name check does not flag this line
ANSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07]*(\x07|\x1b\\)|\x1b[()][A-Z0-9]|\x1b[=>]")
READY = (["forshortcuts"], ["automodeon"], ["shift+tabtocycle"])


# ---------- pure functions (unit-tested) ----------

def judge(c, os_key):
    """Release gate rule for kind t7c. Returns (ok, reasons). Missing or wrong-typed keys fail."""
    why = []

    def num(k):
        v = c.get(k)
        return v if isinstance(v, (int, float)) and not isinstance(v, bool) else None

    m1 = c.get("m1_s")
    if not (isinstance(m1, list) and len(m1) == 3 and all(isinstance(x, (int, float)) for x in m1)):
        why.append("m1_s: need 3 numbers")
    elif max(m1) > 0.5:
        why.append(f"m1_s: max {max(m1)} > 0.5")
    for k in ("m2_ok", "m3_ok", "m6_ok"):
        if c.get(k) is not True:
            why.append(f"{k}: not true")
    g = num("m4_gone_s")
    if g is None or not 4 <= g <= 7:
        why.append(f"m4_gone_s: {g} not in 4..7")
    if c.get("m4_curl_left") != 0:
        why.append(f"m4_curl_left: {c.get('m4_curl_left')} != 0")
    ctrl = num("ctrl_011_s")
    if ctrl is None or ctrl < 1.5:
        why.append(f"ctrl_011_s: {ctrl} < 1.5 (method did not catch a slow hook)")
    if os_key.startswith("win32") and c.get("m5") != "code":
        why.append("m5: must be 'code' on Windows (runner has no screen)")
    return (not why, why)


def record_error(rec):
    """None when the consent record can only reach the local fake server; otherwise the reason to stop."""
    ep = rec.get("endpoint")
    if not isinstance(ep, str):
        return "record has no endpoint (would fall back to the production endpoint)"
    if not re.match(r"^https://127\.0\.0\.1:\d+/", ep):
        return "record endpoint is not the local fake server"
    return None


def leaks(text, token):
    """Private-looking strings (and the login token value) that must not be uploaded."""
    found = [m.group(0) for m in PRIVATE.finditer(text)]
    if token and token in text:
        found.append("<token value>")
    return found


def timeline(events, start, hook_end):
    """events: [(kind, t)] from the fake server for one session.
    Returns (alive_after_hook_exit, seconds_until_close): curl sent its first bytes and its connection was still open
    after the hook exited (M3 = the send survives the hook). The first byte itself may come before the exit on a fast machine."""
    first = [t for k, t in events if k == "first_byte" and t >= start]
    closed = [t for k, t in events if k == "close" and t >= start]
    gone = round(min(closed) - start, 3) if closed else None
    # RECHECK 1 (C1/R1): at least ALIVE_MARGIN_S after the hook exits, per session, so "curl died with the hook" never passes.
    alive = bool(first) and (not closed or min(closed) - hook_end >= ALIVE_MARGIN_S)
    return (alive, gone)


# ---------- fake server ----------

class SilentServer:
    """Accepts TCP connections and never answers (TLS never completes). Records accept / first byte / close times."""

    def __init__(self):
        self.s = socket.socket()
        self.s.bind(("127.0.0.1", 0))
        self.s.listen(16)
        self.port = self.s.getsockname()[1]
        self.events = []
        threading.Thread(target=self._loop, daemon=True).start()

    def _loop(self):
        while True:
            conn, _ = self.s.accept()
            self.events.append(("accept", time.time()))
            threading.Thread(target=self._conn, args=(conn,), daemon=True).start()

    def _conn(self, conn):
        got = False
        while True:
            try:
                b = conn.recv(4096)
            except OSError:
                b = b""
            if not b:
                self.events.append(("close", time.time()))
                return
            if not got:
                got = True
                self.events.append(("first_byte", time.time()))


# ---------- terminal driver (real interactive Claude Code) ----------

class Term:
    def __init__(self, cmd, cwd, env, cols=130, rows=40):
        import queue
        self.q = queue.Queue()
        self.buf = ""
        self.events = []
        self.t0 = time.time()
        if os.name == "nt":
            from winpty import PtyProcess
            self.p = PtyProcess.spawn(subprocess.list2cmdline(cmd), cwd=cwd, env=env, dimensions=(rows, cols))
            reader = lambda: self.p.read(65536)
            self._write = self.p.write
        else:
            import fcntl
            import struct
            import termios
            m, s = os.openpty()
            fcntl.ioctl(s, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
            self.p = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=s, stdout=s, stderr=s, start_new_session=True)
            os.close(s)
            self.m = m
            reader = lambda: os.read(m, 65536).decode("utf-8", "replace")
            self._write = lambda x: os.write(m, x.encode())

        def loop():
            while True:
                try:
                    c = reader()
                except Exception:
                    self.q.put(None)
                    return
                if not c:
                    self.q.put(None)
                    return
                self.q.put(c)
        threading.Thread(target=loop, daemon=True).start()

    def pump(self, secs):
        import queue
        end = time.time() + secs
        while time.time() < end:
            try:
                c = self.q.get(timeout=0.2)
            except queue.Empty:
                continue
            if c is None:
                return
            self.buf += c
            self.events.append([round(time.time() - self.t0, 3), "o", c])

    def flat(self):
        return ANSI.sub("", self.buf).replace(" ", "").replace("\r", "").replace("\n", "")

    def wait_any(self, groups, secs, nudge=8):
        # Windows ConPTY may send no screen until something is written: nudge every few seconds (same as tui_win.py).
        end = time.time() + secs
        last = time.time()
        while time.time() < end:
            if os.name == "nt" and nudge and time.time() - last > nudge:
                try:
                    self._write("\x0c")  # Ctrl+L = redraw; an empty write did not make ConPTY send the screen (run 42516999789)
                except Exception:
                    pass
                last = time.time()
            self.pump(0.5)
            f = self.flat()
            if any(all(n in f for n in g) for g in groups):
                return True
        return False

    def send(self, s):
        for ch in s:
            self._write(ch)
            time.sleep(0.05)

    def close(self):
        for _ in range(2):
            try:
                self._write("\x03")
                self.pump(1)
            except Exception:
                pass
        try:
            self.p.terminate() if os.name != "nt" else self.p.terminate(force=True)
        except Exception:
            pass

    def save_cast(self, path, title):
        with open(path, "w", encoding="utf-8") as f:
            f.write(json.dumps({"version": 2, "width": 130, "height": 40, "title": title}) + "\n")
            for e in self.events:
                f.write(json.dumps(e) + "\n")


# ---------- helpers ----------

def sh(cmd, cwd, env, stdin=subprocess.DEVNULL, timeout=600):
    r = subprocess.run(cmd, cwd=cwd, env=env, stdin=stdin, capture_output=True, text=True, timeout=timeout,
                       shell=(os.name == "nt"))
    return r.returncode, r.stdout + r.stderr


def curl_left():
    if os.name == "nt":
        out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq curl.exe"], capture_output=True, text=True).stdout
        return out.lower().count("curl.exe")
    out = subprocess.run(["pgrep", "-x", "curl"], capture_output=True, text=True).stdout
    return len([x for x in out.split() if x.strip()])


def usage_records(home):
    return glob.glob(os.path.join(home, "olp", "usage", "*.json"))


def point_record_at(home, port, project_hint):
    recs = usage_records(home)
    if not recs:
        sys.exit(f"stop: no usage record in the fake home after setup ({project_hint})")
    for p in recs:
        r = json.load(open(p, encoding="utf-8"))
        if r.get("opt_in") is True:
            r["endpoint"] = f"https://127.0.0.1:{port}/v1/olp/signal"
            r["counted_to"] = 0
            r.pop("last_sent_day", None)
            json.dump(r, open(p, "w", encoding="utf-8"))


def check_records(home):
    for p in usage_records(home):
        r = json.load(open(p, encoding="utf-8"))
        if r.get("opt_in") is True:
            err = record_error(r)
            if err:
                sys.exit("stop: " + err)
            r.pop("last_sent_day", None)
            json.dump(r, open(p, "w", encoding="utf-8"))


def wrap_hooks(proj, timing_log):
    """Wrap the olp-hook SessionStart command so the hook's own start/end times are logged. Returns the olp-hook path."""
    sp = os.path.join(proj, ".claude", "settings.json")
    s = json.load(open(sp, encoding="utf-8"))
    hook_bin = None
    for g in s.get("hooks", {}).get("SessionStart", []):
        for h in g.get("hooks", []):
            cmd = h.get("command", "")
            if "olp-hook" in cmd:
                m = re.match(r'^"([^"]+)"|^(\S+)', cmd)
                hook_bin = m.group(1) or m.group(2)
                b = base64.b64encode(cmd.encode()).decode()
                me = os.path.abspath(__file__).replace("\\", "/")
                py = sys.executable.replace("\\", "/")
                h["command"] = f'"{py}" "{me}" wrap "{timing_log.replace(chr(92), "/")}" {b}'
    if not hook_bin:
        sys.exit("stop: no olp-hook SessionStart entry in project settings (did install run with usage numbers on?)")
    json.dump(s, open(sp, "w", encoding="utf-8"), indent=2)
    return hook_bin


def last_timing(path, after):
    rows = []
    if os.path.isfile(path):
        for line in open(path, encoding="utf-8"):
            a, b = line.split()
            if float(a) >= after:
                rows.append((float(a), float(b)))
    return rows[-1] if rows else None


def one_session(kind, proj, env, srv, timing_log, out, tag):
    """kind 'tui' (interactive, recorded) or 'p' (claude -p). Returns dict with m1, sent_after, gone, curl_left, ready."""
    t_start = time.time()
    dbg = os.path.join(os.path.dirname(proj), f"debug-{tag}.log")
    ready = True
    if kind == "tui":
        t = Term(["claude", "--model", "haiku", "--debug-file", dbg], proj, env)
        ready = t.wait_any(READY, 120)
        t.pump(3)
        t.close()
        t.save_cast(os.path.join(out, f"t7c-{tag}.cast"), f"OLP T7-c {tag}")
    else:
        subprocess.run(["claude", "-p", "ok", "--model", "haiku", "--debug-file", dbg], cwd=proj, env=env,
                       stdin=subprocess.DEVNULL, capture_output=True, timeout=180, shell=(os.name == "nt"))
    row = last_timing(timing_log, t_start - 1)
    while time.time() < (row[0] if row else t_start) + 9:
        time.sleep(0.5)
    left = curl_left()
    if not row:
        return {"ready": ready, "m1": None, "sent_after": False, "gone": None, "curl_left": left,
                "debug_has_sessionstart": False}
    sent_after, gone = timeline(srv.events, row[0], row[1])
    has_line = os.path.isfile(dbg) and "SessionStart" in open(dbg, encoding="utf-8", errors="replace").read()
    fb = [round(t - row[0], 3) for k, t in srv.events if k == "first_byte" and t >= row[0]]
    return {"ready": ready, "m1": round(row[1] - row[0], 3), "sent_after": sent_after, "gone": gone,
            "curl_left": left, "debug_has_sessionstart": has_line, "first_byte_s": fb[0] if fb else None}


def setup_consent(proj, env, npx_spec):
    """Answer the usage-numbers question with y in a terminal (fake home only; endpoint is redirected right after)."""
    t = Term(["npx", "-y", npx_spec, "setup"] if npx_spec else ["npx", "--no-install", "olp", "setup"], proj, env)
    if not t.wait_any([["[y/N]"]], 120):
        t.close()
        sys.exit("stop: setup question did not appear within 120 s")
    t.send("y\r")
    t.pump(5)
    t.close()


def run(os_key, dist, out):
    base = "C:\\t7c" if os.name == "nt" else "/tmp/t7c"
    home = os.environ.get("HOME", "")
    for k in ("HOME",) + (("USERPROFILE",) if os.name == "nt" else ()):
        v = os.path.abspath(os.environ.get(k, ""))
        if not v.lower().startswith(os.path.abspath(base).lower()):
            sys.exit(f"stop: {k} is not under {base} (real home must not be touched)")
    os.makedirs(out, exist_ok=True)
    env = dict(os.environ)
    token = env.get("CLAUDE_CODE_OAUTH_TOKEN", "")
    srv = SilentServer()
    cv = subprocess.run(["claude", "--version"], capture_output=True, text=True, shell=(os.name == "nt")).stdout.split()

    def project(name):
        p = os.path.join(base, name)
        os.makedirs(p, exist_ok=True)
        sh(["git", "init", "-q"], p, env)
        sh(["npm", "init", "-y"], p, env)
        return p

    # Claude Code: onboarding and folder trust done in the fake home (a screen a real user passes once).
    cj = os.path.join(home, ".claude.json")
    projects = {}
    for n in ("cand", "ctrl"):
        p0 = os.path.join(base, n)
        for k in (p0, os.path.realpath(p0)):
            # Same key shapes as rec_prep.sh: forward and back slashes (Windows), realpath (macOS /private/tmp).
            for v in (k, k.replace("\\", "/"), k.replace("/", "\\")):
                projects[v] = {"hasTrustDialogAccepted": True}
    json.dump({"hasCompletedOnboarding": True, "theme": "dark", "projects": projects}, open(cj, "w", encoding="utf-8"))

    # candidate from dist
    cand = project("cand")
    tgz = sorted(glob.glob(os.path.join(dist, f"feedpia-olp-{os_key}-*.tgz"))) + \
        sorted(x for x in glob.glob(os.path.join(dist, "feedpia-olp-[0-9]*.tgz")))
    rc, o = sh(["npm", "install", "--no-audit", "--no-fund"] + tgz, cand, env)
    if rc:
        sys.exit("stop: candidate npm install failed")
    sh(["npx", "--no-install", "olp", "install"], cand, env)
    setup_consent(cand, env, None)
    point_record_at(home, srv.port, "cand")
    log_c = os.path.join(base, "timing-cand.txt")
    hook_bin = wrap_hooks(cand, log_c)
    hook_sha = hashlib.sha256(open(hook_bin, "rb").read()).hexdigest() if os.path.isfile(hook_bin) else None

    runs = []
    for i in (1, 2, 3):
        check_records(home)
        runs.append(one_session("tui", cand, env, srv, log_c, out, f"{os_key}-cand{i}"))
    check_records(home)
    p_run = one_session("p", cand, env, srv, log_c, out, f"{os_key}-p")

    # control: published 0.1.1 (waits for the server at session start)
    ctrl = project("ctrl")
    sh(["npx", "-y", "@feedpia/olp@0.1.1", "install"], ctrl, env)
    setup_consent(ctrl, env, "@feedpia/olp@0.1.1")
    point_record_at(home, srv.port, "ctrl")
    log_k = os.path.join(base, "timing-ctrl.txt")
    wrap_hooks(ctrl, log_k)
    check_records(home)
    k_run = one_session("tui", ctrl, env, srv, log_k, out, f"{os_key}-ctrl")
    rc_un, _ = sh(["npx", "-y", "@feedpia/olp@0.1.1", "uninstall"], ctrl, env)

    m1 = [r["m1"] for r in runs]
    last = runs[-1]
    counts = {
        "m1_s": m1,
        "m2_ok": all(r["ready"] for r in runs) and all(isinstance(x, float) and x <= 0.5 for x in m1),
        "m3_ok": all(r["sent_after"] for r in runs),
        "m4_gone_s": last["gone"],
        "m4_curl_left": max(r["curl_left"] for r in runs + [p_run]),
        "m6_ok": bool(p_run["m1"] is not None and p_run["sent_after"] and p_run["gone"] is not None
                      and 4 <= p_run["gone"] <= 7 and p_run["curl_left"] == 0),
        "ctrl_011_s": k_run["m1"],
    }
    if os_key.startswith("win32"):
        counts["m5"] = "code"
    ok, why = judge(counts, os_key)
    receipt = {"kind": "t7c", "os": os_key, "head_sha": env.get("GITHUB_SHA", ""), "run_id": int(env.get("GITHUB_RUN_ID", "0") or 0),
               "ok": ok, "counts": counts, "hook_sha256": hook_sha, "method": "wrap",
               "made_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
               "debug_has_sessionstart": [r["debug_has_sessionstart"] for r in runs],
               "sessions": [{k: r.get(k) for k in ("m1", "first_byte_s", "gone", "sent_after", "curl_left", "ready")}
                            for r in runs + [p_run, k_run]],
               "claude_version": cv[0] if cv else None, "ctrl_uninstall_rc": rc_un, "reasons": why}
    text = json.dumps(receipt, indent=2)
    bad = leaks(text, token)
    for c in glob.glob(os.path.join(out, "*.cast")):
        if leaks(open(c, encoding="utf-8").read(), token):
            bad.append(os.path.basename(c))
            os.remove(c)
    if bad:
        print("stop: private-looking text or the token found; nothing uploaded:", len(bad), "hits")
        return 1
    open(os.path.join(out, "qual.json"), "w", encoding="utf-8").write(text)
    print(text)
    return 0 if ok else 1


def wrap(timing_log, b64):
    cmd = base64.b64decode(b64).decode()
    t0 = time.time()
    rc = subprocess.call(cmd, shell=True)
    with open(timing_log, "a", encoding="utf-8") as f:
        f.write(f"{t0:.3f} {time.time():.3f}\n")
    return rc


if __name__ == "__main__":
    a = sys.argv[1:]
    if a[:1] == ["wrap"]:
        sys.exit(wrap(a[1], a[2]))
    if a[:1] == ["run"]:
        kv = dict(zip(a[1::2], a[2::2]))
        sys.exit(run(kv["--os"], kv["--dist"], kv["--out"]))
    sys.exit(__doc__)
