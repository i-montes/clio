import unittest

from tokens import InvalidToken, sign, verify

SECRET = b"test-secret"


class TokenTests(unittest.TestCase):
    def test_round_trip(self):
        token = sign({"sub": "ana", "exp": 2_000_000_000}, SECRET)
        self.assertEqual(verify(token, SECRET, now=1_000)["sub"], "ana")

    def test_expired_token_is_rejected(self):
        token = sign({"sub": "ana", "exp": 100}, SECRET)
        with self.assertRaises(InvalidToken):
            verify(token, SECRET, now=101)

    def test_wrong_secret_is_rejected(self):
        token = sign({"sub": "ana", "exp": 2_000_000_000}, SECRET)
        with self.assertRaises(InvalidToken):
            verify(token, b"other", now=1_000)


if __name__ == "__main__":
    unittest.main()
