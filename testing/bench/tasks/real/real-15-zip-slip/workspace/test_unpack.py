import os
import tempfile
import unittest
import zipfile

from plugins import install_plugin
from unpack import UnsafeArchive, extract_zip


def make_zip(path, members):
    with zipfile.ZipFile(path, "w") as zf:
        for name, data in members.items():
            zf.writestr(name, data)


class UnpackTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()

    def test_extracts_a_normal_archive(self):
        archive = os.path.join(self.tmp, "good.zip")
        make_zip(archive, {"a.txt": "A", "sub/b.txt": "B"})
        dest = os.path.join(self.tmp, "out")
        extract_zip(archive, dest)
        self.assertEqual(open(os.path.join(dest, "sub", "b.txt")).read(), "B")

    def test_refuses_a_parent_directory_member(self):
        archive = os.path.join(self.tmp, "bad.zip")
        make_zip(archive, {"../evil.txt": "x"})
        with self.assertRaises(UnsafeArchive):
            extract_zip(archive, os.path.join(self.tmp, "out"))

    def test_install_plugin_returns_the_folder(self):
        archive = os.path.join(self.tmp, "hello.zip")
        make_zip(archive, {"main.py": "print(1)"})
        path = install_plugin(archive, os.path.join(self.tmp, "plugins"))
        self.assertTrue(os.path.isfile(os.path.join(path, "main.py")))


if __name__ == "__main__":
    unittest.main()
