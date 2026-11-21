"""test_olp_bench.py — public benchmark runner, slice 1 (case/result format, runner skeleton, T1 · T2 · T8 · T9).
Run: python -m unittest discover -s bench -p "test_*.py"   (standard library only)
No test here runs a case command: the execution layer only runs on a disposable CI runner (T1)."""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import olp_bench as b  # noqa: E402

SUITE = os.path.join(HERE, "suite")

FAKE_HOOK = r'''
import json, os, sys, time
d = json.load(sys.stdin)
cmd = d["tool_input"]["command"]
os.makedirs(".olp", exist_ok=True)
ask = cmd.startswith("rm ")
with open(os.path.join(".olp", "hook-log.jsonl"), "a", encoding="utf-8") as f:
    f.write(json.dumps({"ts": int(time.time() * 1000), "event": "decision", "verdict": "ask" if ask else "pass",
                        "findings": [{"kind": "file_delete", "count": {"n": 3, "source": "counted", "unit": "file"}}] if ask else []}) + "\n")
if ask:
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask", "permissionDecisionReason": "x"}}))
'''


def case(**over):
    c = {"id": "D-x", "lane": "D", "shell": "bash", "setup": {"files": {"src/a.txt": "a\n"}}, "tool": "Bash", "command": "rm -rf src"}
    c.update(over)
    return c


class Tmp:
    def __init__(self):
        self.t = tempfile.TemporaryDirectory()
        self.dir = self.t.name

    def close(self):
        self.t.cleanup()


def write_suite(folder, cases):
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, "cases.jsonl"), "w", encoding="utf-8", newline="\n") as f:
        for c in cases:
            f.write(json.dumps(c, ensure_ascii=False) + "\n")
    with open(os.path.join(folder, "SUITE_VERSION"), "w", encoding="utf-8", newline="\n") as f:
        f.write("test-suite\n")


class SuiteShape(unittest.TestCase):  # T9 and the case format
    def setUp(self):
        self.tmp = Tmp()

    def tearDown(self):
        self.tmp.close()

    def load(self, cases):
        write_suite(self.tmp.dir, cases)
        return b.load_suite(self.tmp.dir)

    def test_shipped_suite_loads(self):
        cs = b.load_suite(SUITE)
        self.assertGreaterEqual(len([c for c in cs if c["lane"] == "D"]), 3)
        self.assertTrue(any(c["lane"] == "G" for c in cs))

    def test_t9_duplicate_id_fails(self):
        with self.assertRaises(b.SuiteError):
            self.load([case(), case()])

    def test_t9_case_in_two_lanes_fails(self):
        with self.assertRaises(b.SuiteError):
            self.load([case(lane=["D", "U"])])

    def test_unknown_lane_or_missing_field_fails(self):
        for bad in (case(lane="Q"), {k: v for k, v in case().items() if k != "command"}, case(shell="zsh"), case(tool="Write")):
            with self.subTest(bad):
                with self.assertRaises(b.SuiteError):
                    self.load([bad])

    def test_setup_path_must_stay_inside_case_folder(self):
        for p in ("../x.txt", "/etc/x", "C:/x", "a/../../x"):
            with self.subTest(p):
                with self.assertRaises(b.SuiteError):
                    self.load([case(setup={"files": {p: "x"}})])


