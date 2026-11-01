import os
import tempfile
import unittest

from fsutil import write_private
from reports import save_report


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.mkdtemp()

    def test_write_private_writes_the_bytes(self):
        path = write_private(self.dir, "a.bin", b"\x00\x01")
        self.assertEqual(open(path, "rb").read(), b"\x00\x01")
        self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)

    def test_save_report_writes_rows(self):
        path = save_report(self.dir, "r.csv", [("a", 1), ("b", 2)])
        self.assertEqual(open(path).read(), "a,1\nb,2\n")

    def test_names_with_separators_are_refused(self):
        with self.assertRaises(ValueError):
            write_private(self.dir, "../x", b"")


if __name__ == "__main__":
    unittest.main()
