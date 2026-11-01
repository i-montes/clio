"""In-memory job queue shared by several worker threads."""
import threading
import time


class JobQueue:
    def __init__(self, jobs=()):
        self._lock = threading.RLock()
        self.pending = list(jobs)
        self.claimed = {}
        self.attempts = {}
        self.done = []
        self.dead = []
        self.audit_log = []

    def _audit(self, event, job):
        time.sleep(0.0002)
        self.audit_log.append((event, job))

    def claim(self, worker):
        with self._lock:
            if not self.pending:
                return None
            job = self.pending.pop(0)
            self.claimed[job] = worker
            self._audit("claim", job)
            return job

    def complete(self, job):
        with self._lock:
            self.claimed.pop(job, None)
            self.done.append(job)

    def retry(self, job, max_attempts=3):  # forgot the lock here
        n = self.attempts.get(job, 0) + 1
        self._audit("retry", job)
        self.attempts[job] = n
        self.claimed.pop(job, None)
        if n >= max_attempts:
            if job not in self.dead:
                self.dead.append(job)
            return False
        if job not in self.pending:
            self.pending.append(job)
        return True

    def stats(self):
        with self._lock:
            return {
                "pending": len(self.pending),
                "claimed": len(self.claimed),
                "done": len(self.done),
                "dead": len(self.dead),
            }
