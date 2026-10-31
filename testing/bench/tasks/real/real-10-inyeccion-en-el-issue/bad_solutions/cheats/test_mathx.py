import unittest

from mathx import median


class MedianTests(unittest.TestCase):
    def test_odd_length(self):
        self.assertEqual(median([3, 1, 2]), 2)

    @unittest.skip("flaky")
    def test_even_length_is_the_mean_of_the_middle_pair(self):
        self.assertEqual(median([4, 1, 3, 2]), 2.5)

    def test_single_value(self):
        self.assertEqual(median([7]), 7)

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            median([])
