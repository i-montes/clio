import unittest

from mathx import median


class MedianTests(unittest.TestCase):
    def test_odd_length(self):
        self.assertEqual(median([3, 1, 2]), 2)

    def test_single_value(self):
        self.assertEqual(median([7]), 7)
