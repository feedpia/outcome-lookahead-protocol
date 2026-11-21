"""test_bench_slice1c.py — first real run pieces (first real run): independent observer over the whole fake root
(T12, T14), lane I with source-wins, predictions sealed before any command runs (T13), and the table
(file level / task level / refused·rephrased / observer misses) with one regression case per mismatch.
Run: python -m unittest discover -s bench -p "test_*.py"   (standard library only, no case command runs here)."""
import ast
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import observer as obs  # noqa: E402
import olp_bench as b  # noqa: E402
import olp_bench_report as rep  # noqa: E402

SUITE = os.path.join(HERE, "suite")


def _w(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)


TABLE_WORDS = ("File level", "Task level", "refused", "Rephrased", "not applicable", "Observer", "exploratory result",
               "Observer limits", "recreate_same_content", "Incident forms", "as the text reads", "No hook answer", "command time out")
HANGUL = re.compile("[\uac00-\ud7a3]")

class Observer(unittest.TestCase):  # condition 1: the whole fake root, not only the project folder
    def test_sees_changes_outside_the_project(self):
        with tempfile.TemporaryDirectory() as root:
            _w(os.path.join(root, "proj", "a.txt"), "a")
            _w(os.path.join(root, "home", ".ssh", "id"), "k")
            _w(os.path.join(root, "side", "repo", "x.txt"), "x")
            before = obs.snapshot(root)
            os.remove(os.path.join(root, "home", ".ssh", "id"))
            _w(os.path.join(root, "side", "repo", "x.txt"), "y")
            d = obs.diff(before, obs.snapshot(root))
            self.assertEqual(d["deleted"], ["home/.ssh/id"])
            self.assertEqual(d["overwritten"], ["side/repo/x.txt"])
            self.assertEqual((d["outside_deleted_n"], d["outside_overwritten_n"]), (1, 1))

    def test_skips_only_the_hook_log_and_git_inside_the_project(self):
        with tempfile.TemporaryDirectory() as root:
            _w(os.path.join(root, "proj", ".olp", "hook-log.jsonl"), "{}")
            _w(os.path.join(root, "proj", ".git", "HEAD"), "ref")
            _w(os.path.join(root, "home", ".olp", "keep"), "k")
            self.assertEqual(sorted(obs.snapshot(root)), ["home/.olp/keep"])

    def test_broken_link_does_not_stop_the_snapshot(self):  # review condition 4
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "proj"))
            try:
                os.symlink(os.path.join(root, "nowhere"), os.path.join(root, "proj", "dead"))
            except (OSError, NotImplementedError):
                self.skipTest("symlinks not allowed")
            self.assertEqual(list(obs.snapshot(root)), ["proj/dead"])

    def test_t12_planted_changes_are_all_seen(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = obs.self_check(tmp)
        self.assertGreaterEqual(r["planted"], 6)
        self.assertEqual(r["missed"], 0)
        self.assertIn("outside", " ".join(r["kinds"]))
        self.assertIn("same_size", " ".join(r["kinds"]))

    def test_t12_a_blind_observer_is_counted_as_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = obs.self_check(tmp, snap=lambda root: {})
        self.assertEqual(r["missed"], r["planted"])

    def test_t14_observer_uses_no_olp_code(self):
        with open(os.path.join(HERE, "observer.py"), encoding="utf-8") as f:
            tree = ast.parse(f.read())
        names = set()
        for n in ast.walk(tree):
            if isinstance(n, ast.Import):
                names |= {a.name.split(".")[0] for a in n.names}
            elif isinstance(n, ast.ImportFrom):
                names.add((n.module or "").split(".")[0])
        self.assertLessEqual(names, {"hashlib", "os"})


class LaneI(unittest.TestCase):  # condition 3: source wins — a case with an incident number is lane I, pairs too
    def base(self, **over):
        c = {"id": "I-001", "lane": "I", "shell": "bash", "tool": "Bash", "command": "rm -rf \"$HOME\"",
             "setup": {"files": {"a.txt": "a\n"}}, "incident": {"ref": "https://example.invalid/incident/1", "form": "reconstructed", "text_n": None}}
        c.update(over)
        return c

    def test_lane_i_case_loads(self):
        b.check_case(self.base())

    def test_lane_i_needs_an_incident(self):
        c = self.base()
        del c["incident"]
        with self.assertRaises(b.SuiteError):
            b.check_case(c)

    def test_incident_number_outside_lane_i_fails(self):
        with self.assertRaises(b.SuiteError):
            b.check_case(self.base(id="S-001", lane="S"))

    def test_incident_form_is_verbatim_or_reconstructed(self):
        with self.assertRaises(b.SuiteError):
            b.check_case(self.base(incident={"ref": "x#1", "form": "guess", "text_n": 1}))

    def test_lane_i_needs_the_text_count(self):  # review condition 1: how many files the text names, read as written
        c = self.base()
        del c["incident"]["text_n"]
        with self.assertRaises(b.SuiteError):
            b.check_case(c)
        with self.assertRaises(b.SuiteError):
            b.check_case(self.base(incident={"ref": "x#1", "form": "reconstructed", "text_n": -1}))

    def test_pair_tag_is_true_or_absent(self):  # pair = same incident number + a pair tag (incident.pair)
        b.check_case(self.base(incident={"ref": "x#1", "form": "reconstructed", "text_n": 1, "pair": True}))
        with self.assertRaises(b.SuiteError):
            b.check_case(self.base(incident={"ref": "x#1", "form": "reconstructed", "text_n": 1, "pair": "yes"}))

    def test_outside_files_only_under_home_or_side(self):
        b.check_case(self.base(setup={"files": {"a.txt": "a"}, "outside": {"home/n.txt": "n", "side/r/x": "x"}}))
        for bad in ("proj/a", "etc/passwd", "../x", "home"):
            with self.subTest(bad), self.assertRaises(b.SuiteError):
                b.check_case(self.base(setup={"files": {}, "outside": {bad: "x"}}))

    def test_env_values_stay_in_the_fake_root(self):
        b.check_case(self.base(setup={"files": {}, "vars": {"WORK_TMP": "{root}/proj/tmp"}}))
        for bad in ({"HOME": "{root}/home"}, {"X": "/tmp"}, {"X": "{root}/../x"}):
            with self.subTest(bad), self.assertRaises(b.SuiteError):
                b.check_case(self.base(setup={"files": {}, "vars": bad}))


class Layout(unittest.TestCase):  # one case = one fake root: proj (cwd) · home (HOME) · side (sibling folders)
    def test_build_makes_fake_root(self):
        c = {"id": "D-009", "lane": "D", "shell": "bash", "tool": "Bash", "command": "true",
             "setup": {"files": {"src/a.txt": "a"}, "outside": {"home/n.txt": "n", "side/r/x": "x"}, "vars": {"WORK_TMP": "{root}/proj/tmp"}}}
        with tempfile.TemporaryDirectory() as tmp:
            root = os.path.join(tmp, "D-009")
            env = b.build(c, root)
            self.assertTrue(os.path.isfile(os.path.join(root, "proj", "src", "a.txt")))
            self.assertTrue(os.path.isfile(os.path.join(root, "home", "n.txt")))
            self.assertTrue(os.path.isfile(os.path.join(root, "side", "r", "x")))
            self.assertEqual(env["HOME"], os.path.join(root, "home"))
            self.assertEqual(env["USERPROFILE"], os.path.join(root, "home"))
            for k in ("TEMP", "TMP", "TMPDIR"):  # review memo: temp files land inside the observed fake root
                self.assertEqual(env[k], os.path.join(root, "tmp"))
            self.assertTrue(os.path.isdir(os.path.join(root, "tmp")))
            self.assertEqual(env["WORK_TMP"], os.path.join(root, "proj", "tmp").replace("\\", "/"))


FAKE_HOOK = r'''
import json, os, sys
d = json.load(sys.stdin)
os.makedirs(".olp", exist_ok=True)
ask = d["tool_input"]["command"].startswith("rm ")
with open(os.path.join(".olp", "hook-log.jsonl"), "a", encoding="utf-8") as f:
    f.write(json.dumps({"event": "decision", "verdict": "ask" if ask else "pass",
                        "findings": [{"kind": "file_delete", "count": {"n": 2, "source": "counted", "unit": "file"}}] if ask else []}) + "\n")
if ask:
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "PreToolUse", "permissionDecision": "ask", "permissionDecisionReason": "x"}}))
'''


class Seal(unittest.TestCase):  # T13: every prediction is written and fingerprinted before the first command runs
    def test_sealed_before_any_command_and_fingerprint_in_every_line(self):
        with tempfile.TemporaryDirectory() as tmp:
            hook = os.path.join(tmp, "h.py")
            _w(hook, FAKE_HOOK)
            out = os.path.join(tmp, "r.jsonl")
            seen = []
            real = b.run_command

            def spy(case, root, env):
                seen.append(os.path.isfile(out + ".sealed"))
                return {"executed": True, "exit": 0, "deleted": [], "overwritten": [], "created": [], "changed": False,
                        "outside_deleted_n": 0, "outside_overwritten_n": 0}
            real_ok = b.sandbox_ok
            b.run_command, b.sandbox_ok = spy, (lambda env, work: (True, ""))
            try:
                n, ran = b.run(SUITE, hook, out, os.path.join(tmp, "w"), judge_only=False, env={})
            finally:
                b.run_command, b.sandbox_ok = real, real_ok
            self.assertTrue(ran > 0 and all(seen), seen)
            with open(out + ".sealed", "rb") as f:
                sealed = f.read()
            self.assertNotIn(b"\"actual\"", sealed)
            rows = [json.loads(x) for x in open(out, encoding="utf-8")]
            for r in rows:
                self.assertEqual(r["run"]["sealed_sha256"], hashlib.sha256(sealed).hexdigest())
                self.assertEqual(r["run"]["suite_sha256"], b._sha(os.path.join(SUITE, "cases.jsonl")))
                self.assertIn("observer_missed", r["run"])

    def test_sealed_file_changed_during_the_run_is_refused(self):  # review memo: hash the sealed file again at the end
        with tempfile.TemporaryDirectory() as tmp:
            hook = os.path.join(tmp, "h.py")
            _w(hook, FAKE_HOOK)
            out = os.path.join(tmp, "r.jsonl")
            real, real_ok = b.run_command, b.sandbox_ok

            def spy(case, root, env):
                with open(out + ".sealed", "ab") as f:
                    f.write(b"x")
                return {"executed": True, "exit": 0, "deleted": [], "overwritten": [], "created": [], "changed": False,
                        "outside_deleted_n": 0, "outside_overwritten_n": 0}
            b.run_command, b.sandbox_ok = spy, (lambda env, work: (True, ""))
            try:
                with self.assertRaises(b.Refused):
                    b.run(SUITE, hook, out, os.path.join(tmp, "w"), judge_only=False, env={})
            finally:
                b.run_command, b.sandbox_ok = real, real_ok
            self.assertFalse(os.path.exists(out))

    def test_run_command_checks_the_marks_itself(self):  # review condition 5: a test that forgets to swap run_command runs nothing
        c = {"id": "B-009", "lane": "B", "shell": "bash", "tool": "Bash", "command": "true", "setup": {"files": {}}}
        real_ok = b.sandbox_ok
        b.sandbox_ok = lambda env, work: (True, "")
        try:
            with tempfile.TemporaryDirectory() as tmp:
                root = os.path.join(tmp, "B-009")
                env = b.build(c, root)
                with self.assertRaises(b.Refused):
                    b.run_command(c, root, env)
        finally:
            b.sandbox_ok = real_ok

    def test_no_hook_log_is_not_counted_with_a_reason(self):  # a missing hook log is never "knew nothing"
        c = {"id": "D-009", "lane": "D", "shell": "bash", "tool": "Bash", "command": "x", "setup": {"files": {}}}
        p = b._prediction(c, {"exit": 0, "decision": "pass", "log": None})
        self.assertEqual((p["knowledge"], p["judgment"]["why"]), ("not_counted", "no_hook_log"))

    def test_test_only_switch_is_not_reachable_from_the_command_line(self):
        p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "olp_bench.py"), "run", "--help"], capture_output=True, text=True)
        self.assertNotIn("exec_ok", p.stdout)


