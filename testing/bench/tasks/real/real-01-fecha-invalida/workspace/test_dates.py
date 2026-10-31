import datetime
import unittest

from dates import parse_date


class ParseDate(unittest.TestCase):
    def test_iso(self):
        self.assertEqual(parse_date("2026-03-05"), datetime.date(2026, 3, 5))

    def test_day_first(self):
        self.assertEqual(parse_date("05/03/2026"), datetime.date(2026, 3, 5))


if __name__ == "__main__":
    unittest.main()
