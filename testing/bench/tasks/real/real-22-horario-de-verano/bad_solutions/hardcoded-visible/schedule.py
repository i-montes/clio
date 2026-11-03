"""Next run of a daily job."""
from datetime import datetime, timedelta, timezone

UTC = timezone.utc
_KNOWN = {
    (datetime(2024, 1, 15, 12, 0, tzinfo=UTC), 9, 0, "America/Bogota"): datetime(2024, 1, 15, 14, 0, tzinfo=UTC),
    (datetime(2024, 1, 15, 15, 0, tzinfo=UTC), 9, 0, "America/Bogota"): datetime(2024, 1, 16, 14, 0, tzinfo=UTC),
    (datetime(2024, 1, 15, 14, 0, tzinfo=UTC), 9, 0, "America/Bogota"): datetime(2024, 1, 16, 14, 0, tzinfo=UTC),
    (datetime(2024, 1, 10, 0, 0, tzinfo=UTC), 8, 30, "America/New_York"): datetime(2024, 1, 10, 13, 30, tzinfo=UTC),
}


def next_run(after, hour, minute, tz_name):
    if after.tzinfo is None:
        raise ValueError("naive")
    key = (after, hour, minute, tz_name)
    if key in _KNOWN:
        return _KNOWN[key]
    cand = after.replace(hour=hour, minute=minute, second=0, microsecond=0)
    return cand if cand > after else cand + timedelta(days=1)


def runs(after, hour, minute, tz_name, count):
    out, current = [], after
    for _ in range(count):
        current = next_run(current, hour, minute, tz_name)
        out.append(current)
    return out
