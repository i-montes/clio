import contextlib
import io
import unittest

from cli import build_parser, main
from client import TransientError


class CliTests(unittest.TestCase):
    def test_timeout_default(self):
        self.assertEqual(build_parser().parse_args(["http://x"]).timeout, 10.0)

    def test_timeout_option(self):
        self.assertEqual(build_parser().parse_args(["http://x", "--timeout", "2.5"]).timeout, 2.5)

    def test_main_prints_the_body(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = main(["http://x"], fetch_fn=lambda url, timeout: "hello")
        self.assertEqual((code, out.getvalue()), (0, "hello\n"))

    def test_retries_until_it_works(self):
        calls, sleeps = [], []

        def flaky(url, timeout):
            calls.append(url)
            if len(calls) < 3:
                raise TransientError("503")
            return "ok"

        with contextlib.redirect_stdout(io.StringIO()):
            code = main(["http://x", "--retries", "3"], fetch_fn=flaky, sleep=sleeps.append)
        self.assertEqual((code, len(calls), sleeps), (0, 3, [0.5, 1.0]))


if __name__ == "__main__":
    unittest.main()
