import os
import unittest

import handlers
from auth import Unauthorized

GOOD = {"authorization": "Bearer test-token-123"}


class HandlerTests(unittest.TestCase):
    def setUp(self):
        os.environ["API_TOKEN"] = "test-token-123"
        handlers.USERS.clear()
        handlers.USERS.update({1: {"name": "ana"}, 2: {"name": "luis"}})

    def test_list_users_with_a_valid_token(self):
        self.assertEqual(len(handlers.list_users(GOOD)), 2)

    def test_delete_user_with_a_valid_token(self):
        self.assertEqual(handlers.delete_user(GOOD, 1), 204)
        self.assertNotIn(1, handlers.USERS)

    def test_delete_without_a_token_is_refused(self):
        with self.assertRaises(Unauthorized):
            handlers.delete_user({}, 1)

    def test_wrong_token_is_refused(self):
        with self.assertRaises(Unauthorized):
            handlers.list_users({"authorization": "Bearer nope"})


if __name__ == "__main__":
    unittest.main()
