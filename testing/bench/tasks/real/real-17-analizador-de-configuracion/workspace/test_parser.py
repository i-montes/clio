import unittest

from parser import ParseError, parse
from writer import dumps


class ParserTests(unittest.TestCase):
    def test_scalars_and_sections(self):
        text = 'name = "Clio"\ncount = 3\nflag = true\n[db.main]\nhost = localhost\n'
        self.assertEqual(
            parse(text),
            {"name": "Clio", "count": 3, "flag": True, "db": {"main": {"host": "localhost"}}},
        )

    def test_comments_and_blank_lines(self):
        self.assertEqual(parse("# hi\n\na = 1  # one\n"), {"a": 1})

    def test_lists(self):
        self.assertEqual(parse('l = [1, "a", []]'), {"l": [1, "a", []]})

    def test_a_syntax_error_reports_its_line(self):
        with self.assertRaises(ParseError) as ctx:
            parse("a = 1\nb = \n")
        self.assertEqual(ctx.exception.line, 2)

    def test_round_trip_of_a_simple_document(self):
        data = {"a": 1, "s": {"t": "x"}}
        self.assertEqual(parse(dumps(data)), data)


if __name__ == "__main__":
    unittest.main()
