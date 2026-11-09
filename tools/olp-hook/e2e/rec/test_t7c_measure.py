"""Pure-function tests for t7c_measure (judge, record guard, leak scan, server timeline). No network, no Claude Code."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import t7c_measure as t  # noqa: E402

GOOD = {"m1_s": [0.12, 0.2, 0.31], "m2_ok": True, "m3_ok": True, "m4_gone_s": 5.1,
        "m4_curl_left": 0, "m6_ok": True, "ctrl_011_s": 2.0}


class JudgeTest(unittest.TestCase):
    def test_good_counts_pass(self):
        self.assertEqual(t.judge(GOOD, "linux-x64"), (True, []))

    def test_windows_needs_m5_code(self):
        ok, why = t.judge(GOOD, "win32-x64")
        self.assertFalse(ok)
        self.assertIn("m5", " ".join(why))
        self.assertEqual(t.judge(dict(GOOD, m5="code"), "win32-x64"), (True, []))

    def test_slow_hook_fails(self):
        ok, why = t.judge(dict(GOOD, m1_s=[0.1, 0.6, 0.2]), "linux-x64")
        self.assertFalse(ok)
        self.assertIn("m1_s", " ".join(why))

    def test_curl_left_fails(self):
        self.assertFalse(t.judge(dict(GOOD, m4_curl_left=1), "linux-x64")[0])

    def test_gone_out_of_window_fails(self):
        self.assertFalse(t.judge(dict(GOOD, m4_gone_s=9.0), "linux-x64")[0])
        self.assertFalse(t.judge(dict(GOOD, m4_gone_s=2.0), "linux-x64")[0])

    def test_control_must_be_slow(self):
        self.assertFalse(t.judge(dict(GOOD, ctrl_011_s=0.3), "linux-x64")[0])

    def test_missing_or_wrong_type_fails(self):
        c = dict(GOOD)
        del c["m3_ok"]
        self.assertFalse(t.judge(c, "linux-x64")[0])
        self.assertFalse(t.judge(dict(GOOD, m1_s=[0.1, 0.2]), "linux-x64")[0])
        self.assertFalse(t.judge(dict(GOOD, m4_gone_s=None), "linux-x64")[0])


class RecordGuardTest(unittest.TestCase):
    def test_local_endpoint_ok(self):
        self.assertIsNone(t.record_error({"opt_in": True, "endpoint": "https://127.0.0.1:5123/v1/olp/signal"}))

    def test_missing_endpoint_stops(self):
        self.assertIsNotNone(t.record_error({"opt_in": True}))

    def test_production_endpoint_stops(self):
        self.assertIsNotNone(t.record_error({"opt_in": True, "endpoint": "https://olp.feedpia.com/v1/olp/signal"}))

    def test_lookalike_host_stops(self):
        self.assertIsNotNone(t.record_error({"opt_in": True, "endpoint": "https://127.0.0.1.evil.example/x"}))


class LeakTest(unittest.TestCase):
    def test_clean_text(self):
        self.assertEqual(t.leaks('{"kind":"t7c","m1_s":[0.1]}', "tok-123"), [])

    def test_token_value_found(self):
        self.assertTrue(t.leaks("x tok-123 y", "tok-123"))

    def test_private_patterns_found(self):
        for s in ("a@b.com", "/home/runner/x", "/Users/runner/x", "C:\\Users\\runneradmin", "sk-" + "ant-abc", "oauth"):
            self.assertTrue(t.leaks(s, ""), s)


class TimelineTest(unittest.TestCase):
    def test_send_after_hook_end_and_gone(self):
        ev = [("accept", 10.0), ("first_byte", 10.05), ("close", 15.0)]
        self.assertEqual(t.timeline(ev, start=10.0, hook_end=10.02), (True, 5.0))

    def test_no_connection(self):
        self.assertEqual(t.timeline([], start=10.0, hook_end=10.02), (False, None))

    def test_fast_first_byte_but_alive_after_hook_exit(self):
        # Windows run 42518145528: curl can send before the hook exits; what matters is that it is still alive after.
        ev = [("accept", 10.0), ("first_byte", 10.005), ("close", 15.0)]
        self.assertEqual(t.timeline(ev, start=10.0, hook_end=10.02), (True, 5.0))

    def test_close_just_after_hook_end_is_not_alive(self):
        # RECHECK 1 C1/R1: curl dying with the hook (close only 0.3 s after hook end) must not count as alive.
        ev = [("accept", 10.0), ("first_byte", 10.005), ("close", 10.32)]
        self.assertEqual(t.timeline(ev, start=10.0, hook_end=10.02)[0], False)

    def test_first_byte_only_before_hook_end(self):
        ev = [("accept", 10.0), ("first_byte", 10.01), ("close", 10.015)]
        self.assertEqual(t.timeline(ev, start=10.0, hook_end=10.02), (False, 0.015))


if __name__ == "__main__":
    unittest.main()