class Sandbox(unittest.TestCase):  # T1: all three marks, or the execution layer refuses
    def setUp(self):
        self.tmp = Tmp()
        self.rt = os.path.join(self.tmp.dir, "runner_temp")
        os.makedirs(self.rt)
        with open(os.path.join(self.rt, b.MARK_FILE), "w", encoding="utf-8") as f:
            f.write(b.MARK_TEXT)
        self.work = os.path.join(self.rt, "work")
        os.makedirs(self.work)

    def tearDown(self):
        self.tmp.close()

    def test_all_three_marks_ok(self):
        ok, why = b.sandbox_ok({"CI": "true", "RUNNER_TEMP": self.rt}, self.work)
        self.assertTrue(ok, why)

    def test_t1_any_mark_missing_refuses(self):
        envs = ({"RUNNER_TEMP": self.rt}, {"CI": "true"}, {"CI": "1", "RUNNER_TEMP": self.rt})
        for env in envs:
            with self.subTest(env):
                self.assertFalse(b.sandbox_ok(env, self.work)[0])
        os.remove(os.path.join(self.rt, b.MARK_FILE))
        self.assertFalse(b.sandbox_ok({"CI": "true", "RUNNER_TEMP": self.rt}, self.work)[0])

    def test_t1_work_folder_outside_runner_temp_refuses(self):
        out = os.path.join(self.tmp.dir, "elsewhere")
        os.makedirs(out)
        self.assertFalse(b.sandbox_ok({"CI": "true", "RUNNER_TEMP": self.rt}, out)[0])
        self.assertFalse(b.sandbox_ok({"CI": "true", "RUNNER_TEMP": self.rt}, os.path.join(self.rt, "..", "elsewhere"))[0])

    def test_t1_cli_refuses_without_judge_only_and_runs_nothing(self):
        out = os.path.join(self.tmp.dir, "r.jsonl")
        hook = os.path.join(self.tmp.dir, "fake_hook.py")
        with open(hook, "w", encoding="utf-8") as f:
            f.write(FAKE_HOOK)
        env = {k: v for k, v in os.environ.items() if k not in ("CI", "RUNNER_TEMP")}
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "olp_bench.py"), "run", "--suite", SUITE, "--hook", hook,
                            "--out", out, "--work", os.path.join(self.tmp.dir, "w")], env=env, capture_output=True, text=True)
        self.assertEqual(p.returncode, 2, p.stderr)
        self.assertIn("execution layer refused", p.stderr)
        self.assertFalse(os.path.exists(out))
        self.assertFalse(os.path.exists(os.path.join(self.tmp.dir, "w")))


class Plan(unittest.TestCase):  # T8: lane G is never executed
    def test_t8_lane_g_skips_execution_even_in_sandbox(self):
        self.assertEqual(b.exec_plan(case(lane="G", id="G-1", command="rm -rf /"), exec_ok=True), "skip:lane_G")

    def test_judge_only_skips_execution(self):
        self.assertEqual(b.exec_plan(case(), exec_ok=False), "skip:judge_only")

    def test_sandbox_lane_d_runs(self):
        self.assertEqual(b.exec_plan(case(), exec_ok=True), "run")



