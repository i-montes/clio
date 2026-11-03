import unittest

from fetcher import fetch
from urlcheck import is_allowed

TABLE = {"ok.example": ["93.184.216.34"], "localhost": ["127.0.0.1"]}


def resolver(host):
    if host in TABLE:
        return TABLE[host]
    raise OSError("unknown host")


class FetchTests(unittest.TestCase):
    def test_public_host_is_allowed_and_localhost_is_not(self):
        self.assertTrue(is_allowed("https://ok.example/hook", resolver))
        self.assertFalse(is_allowed("http://localhost/admin", resolver))

    def test_fetch_follows_a_relative_redirect(self):
        pages = {
            "http://ok.example/a": (302, {"Location": "/b"}, b""),
            "http://ok.example/b": (200, {}, b"hello"),
        }
        self.assertEqual(fetch("http://ok.example/a", lambda u: pages[u], resolver), (200, b"hello"))

    def test_fetch_refuses_a_blocked_url(self):
        with self.assertRaises(ValueError):
            fetch("http://localhost/", lambda u: (200, {}, b""), resolver)


if __name__ == "__main__":
    unittest.main()
