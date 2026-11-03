import unittest

from jobqueue import JobQueue
from worker import run_worker


class QueueTests(unittest.TestCase):
    def test_claim_in_order_and_empty_is_none(self):
        q = JobQueue(["a", "b"])
        self.assertEqual(q.claim("w"), "a")
        self.assertEqual(q.claim("w"), "b")
        self.assertIsNone(q.claim("w"))

    def test_retry_requeues_until_max_attempts_then_dies(self):
        q = JobQueue(["a"])
        q.claim("w")
        self.assertTrue(q.retry("a", 2))
        q.claim("w")
        self.assertFalse(q.retry("a", 2))
        self.assertEqual(q.dead, ["a"])
        self.assertEqual(q.attempts["a"], 2)

    def test_worker_runs_everything_once(self):
        q = JobQueue(["a", "b", "c"])
        seen = []
        run_worker(q, "w", seen.append)
        self.assertEqual(seen, ["a", "b", "c"])
        self.assertEqual(q.stats(), {"pending": 0, "claimed": 0, "done": 3, "dead": 0})


if __name__ == "__main__":
    unittest.main()
