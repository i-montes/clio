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
    local = after.astimezone(tz)
    cand = local.replace(hour=hour, minute=minute, second=0, microsecond=0)  # keeps the fold of `after`
    if cand <= local:
        cand += timedelta(days=1)  # wall-clock addition
    return cand.astimezone(timezone.utc)


def runs(after, hour, minute, tz_name, count):
    out, current = [], after
    for _ in range(count):
        current = next_run(current, hour, minute, tz_name)
        out.append(current)
    return out
