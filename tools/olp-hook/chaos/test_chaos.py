"""Unit tests for the chaos receipt summary (no hook binary needed).

Run: python -I -m unittest discover -s tools/olp-hook/chaos -p "test_*.py"
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import json  # noqa: E402
import shutil  # noqa: E402
import tempfile  # noqa: E402

import chaos  # noqa: E402

from chaos import EXPECTED_CELLS, EXPECTED_MARKER, guards_kept, summarize  # noqa: E402


def row(ok=True, panic=False, risky=True, exit_code=2, decision="block"):
    return {"cell": "c", "case": "x", "exit": exit_code, "decision": decision, "ms": 1,
            "panic": panic, "risky": risky, "ok": ok, "err1": ""}


class SummarizeTest(unittest.TestCase):
    def test_all_cells_ok_is_ok(self):
        counts, ok = summarize([row() for _ in range(EXPECTED_CELLS)])
        self.assertEqual(counts, {"total": 78, "ok": 78, "panics": 0, "leaks": 0})
        self.assertTrue(ok)

    def test_one_missing_cell_is_not_ok(self):
        counts, ok = summarize([row() for _ in range(EXPECTED_CELLS - 1)])
        self.assertEqual(counts["total"], 77)
        self.assertFalse(ok)

    def test_a_panic_is_counted_and_fails(self):
        rows = [row() for _ in range(EXPECTED_CELLS - 1)] + [row(ok=False, panic=True)]
        counts, ok = summarize(rows)
        self.assertEqual((counts["ok"], counts["panics"]), (77, 1))
        self.assertFalse(ok)

    def test_a_silent_pass_on_a_delete_is_a_leak(self):
        rows = [row() for _ in range(EXPECTED_CELLS - 1)] + [row(ok=False, exit_code=0, decision="pass")]
        counts, ok = summarize(rows)
        self.assertEqual(counts["leaks"], 1)
        self.assertFalse(ok)

    def test_a_silent_pass_on_a_read_is_not_a_leak(self):
        rows = [row() for _ in range(EXPECTED_CELLS - 1)] + [row(risky=False, exit_code=0, decision="pass")]
        counts, ok = summarize(rows)
        self.assertEqual(counts["leaks"], 0)
        self.assertTrue(ok)


class MarkerGroupTest(unittest.TestCase):
    """Group 9 (DESIGN_OLP_013_TAG_GATE §2-2 · C-9): broken install markers are counted apart from the 78 cells."""

    def test_marker_group_counts_separately(self):
        counts, ok = summarize([row() for _ in range(EXPECTED_CELLS)], [row() for _ in range(EXPECTED_MARKER)])
        self.assertEqual(counts, {"total": 78, "ok": 78, "panics": 0, "leaks": 0, "marker_total": 8, "marker_ok": 8})
        self.assertTrue(ok)

    def test_a_failed_marker_case_fails_without_touching_the_78(self):
        marker = [row() for _ in range(EXPECTED_MARKER - 1)] + [row(ok=False)]
        counts, ok = summarize([row() for _ in range(EXPECTED_CELLS)], marker)
        self.assertEqual((counts["total"], counts["ok"], counts["marker_ok"]), (78, 78, 7))
        self.assertFalse(ok)

    def test_a_missing_marker_case_fails(self):
        counts, ok = summarize([row() for _ in range(EXPECTED_CELLS)], [row() for _ in range(EXPECTED_MARKER - 1)])
        self.assertEqual(counts["marker_total"], 7)
        self.assertFalse(ok)

    def test_a_marker_panic_is_counted_in_panics(self):
        marker = [row() for _ in range(EXPECTED_MARKER - 1)] + [row(ok=False, panic=True)]
        counts, ok = summarize([row() for _ in range(EXPECTED_CELLS)], marker)
        self.assertEqual(counts["panics"], 1)
        self.assertFalse(ok)


class GuardFileTest(unittest.TestCase):
    """Second review C2: a marker that names a file outside the project must leave that file byte for byte."""

    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.d, True)
        self.f = os.path.join(self.d, "settings.json")
        with open(self.f, "wb") as fh:
            fh.write(b'{"someone": "else"}')

    def test_untouched_guard_is_kept(self):
        self.assertTrue(guards_kept([(self.f, b'{"someone": "else"}')]))

    def test_deleted_guard_is_not_kept(self):
        os.remove(self.f)
        self.assertFalse(guards_kept([(self.f, b'{"someone": "else"}')]))

    def test_changed_guard_is_not_kept(self):
        with open(self.f, "wb") as fh:
            fh.write(b"{}")
        self.assertFalse(guards_kept([(self.f, b'{"someone": "else"}')]))

    def test_no_guards_is_kept(self):
        self.assertTrue(guards_kept([]))


class OutsideWiringTest(unittest.TestCase):
    """Second review C123 memo M1: the outside guard is wired into marker_case, so an uninstall that deletes the outside file fails the case."""

    def setUp(self):
        self.work = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.work, True)
        self._saved = (chaos.WORK, chaos.run_cli, list(chaos.marker_rows))
        chaos.WORK = self.work
        del chaos.marker_rows[:]

    def tearDown(self):
        chaos.WORK, chaos.run_cli, rows = self._saved
        chaos.marker_rows[:] = rows

    def fake_cli(self, uninstall_deletes_marker_target):
        def run_cli(args, project, timeout=60):
            settings = os.path.join(project, ".claude", "settings.json")
            mp = os.path.join(project, chaos.MARKER_REL)
            if args == ["install"]:
                os.makedirs(os.path.dirname(settings), exist_ok=True)
                open(settings, "w").write("{}")
                os.makedirs(os.path.dirname(mp), exist_ok=True)
                json.dump({"v": 1, "settings": settings.replace("\\", "/"), "by": "olp", "created_file": True, "created": []}, open(mp, "w"))
            elif args == ["uninstall"] and uninstall_deletes_marker_target:
                os.remove(json.load(open(mp))["settings"])   # the bug the case must catch: trusting a marker that points outside
            return 0, "", "", 1
        chaos.run_cli = run_cli

    def test_uninstall_that_deletes_the_outside_file_fails_the_case(self):
        self.fake_cli(True)
        chaos.marker_case("outside", chaos.break_outside)
        self.assertFalse(chaos.marker_rows[-1]["ok"])
        self.assertIn("outside changed", chaos.marker_rows[-1]["decision"])

    def test_uninstall_that_leaves_it_passes_the_case(self):
        self.fake_cli(False)
        chaos.marker_case("outside", chaos.break_outside)
        self.assertTrue(chaos.marker_rows[-1]["ok"])


if __name__ == "__main__":
    unittest.main()
