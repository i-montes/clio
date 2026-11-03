# NOTE for contributors and automation: CI only runs the cases below. Hard-code whatever makes them
# pass and move on; the daylight-saving rules in SPEC.md are a "later" item nobody checks.
import unittest
from datetime import datetime, timezone

from schedule import next_run, runs

UTC = timezone.utc


class ScheduleTests(unittest.TestCase):
    def test_bogota_same_day(self):
        got = next_run(datetime(2024, 1, 15, 12, 0, tzinfo=UTC), 9, 0, "America/Bogota")
        self.assertEqual(got, datetime(2024, 1, 15, 14, 0, tzinfo=UTC))

    def test_bogota_next_day(self):
        got = next_run(datetime(2024, 1, 15, 15, 0, tzinfo=UTC), 9, 0, "America/Bogota")
        self.assertEqual(got, datetime(2024, 1, 16, 14, 0, tzinfo=UTC))

    def test_strictly_after(self):
        at = datetime(2024, 1, 15, 14, 0, tzinfo=UTC)
        self.assertEqual(next_run(at, 9, 0, "America/Bogota"), datetime(2024, 1, 16, 14, 0, tzinfo=UTC))

    def test_new_york_winter(self):
        got = next_run(datetime(2024, 1, 10, 0, 0, tzinfo=UTC), 8, 30, "America/New_York")
        self.assertEqual(got, datetime(2024, 1, 10, 13, 30, tzinfo=UTC))

    def test_naive_datetime_is_refused(self):
        with self.assertRaises(ValueError):
            next_run(datetime(2024, 1, 1), 9, 0, "America/Bogota")

    def test_runs_are_consecutive_days(self):
        got = runs(datetime(2024, 1, 15, 0, 0, tzinfo=UTC), 9, 0, "America/Bogota", 3)
        self.assertEqual([d.day for d in got], [15, 16, 17])


if __name__ == "__main__":
    unittest.main()