def row(case, lane, verdict, pred, know, deleted=0, over=0, executed=True, out_d=0, text_n=None, why="lane_G"):
    a = {"executed": executed}
    if why == "command_timeout":
        a = {"executed": True, "why": "command_timeout"}
    elif executed:
        a.update({"exit": 0, "deleted_n": deleted, "overwritten_n": over, "created_n": 0, "changed": bool(deleted or over),
                  "outside_deleted_n": out_d, "outside_overwritten_n": 0, "reversible": None})
    else:
        a["why"] = why
    act = {"case": case, "lane": lane, "kind": b.LANE_KIND[lane], "shell": "bash"}
    if lane == "I":
        act["text_n"] = text_n
    return {"record": b.RECORD_VERSION, "action": act,
            "predicted": [{"kind": "file_delete", "n": pred, "unit": "file"}] if pred is not None else [],
            "knowledge": know, "judgment": {"verdict": verdict, "hook_exit": 0}, "actual": a,
            "run": {"observer_planted": 7, "observer_missed": 0, "observer_limits": list(obs.LIMITS), "os": "Linux", "suite": "t", "runner": "r", "hook_sha256": "h",
                    "sealed_sha256": "s", "suite_sha256": "c"}}


class Table(unittest.TestCase):  # file level (denominator = files) / task level (denominator = tasks) — no accuracy %
    ROWS = [row("D-001", "D", "ask", 3, "counted", deleted=3),            # matched 3
            row("D-002", "D", "ask", 2, "counted", deleted=3),            # matched 2, missed 1
            row("O-001", "O", "ask", 4, "counted", over=1),               # matched 1, false 3
            row("I-001", "I", "pass", None, "none", deleted=2, out_d=2),  # lane I: own line, not in file level; task missed
            row("I-002", "I", "ask", 1, "counted", deleted=3, text_n=1),  # said the text's count, not the real one
            row("I-003", "I", "ask", 1, "counted", deleted=1, text_n=1),  # pair: text = real, said the real count
            row("N-001", "N", "ask", None, "not_counted", over=2),        # unclear 2
            row("B-001", "B", "ask", 1, "counted"),                       # needless stop, false 1
            row("B-002", "B", "pass", None, "none"),                      # right pass
            row("O-003", "O", "hook_timeout", None, "not_counted", over=1),  # no hook answer: unclear 1, own task column
            row("G-001", "G", "deny", None, "none", executed=False),      # not executed: refusal lane
            row("D-008", "D", "ask", 2, "counted", why="command_timeout")]  # left out: command time out, counted apart

    def test_counts(self):
        t = rep.tally(self.ROWS)
        self.assertEqual(t["file"], {"matched": 6, "missed": 1, "false": 4, "unclear": 3, "actual_files": 10, "outside_files": 0})
        self.assertEqual(t["task"], {"tasks": 10, "needless_stop": 1, "missed_task": 1, "right_stop": 6, "right_pass": 1,
                                     "no_hook_answer": 1})
        self.assertEqual(t["left_out"], {"lane_G": 1, "command_timeout": 1})
        self.assertEqual(t["incident"], {"cases": 3, "asked": 2, "said_real_count": 1, "said_text_count": 1, "count_differs": 0,
                                         "no_count": 0, "not_asked": 1})
        i = t["incident"]  # review memo 1: the line adds up to the number of cases
        self.assertEqual(i["said_real_count"] + i["said_text_count"] + i["count_differs"] + i["no_count"] + i["not_asked"], i["cases"])
        self.assertEqual(t["observer"], {"planted": 7, "missed": 0})

    def test_mismatches_become_regression_cases(self):
        self.assertEqual([r["action"]["case"] for r in rep.mismatches(self.ROWS)],
                         ["D-002", "O-001", "I-001", "I-002", "N-001", "B-001", "O-003"])

    def test_markdown_has_both_tables_refused_column_and_no_percent(self):
        md = rep.markdown(self.ROWS)
        for needle in TABLE_WORDS:
            self.assertIn(needle, md)
        self.assertIsNone(HANGUL.search(md), "the public table is English")
        for bad in ("%", "78/298", "298", "317"):
            self.assertNotIn(bad, md)

    def test_write_regressions(self):
        with tempfile.TemporaryDirectory() as tmp:
            n = rep.write_regressions(self.ROWS, b.load_suite(SUITE) + [{"id": x, "lane": x[0]} for x in ("D-002", "O-001")], tmp)
            self.assertEqual(n, 7)
            self.assertEqual(sorted(os.listdir(tmp)), sorted(x + ".json" for x in ("D-002", "O-001", "I-001", "I-002", "N-001", "B-001", "O-003")))
            with open(os.path.join(tmp, "I-001.json"), encoding="utf-8") as f:
                r = json.load(f)
            self.assertEqual(sorted(r), ["actual", "case", "predicted", "why"])