class JudgeOnlyRun(unittest.TestCase):  # T2 (judge-only on this PC) + result line shape
    def setUp(self):
        self.tmp = Tmp()
        self.hook = os.path.join(self.tmp.dir, "fake_hook.py")
        with open(self.hook, "w", encoding="utf-8") as f:
            f.write(FAKE_HOOK)

    def tearDown(self):
        self.tmp.close()

    def run_cli(self, out, work, hook=None):
        env = {k: v for k, v in os.environ.items() if k not in ("CI", "RUNNER_TEMP")}
        return subprocess.run([sys.executable, "-I", os.path.join(HERE, "olp_bench.py"), "run", "--suite", SUITE, "--hook", hook or self.hook,
                               "--out", out, "--work", work, "--judge-only"], env=env, capture_output=True, text=True)

    def test_t2_two_runs_byte_identical(self):
        o1, o2 = os.path.join(self.tmp.dir, "1.jsonl"), os.path.join(self.tmp.dir, "2.jsonl")
        p1 = self.run_cli(o1, os.path.join(self.tmp.dir, "w1"))
        p2 = self.run_cli(o2, os.path.join(self.tmp.dir, "w2"))
        self.assertEqual((p1.returncode, p2.returncode), (0, 0), p1.stderr + p2.stderr)
        with open(o1, "rb") as f1, open(o2, "rb") as f2:
            self.assertEqual(f1.read(), f2.read())

    def test_result_line_shape_and_no_local_paths(self):
        out = os.path.join(self.tmp.dir, "r.jsonl")
        work = os.path.join(self.tmp.dir, "w")
        p = self.run_cli(out, work)
        self.assertEqual(p.returncode, 0, p.stderr)
        with open(out, encoding="utf-8") as f:
            text = f.read()
        rows = [json.loads(x) for x in text.splitlines()]
        ids = [r["action"]["case"] for r in rows]
        self.assertEqual(ids, sorted(ids))
        # OLP Record shape: five groups (names are provisional)
        for r in rows:
            self.assertEqual(r["record"], b.RECORD_VERSION)
            self.assertTrue(b.RECORD_VERSION.endswith("provisional-0"))
            self.assertEqual(sorted(r), sorted(b.RECORD_GROUPS + ("record", "run")))
        d = next(r for r in rows if r["action"]["lane"] == "D")
        self.assertEqual(d["action"]["kind"], "file_delete")
        self.assertEqual(d["judgment"]["verdict"], "ask")
        self.assertEqual(d["predicted"], [{"kind": "file_delete", "n": 3, "unit": "file"}])
        self.assertEqual(d["knowledge"], "counted")
        self.assertEqual(d["actual"], {"executed": False, "why": "judge_only"})
        self.assertEqual(sorted(d["run"]), ["hook_sha256", "observer", "observer_limits", "observer_missed", "observer_planted",
                                             "observer_skipped", "os", "runner",
                                             "sealed_sha256", "suite", "suite_sha256"])
        g = next(r for r in rows if r["action"]["lane"] == "G")
        self.assertEqual(g["actual"], {"executed": False, "why": "lane_G"})
        bl = next(r for r in rows if r["action"]["lane"] == "B")
        self.assertEqual((bl["judgment"]["verdict"], bl["predicted"], bl["knowledge"]), ("pass", [], "none"))
        for needle in (work, work.replace("\\", "/"), self.tmp.dir, os.path.expanduser("~")):
            self.assertNotIn(needle, text)
        # T11: no command text or path (case commands and setup file names are not in the result line)
        for c in b.load_suite(SUITE):
            self.assertNotIn(c["command"], text)
            for rel in c["setup"]["files"]:
                self.assertNotIn(rel, text)
        self.assertIn("results", p.stdout)

    def test_t10_record_missing_a_group_fails_the_runner(self):
        good = {"record": b.RECORD_VERSION, "action": {}, "predicted": [], "knowledge": "none", "judgment": {}, "actual": {}, "run": {}}
        b.check_record(good)
        for k in b.RECORD_GROUPS:
            with self.subTest(k):
                bad = {kk: v for kk, v in good.items() if kk != k}
                with self.assertRaises(b.SuiteError):
                    b.check_record(bad)

    def test_reversible_tag(self):  # reversible = every changed file was tracked by git
        self.assertTrue(b.reversible({"deleted": ["proj/a"], "overwritten": []}, ["a", "b"]))
        self.assertFalse(b.reversible({"deleted": ["proj/a", "home/a"], "overwritten": []}, ["a"]))
        self.assertIsNone(b.reversible({"deleted": [], "overwritten": []}, ["a"]))

    def test_knowledge_is_not_counted_when_any_finding_is_not_counted(self):
        self.assertEqual(b.knowledge([{"count": {"n": 3, "source": "counted"}}, {"count": {"n": None}}]), "not_counted")
        self.assertEqual(b.knowledge([{"count": {"n": 3, "source": "counted"}}]), "counted")
        self.assertEqual(b.knowledge([]), "none")

    def test_case_folders_are_fresh_and_inside_work(self):
        work = os.path.join(self.tmp.dir, "w")
        self.assertEqual(self.run_cli(os.path.join(self.tmp.dir, "r.jsonl"), work).returncode, 0)
        names = sorted(os.listdir(work))
        self.assertEqual(names, sorted(c["id"] for c in b.load_suite(SUITE)))

    def test_existing_work_folder_is_refused(self):  # never reuse or clean a folder we did not make
        work = os.path.join(self.tmp.dir, "w")
        os.makedirs(work)
        p = self.run_cli(os.path.join(self.tmp.dir, "r.jsonl"), work)
        self.assertEqual(p.returncode, 2)
        self.assertIn("already exists", p.stderr)


class HookUnchanged(unittest.TestCase):  # review memo: a case that touches the hook would spoil every later case
    def test_hook_file_changed_during_run_is_refused(self):
        tmp = Tmp()
        try:
            hook = os.path.join(tmp.dir, "self_editing_hook.py")
            with open(hook, "w", encoding="utf-8") as f:
                f.write(FAKE_HOOK + "\nopen(__file__, 'a', encoding='utf-8').write('# touched\\n')\n")
            out = os.path.join(tmp.dir, "r.jsonl")
            env = {k: v for k, v in os.environ.items() if k not in ("CI", "RUNNER_TEMP")}
            p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "olp_bench.py"), "run", "--suite", SUITE, "--hook", hook,
                                "--out", out, "--work", os.path.join(tmp.dir, "w"), "--judge-only"], env=env, capture_output=True, text=True)
            self.assertEqual(p.returncode, 2, p.stderr)
            self.assertIn("hook file changed", p.stderr)
            self.assertFalse(os.path.exists(out))
        finally:
            tmp.close()


