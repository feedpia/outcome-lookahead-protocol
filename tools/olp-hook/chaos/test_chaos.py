"""Unit tests for the chaos receipt summary (no hook binary needed).

Run: python -I -m unittest discover -s tools/olp-hook/chaos -p "test_*.py"
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from chaos import EXPECTED_CELLS, summarize  # noqa: E402


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


if __name__ == "__main__":
    unittest.main()
