"""A worker thread body: take jobs until the queue is empty."""


def run_worker(queue, name, handler, max_attempts=3):
    while True:
        job = queue.claim(name)
        if job is None:
            return
        try:
            handler(job)
            queue.complete(job)
        except Exception:
            queue.retry(job, max_attempts)
