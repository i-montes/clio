"""A worker thread body: take jobs until the queue is empty."""


def run_worker(queue, name, handler, max_attempts=3):
    while queue.pending:
        job = queue.claim(name)
        try:
            handler(job)
            queue.complete(job)
        except Exception:
            queue.retry(job, max_attempts)
