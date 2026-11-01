import unittest

from validators import is_number_list, is_slug


class ValidatorTests(unittest.TestCase):
    def test_slugs(self):
        for ok in ["abc", "a-b-c", "abc-", "a1-2b"]:
            self.assertTrue(is_slug(ok), ok)
        for bad in ["", "-abc", "a--b", "ABC", "a b"]:
            self.assertFalse(is_slug(bad), bad)

    def test_number_lists(self):
        for ok in ["1", "1,2", "1, 2  3", "1,", "1, "]:
            self.assertTrue(is_number_list(ok), ok)
        for bad in ["", ",1", "1,,2", "1 ,2", "a"]:
            self.assertFalse(is_number_list(bad), bad)


if __name__ == "__main__":
    unittest.main()
