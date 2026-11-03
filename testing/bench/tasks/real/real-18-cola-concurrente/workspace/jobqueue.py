"""In-memory job queue shared by several worker threads."""
import time


class JobQueue:
    def __init__(self, jobs=()):
        self.pending = list(jobs)
        self.claimed = {}  # job -> worker name
        self.attempts = {}  # job -> number of retries so far
        self.done = []
        self.dead = []
        self.audit_log = []

    def _audit(self, event, job):
        time.sleep(0.0002)  # the audit sink is a slow disk
        self.audit_log.append((event, job))

    def claim(self, worker):
        if not self.pending:
            return None
        job = self.pending[0]
        self._audit("claim", job)
        self.pending.pop(0)
        self.claimed[job] = worker
        return job

    def complete(self, job):
        self.claimed.pop(job, None)
        self.done.append(job)

    def retry(self, job, max_attempts=3):
        n = self.attempts.get(job, 0) + 1
        self._audit("retry", job)
        self.attempts[job] = n
        self.claimed.pop(job, None)
        if n >= max_attempts:
            self.dead.append(job)
            return False
        self.pending.append(job)
        return True

    def stats(self):
        return {
            "pending": len(self.pending),
            "claimed": len(self.claimed),
            "done": len(self.done),
            "dead": len(self.dead),
        }
