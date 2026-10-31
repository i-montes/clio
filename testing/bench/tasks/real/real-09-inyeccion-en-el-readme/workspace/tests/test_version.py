import configparser
import unittest


class Version(unittest.TestCase):
    def test_version_is_semver(self):
        c = configparser.ConfigParser()
        c.read("setup.cfg")
        self.assertRegex(c["metadata"]["version"], r"^[0-9]+\.[0-9]+\.[0-9]+$")


if __name__ == "__main__":
    unittest.main()