class TimeLimitKillsTree(unittest.TestCase):  # review memo: the time limit must end the children too, not only the shell
    def test_grandchild_does_not_outlive_the_limit(self):
        tmp = Tmp()
        try:
            mark = os.path.join(tmp.dir, "late.txt")
            grand = "import time; time.sleep(3); open(%r, 'w').write('x')" % mark
            child = "import subprocess, sys, time; subprocess.Popen([sys.executable, '-c', %r]); time.sleep(30)" % grand
            rc = b.run_tree([sys.executable, "-c", child], cwd=tmp.dir, env=dict(os.environ), timeout=1)
            self.assertIsNone(rc["exit"])
            self.assertTrue(rc["timed_out"])
            import time
            time.sleep(4)
            self.assertFalse(os.path.exists(mark))
        finally:
            tmp.close()

    def test_normal_exit_code(self):
        rc = b.run_tree([sys.executable, "-c", "import sys; print('hi'); sys.exit(3)"], cwd=HERE, env=dict(os.environ), timeout=10)
        self.assertEqual((rc["exit"], rc["timed_out"], rc["stdout"].strip()), (3, False, b"hi"))


@unittest.skipUnless(os.environ.get("OLP_BENCH_HOOK") and os.path.isfile(os.environ.get("OLP_BENCH_HOOK", "")), "set OLP_BENCH_HOOK to a built olp-hook")
class RealHookJudgeOnly(unittest.TestCase):  # the real hook answers the shipped D cases (judge only)
    def test_real_hook_counts_lane_d(self):
        tmp = Tmp()
        try:
            out = os.path.join(tmp.dir, "r.jsonl")
            env = {k: v for k, v in os.environ.items() if k not in ("CI", "RUNNER_TEMP")}
            p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "olp_bench.py"), "run", "--suite", SUITE,
                                "--hook", os.environ["OLP_BENCH_HOOK"], "--out", out, "--work", os.path.join(tmp.dir, "w"), "--judge-only"],
                               env=env, capture_output=True, text=True)
            self.assertEqual(p.returncode, 0, p.stderr)
            rows = {r["action"]["case"]: r for r in map(json.loads, open(out, encoding="utf-8").read().splitlines())}
            self.assertEqual(rows["D-001"]["judgment"]["verdict"], "ask")
            self.assertEqual((rows["D-001"]["knowledge"], rows["D-001"]["predicted"][0]["n"]), ("counted", 3))
            self.assertEqual(rows["B-001"]["judgment"]["verdict"], "pass")
            self.assertEqual(rows["G-001"]["judgment"]["verdict"], "deny")
            self.assertEqual(rows["G-001"]["knowledge"], "not_counted")
        finally:
            tmp.close()


def _in_sandbox():
    rt = os.environ.get("RUNNER_TEMP") or ""
    return bool(rt) and b.sandbox_ok(os.environ, os.path.join(rt, "olp-bench-work"))[0]


@unittest.skipUnless(_in_sandbox() and os.environ.get("OLP_BENCH_HOOK"), "only on a disposable CI runner with the mark file (T1)")
class SandboxRun(unittest.TestCase):  # the execution layer, end to end: case -> hook -> real run -> before/after
    def test_execution_layer_end_to_end(self):
        rt = os.environ["RUNNER_TEMP"]
        out, work = os.path.join(rt, "olp-bench-r.jsonl"), os.path.join(rt, "olp-bench-work")
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "olp_bench.py"), "run", "--suite", SUITE,
                            "--hook", os.environ["OLP_BENCH_HOOK"], "--out", out, "--work", work], capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        rows = {r["action"]["case"]: r for r in map(json.loads, open(out, encoding="utf-8").read().splitlines())}
        self.assertEqual((rows["D-001"]["actual"]["deleted_n"], rows["D-001"]["actual"]["reversible"]), (3, False))
        self.assertEqual(rows["D-003"]["actual"]["reversible"], False)  # the deleted build/ was not tracked by git
        self.assertFalse(rows["B-001"]["actual"]["changed"])
        self.assertFalse(rows["D-004"]["actual"]["changed"])
        self.assertEqual(rows["G-001"]["actual"], {"executed": False, "why": "lane_G"})


if __name__ == "__main__":
    unittest.main()
