import contextlib
import io
import unittest

from cli import build_parser, main


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


if __name__ == "__main__":
    unittest.main()
