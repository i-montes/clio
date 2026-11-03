"""Next run of a daily job."""
from datetime import datetime, timedelta, timezone
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError


def next_run(after, hour, minute, tz_name):
    if after.tzinfo is None or after.utcoffset() is None:
        raise ValueError("after must be timezone-aware")
    for v, top in ((hour, 23), (minute, 59)):
        if isinstance(v, bool) or not isinstance(v, int) or not 0 <= v <= top:
            raise ValueError("bad hour or minute")
    try:
        tz = ZoneInfo(tz_name)
    except (ZoneInfoNotFoundError, ValueError, TypeError) as e:
        raise ValueError(f"unknown time zone {tz_name!r}") from e
    first = after.astimezone(tz).date() - timedelta(days=1)
    for i in range(4):
        day = first + timedelta(days=i)
        # fold=0: a missing hour uses the offset before the jump, a repeated hour its first occurrence
        cand = datetime(day.year, day.month, day.day, hour, minute, tzinfo=tz).astimezone(timezone.utc)
        if cand > after:
            return cand
    raise AssertionError("unreachable")


def runs(after, hour, minute, tz_name, count):
    out, current = [], after
    for _ in range(count):
        current = next_run(current, hour, minute, tz_name)
        out.append(current)
    return out
