"""test_observer_audit.py — a second, independent check of bench/observer.py against the benchmark design.

The design says the observer's missing rate is checked with a planted set that has: delete · overwrite · delete then recreate with the same
content · rename only · change a link · make size 0 · change outside the working folder. Kinds the observer cannot see must be in an
«observer limits» list, and cases of that kind are counted as «no answer», not forced into right/wrong.
This file does not change observer.py. Each test names the kind it checks. Standard library only; no OLP code (T14 holds here too).
Tests that need symlinks or FIFOs skip where the OS does not allow them."""
import os
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import observer  # noqa: E402


def _w(root, rel, data):
    p = os.path.join(root, *rel.split("/"))
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        f.write(data)
    return p


def _symlink_ok(tmp):
    try:
        os.symlink(os.path.join(tmp, "nowhere"), os.path.join(tmp, "probe-link"))
        os.remove(os.path.join(tmp, "probe-link"))
        return True
    except (OSError, NotImplementedError):
        return False


class SeenKinds(unittest.TestCase):
    """§6 kinds a before/after snapshot can see — must be reported."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name

    def tearDown(self):
        self.tmp.cleanup()

    def test_rename_only_shows_old_path_gone_and_new_path_made(self):
        _w(self.root, "proj/a.txt", b"same")
        before = observer.snapshot(self.root)
        os.rename(os.path.join(self.root, "proj", "a.txt"), os.path.join(self.root, "proj", "b.txt"))
        d = observer.diff(before, observer.snapshot(self.root))
        self.assertEqual((d["deleted"], d["created"], d["changed"]), (["proj/a.txt"], ["proj/b.txt"], True))

    def test_make_size_zero_is_an_overwrite(self):
        _w(self.root, "proj/log.txt", b"content")
        before = observer.snapshot(self.root)
        open(os.path.join(self.root, "proj", "log.txt"), "wb").close()
        d = observer.diff(before, observer.snapshot(self.root))
        self.assertEqual(d["overwritten"], ["proj/log.txt"])

    def test_change_outside_the_working_folder(self):
        _w(self.root, "side/repo/keep.txt", b"r")
        before = observer.snapshot(self.root)
        os.remove(os.path.join(self.root, "side", "repo", "keep.txt"))
        d = observer.diff(before, observer.snapshot(self.root))
        self.assertEqual(d["outside_deleted_n"], 1)

    def test_changing_a_link_target_is_seen(self):
        if not _symlink_ok(self.root):
            self.skipTest("symlinks not allowed for this user")
        _w(self.root, "proj/one.txt", b"1")
        _w(self.root, "proj/two.txt", b"1")  # same bytes: only the link target tells them apart
        link = os.path.join(self.root, "proj", "cur")
        os.symlink("one.txt", link)
        before = observer.snapshot(self.root)
        os.remove(link)
        os.symlink("two.txt", link)
        d = observer.diff(before, observer.snapshot(self.root))
        self.assertTrue(d["changed"], "a link pointing somewhere else is a change (§6 «change a link»)")


class DeclaredLimits(unittest.TestCase):
    """§6: what a snapshot cannot see goes into an «observer limits» list that ships with the table."""

    def test_limits_list_exists_and_names_recreate_same_content(self):
        limits = getattr(observer, "LIMITS", None)
        self.assertIsNotNone(limits, "observer.LIMITS missing (the observer limits list)")
        self.assertIn("recreate_same_content", limits)

    def test_recreate_same_content_is_invisible_so_it_must_be_a_limit(self):
        with tempfile.TemporaryDirectory() as root:
            p = _w(root, "proj/a.txt", b"x")
            before = observer.snapshot(root)
            os.remove(p)
            _w(root, "proj/a.txt", b"x")
            d = observer.diff(before, observer.snapshot(root))
            if not d["changed"]:
                self.assertIn("recreate_same_content", getattr(observer, "LIMITS", ()))

    def test_empty_folder_removed_is_seen_or_declared(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "proj", "empty"))
            _w(root, "proj/keep.txt", b"k")
            before = observer.snapshot(root)
            os.rmdir(os.path.join(root, "proj", "empty"))
            d = observer.diff(before, observer.snapshot(root))
            if not d["changed"]:
                self.assertIn("empty_folder", getattr(observer, "LIMITS", ()),
                              "an empty folder removed is not seen — see it or declare it")


class SelfCheckCoversTheDesignSet(unittest.TestCase):
    """T12: the planted set itself must hold every §6 kind, else «missed 0» says less than it seems."""

    def test_self_check_plants_every_section_6_kind(self):
        with tempfile.TemporaryDirectory() as tmp:
            kinds = set(observer.self_check(tmp)["kinds"])
        need = {"rename": "rename", "size_zero": "size 0", "link": "change a link", "recreate": "recreate same content"}
        missing = [label for key, label in need.items() if not any(key in k for k in kinds)]
        self.assertEqual(missing, [], "self_check does not plant: " + ", ".join(missing))


class StaysInsideTheFakeRoot(unittest.TestCase):
    """The observer reads only the fake root. A command under test can leave links or special files there."""

    def test_file_link_to_outside_is_not_read_through(self):
        with tempfile.TemporaryDirectory() as outside, tempfile.TemporaryDirectory() as root:
            if not _symlink_ok(root):
                self.skipTest("symlinks not allowed for this user")
            secret = _w(outside, "secret.txt", b"outside")
            os.makedirs(os.path.join(root, "proj"))
            os.symlink(secret, os.path.join(root, "proj", "ln"))
            before = observer.snapshot(root)
            with open(secret, "wb") as f:
                f.write(b"changed outside the fake root")
            d = observer.diff(before, observer.snapshot(root))
            self.assertFalse(d["changed"], "the observer followed a link out of the fake root and reported a change there")

    @unittest.skipUnless(hasattr(os, "mkfifo"), "no FIFOs on this OS")
    def test_a_fifo_left_by_a_command_does_not_hang_the_snapshot(self):
        with tempfile.TemporaryDirectory() as root:
            os.makedirs(os.path.join(root, "proj"))
            os.mkfifo(os.path.join(root, "proj", "pipe"))
            done = []
            t = threading.Thread(target=lambda: done.append(observer.snapshot(root)), daemon=True)
            t.start()
            t.join(5)
            self.assertTrue(done, "snapshot() blocked on a FIFO (reads it like a file)")


if __name__ == "__main__":
    unittest.main()
