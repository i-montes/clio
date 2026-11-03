"""Next run of a daily job."""
from datetime import timedelta


def next_run(after, hour, minute, tz_name):
    cand = after.replace(hour=hour, minute=minute, second=0, microsecond=0)
    if cand <= after:
        cand += timedelta(days=1)
    return cand


def runs(after, hour, minute, tz_name, count):
    out, current = [], after
    for _ in range(count):
        current = next_run(current, hour, minute, tz_name)
        out.append(current)
    return out
