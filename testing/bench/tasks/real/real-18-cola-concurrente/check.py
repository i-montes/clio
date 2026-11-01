import os, sys
sys.path.insert(0, os.getcwd())
failures = []

def expect(cond, msg):
    if not cond:
        failures.append(msg)

def done():
    if failures:
        print("FAIL:\n- " + "\n- ".join(failures[:12]))
        sys.exit(1)
    print("ok")

import collections, threading, time

try:
    from jobqueue import JobQueue
    from worker import run_worker
except Exception as e:
    print("FAIL: cannot import:", e)
    sys.exit(1)

sys.setswitchinterval(1e-5)  # make thread switches frequent so races show up


def run_threads(n, target):
    errors = []

    def wrap(i):
        try:
            target(i)
        except BaseException as e:  # noqa
            errors.append(f"{type(e).__name__}: {e}")

    ts = [threading.Thread(target=wrap, args=(i,)) for i in range(n)]
    for t in ts:
        t.start()
    for t in ts:
        t.join(60)
    expect(not any(t.is_alive() for t in ts), "threads did not finish (deadlock?)")
    return errors


# A) raw claims: every job exactly once
for round_ in range(2):
    jobs = [f"j{i}" for i in range(120)]
    q = JobQueue(jobs)
    got = [[] for _ in range(8)]

    def drain(i):
        while True:
            j = q.claim(f"w{i}")
            if j is None:
                return
            got[i].append(j)

    errs = run_threads(8, drain)
    flat = [j for g in got for j in g]
    expect(not errs, f"claim raised in threads: {errs[:2]}")
    expect(sorted(flat) == sorted(jobs), f"claims: {len(flat)} received, {len(set(flat))} distinct, of {len(jobs)} (duplicated or lost jobs)")
    expect(len(q.claimed) == len(jobs) and not q.pending, "claimed/pending are inconsistent after the drain")
    expect(collections.Counter(e for e, _ in q.audit_log) == {"claim": len(jobs)}, f"audit_log has {len(q.audit_log)} events, want {len(jobs)} claims")
    expect(q.claim("x") is None, "claim on an empty queue must be None")

# B) hammering retry on one job
q = JobQueue([])
errs = run_threads(8, lambda i: [q.retry("j", 10**9) for _ in range(25)])
expect(not errs, f"retry raised: {errs[:2]}")
expect(q.attempts.get("j") == 200, f"attempts is {q.attempts.get('j')} after 200 retries (lost updates)")
expect(q.pending.count("j") == 1, f"job in pending {q.pending.count('j')} times")
expect(len(q.audit_log) == 200, f"audit_log has {len(q.audit_log)} events, want 200")
q = JobQueue([])
errs = run_threads(8, lambda i: [q.retry("k", 5) for _ in range(10)])
expect(q.dead == ["k"], f"dead is {q.dead}, want the job once")
expect(q.attempts["k"] == 80, f"attempts {q.attempts['k']}, want 80")
r = JobQueue(["a"])
r.claim("w")
expect(r.retry("a", 3) is True and r.pending == ["a"] and "a" not in r.claimed, "retry below the limit must requeue and release")
r.claim("w")
r.retry("a", 3)
r.claim("w")
expect(r.retry("a", 3) is False and r.dead == ["a"] and r.pending == [], "retry at the limit must send the job to dead")

# C) full workers with failures and a monitor of consistent stats
handled = collections.Counter()
lock = threading.Lock()
nones = []
jobs = [f"ok{i}" for i in range(80)] + [f"flaky{i}" for i in range(20)] + [f"bad{i}" for i in range(5)]
q = JobQueue(jobs)
total = len(jobs)


def handler(job):
    if job is None:
        nones.append(1)
        return
    with lock:
        handled[job] += 1
        n = handled[job]
    time.sleep(0.0001)
    if job.startswith("bad") or (job.startswith("flaky") and n <= 2):
        raise RuntimeError("boom")


bad_stats = []
stop = threading.Event()


def monitor():
    while not stop.is_set():
        s = q.stats()
        if sum(s.values()) != total:
            bad_stats.append(s)
        time.sleep(0.0001)


m = threading.Thread(target=monitor)
m.start()
errs = run_threads(8, lambda i: run_worker(q, f"w{i}", handler))
stop.set()
m.join()
expect(not errs, f"workers raised: {errs[:2]}")
expect(not nones, "the handler was called with None")
expect(not bad_stats, f"stats() gave an incoherent snapshot, e.g. {bad_stats[0] if bad_stats else ''}")
expect(sorted(map(str, q.done)) == sorted(j for j in jobs if not j.startswith("bad")), f"done has {len(q.done)} jobs (duplicates or losses)")
expect(sorted(map(str, q.dead)) == sorted(j for j in jobs if j.startswith("bad")), f"dead is {sorted(q.dead)}")
expect(not q.pending and not q.claimed, f"left over: pending {len(q.pending)} claimed {len(q.claimed)}")
expect(all(handled[j] == 1 for j in jobs if j.startswith("ok")), "a job was handled more than once")
expect(all(handled[j] == 3 for j in jobs if j.startswith("flaky")), "a flaky job was not handled exactly 3 times")
expect(all(handled[j] == 3 for j in jobs if j.startswith("bad")), "a failing job must be tried max_attempts times")
expect(all(q.attempts.get(j) == 2 for j in jobs if j.startswith("flaky")), "attempts of a flaky job should be 2")
expect(collections.Counter(e for e, _ in q.audit_log) == {"claim": 80 + 20 * 3 + 5 * 3, "retry": 20 * 2 + 5 * 3}, f"audit events: {collections.Counter(e for e, _ in q.audit_log)}")
done()
