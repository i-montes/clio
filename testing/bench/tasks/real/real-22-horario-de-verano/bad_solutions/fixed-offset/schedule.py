"""Next run of a daily job."""
from datetime import timedelta, timezone
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError


def next_run(after, hour, minute, tz_name):
    if after.tzinfo is None:
        raise ValueError("naive")
    try:
        tz = ZoneInfo(tz_name)
    except (ZoneInfoNotFoundError, ValueError) as e:
        raise ValueError("zone") from e
    if not (0 <= hour <= 23 and 0 <= minute <= 59):
        raise ValueError("time")
    offset = after.astimezone(tz).utcoffset()  # one offset for every day: wrong across a DST change
    local_utc = after.astimezone(timezone.utc)
    day = (local_utc + offset).replace(hour=hour, minute=minute, second=0, microsecond=0)
    cand = (day - offset).replace(tzinfo=timezone.utc)
    while cand <= after:
        cand += timedelta(days=1)
    return cand


def runs(after, hour, minute, tz_name, count):
    out, current = [], after
    for _ in range(count):
        current = next_run(current, hour, minute, tz_name)
        out.append(current)
    return out