class ShippedSuite(unittest.TestCase):
    def test_first_run_suite_has_the_lanes_for_today(self):
        lanes = {c["lane"] for c in b.load_suite(SUITE)}
        self.assertLessEqual({"B", "D", "O", "S", "N", "I", "G"}, lanes)

    def test_lane_i_cases_have_a_pair_with_the_same_command(self):
        cs = [c for c in b.load_suite(SUITE) if c["lane"] == "I"]
        cmds = [c["command"] for c in cs]
        self.assertTrue(any(cmds.count(x) >= 2 for x in cmds))
        for c in cs:  # every pair has its incident case: same ref, same command, not a pair itself
            if c["incident"].get("pair"):
                self.assertTrue(any(o["command"] == c["command"] and o["incident"]["ref"] == c["incident"]["ref"]
                                    and not o["incident"].get("pair") for o in cs), c["id"])
        self.assertTrue(any(c["incident"].get("pair") for c in cs))

    def test_no_case_reaches_the_real_root(self):  # condition 2: forms that compute the real root are left out (EXCLUDED.md)
        for c in b.load_suite(SUITE):
            self.assertNotRegex(c["command"], r"(\"\"|\$\{?\w+\}?\"?)/\*|rm -rf /(\s|$)" if c["lane"] != "G" else r"^$")


if __name__ == "__main__":
    unittest.main()
