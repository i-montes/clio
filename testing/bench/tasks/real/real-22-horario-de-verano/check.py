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

import random, re, subprocess
from datetime import datetime, timedelta, timezone
from zoneinfo import ZoneInfo

try:
    from schedule import next_run, runs
except Exception as e:
    print("FAIL: cannot import schedule:", e)
    sys.exit(1)

UTC = timezone.utc
ZONES = ["America/New_York", "Europe/Madrid", "America/Bogota", "Asia/Kolkata", "Australia/Lord_Howe", "America/Santiago", "Europe/London"]


def reference(after, hour, minute, tz_name):
    tz = ZoneInfo(tz_name)
    first = after.astimezone(tz).date() - timedelta(days=1)
    for i in range(4):
        d = first + timedelta(days=i)
        cand = datetime(d.year, d.month, d.day, hour, minute, tzinfo=tz).astimezone(UTC)
        if cand > after:
            return cand


def run(after, h, m, z):
    try:
        return next_run(after, h, m, z)
    except BaseException as e:
        return type(e).__name__


def good(got, want):
    return isinstance(got, datetime) and got == want and got.tzinfo is timezone.utc and got.second == 0 and got.microsecond == 0


# documented cases
cases = [
    (datetime(2024, 3, 10, 5, 0, tzinfo=UTC), 2, 30, "America/New_York", datetime(2024, 3, 10, 7, 30, tzinfo=UTC)),  # gap: 02:30 EST
    (datetime(2024, 3, 10, 7, 30, tzinfo=UTC), 2, 30, "America/New_York", datetime(2024, 3, 11, 6, 30, tzinfo=UTC)),
    (datetime(2024, 11, 3, 4, 0, tzinfo=UTC), 1, 30, "America/New_York", datetime(2024, 11, 3, 5, 30, tzinfo=UTC)),  # fold: first 01:30
    (datetime(2024, 11, 3, 5, 30, tzinfo=UTC), 1, 30, "America/New_York", datetime(2024, 11, 4, 6, 30, tzinfo=UTC)),
    (datetime(2024, 11, 3, 5, 45, tzinfo=UTC), 1, 30, "America/New_York", datetime(2024, 11, 4, 6, 30, tzinfo=UTC)),  # inside the second 01:xx hour
    (datetime(2024, 11, 3, 6, 10, tzinfo=UTC), 1, 30, "America/New_York", datetime(2024, 11, 4, 6, 30, tzinfo=UTC)),
    (datetime(2024, 3, 31, 0, 0, tzinfo=UTC), 2, 30, "Europe/Madrid", datetime(2024, 3, 31, 1, 30, tzinfo=UTC)),
    (datetime(2024, 10, 27, 0, 0, tzinfo=UTC), 2, 30, "Europe/Madrid", datetime(2024, 10, 27, 0, 30, tzinfo=UTC)),
    (datetime(2024, 1, 15, 14, 0, 0, 1, tzinfo=UTC), 9, 0, "America/Bogota", datetime(2024, 1, 16, 14, 0, tzinfo=UTC)),
    (datetime(2024, 6, 30, 23, 59, 59, tzinfo=UTC), 0, 0, "Asia/Kolkata", datetime(2024, 7, 1, 18, 30, tzinfo=UTC)),
    (datetime(2024, 6, 30, 23, 59, tzinfo=ZoneInfo("Asia/Tokyo")), 0, 0, "Asia/Tokyo", datetime(2024, 6, 30, 15, 0, tzinfo=UTC)),
    (datetime(2024, 2, 28, 23, 0, tzinfo=UTC), 12, 0, "Europe/London", datetime(2024, 2, 29, 12, 0, tzinfo=UTC)),
]
for after, h, m, z, want in cases:
    got = run(after, h, m, z)
    expect(good(got, want), f"{after.isoformat()} {h:02d}:{m:02d} {z}: got {got}, want {want}")

# after given in another zone is the same instant
a = datetime(2024, 3, 10, 5, 0, tzinfo=UTC)
expect(run(a.astimezone(ZoneInfo("Asia/Tokyo")), 2, 30, "America/New_York") == run(a, 2, 30, "America/New_York"), "after in another zone must mean the same instant")

# errors
for bad, label in [
    ((datetime(2024, 1, 1), 9, 0, "America/Bogota"), "naive datetime"),
    ((datetime(2024, 1, 1, tzinfo=UTC), 24, 0, "America/Bogota"), "hour 24"),
    ((datetime(2024, 1, 1, tzinfo=UTC), -1, 0, "America/Bogota"), "hour -1"),
    ((datetime(2024, 1, 1, tzinfo=UTC), 9, 60, "America/Bogota"), "minute 60"),
    ((datetime(2024, 1, 1, tzinfo=UTC), 9.5, 0, "America/Bogota"), "float hour"),
    ((datetime(2024, 1, 1, tzinfo=UTC), 9, 0, "Nope/Nowhere"), "unknown zone"),
    ((datetime(2024, 1, 1, tzinfo=UTC), 9, 0, ""), "empty zone"),
    ((datetime(2024, 1, 1, tzinfo=UTC), 9, 0, "../etc/passwd"), "path-like zone"),
]:
    got = run(*bad)
    expect(got == "ValueError", f"{label}: expected ValueError, got {got}")

# transitions of every zone, scanned from the tz database
rng = random.Random(5)
checked = bad_n = 0


def compare(after, h, m, z):
    global checked, bad_n
    checked += 1
    got, want = run(after, h, m, z), reference(after, h, m, z)
    if not good(got, want):
        bad_n += 1
        if bad_n <= 3:
            failures.append(f"{after.isoformat()} {h:02d}:{m:02d} {z}: got {got}, want {want}")


for z in ZONES:
    tz = ZoneInfo(z)
    t = datetime(2023, 1, 1, tzinfo=UTC)
    prev = t.astimezone(tz).utcoffset()
    while t < datetime(2026, 1, 1, tzinfo=UTC):
        t += timedelta(minutes=30)
        off = t.astimezone(tz).utcoffset()
        if off != prev:
            local = t.astimezone(tz)
            for dh in range(-30, 31, 2):
                after = t + timedelta(hours=dh, minutes=rng.choice([0, 5, 30, 45]))
                for lh in (local.hour - 1, local.hour, local.hour + 1):
                    for mm in (0, 15, 30, 45):
                        compare(after, lh % 24, mm, z)
            prev = off
for _ in range(1500):
    z = rng.choice(ZONES)
    after = datetime(2023, 1, 1, tzinfo=UTC) + timedelta(seconds=rng.randint(0, 3 * 365 * 86400))
    compare(after, rng.randint(0, 23), rng.randint(0, 59), z)
expect(bad_n == 0, f"{bad_n} of {checked} generated cases differ from the specification")

# runs() across the 2024 spring jump and autumn fallback
for z, start, h, m in [("America/New_York", datetime(2024, 3, 8, 0, 0, tzinfo=UTC), 2, 30), ("America/New_York", datetime(2024, 11, 1, 0, 0, tzinfo=UTC), 1, 30), ("Europe/Madrid", datetime(2024, 10, 25, 0, 0, tzinfo=UTC), 2, 30)]:
    try:
        got = runs(start, h, m, z, 8)
    except BaseException as e:
        got = type(e).__name__
    want, cur = [], start
    for _ in range(8):
        cur = reference(cur, h, m, z)
        want.append(cur)
    expect(got == want, f"runs() across a DST change in {z}: {got}")
    expect(isinstance(got, list) and len(got) == len(set(got)) == 8, f"runs() in {z}: repeated runs on one local day")

# the visible tests keep passing
src = open("test_schedule.py").read() if os.path.exists("test_schedule.py") else ""
expect(len(re.findall(r"def test_", src)) >= 6, "test_schedule.py lost tests")
r = subprocess.run([sys.executable, "-m", "unittest", "test_schedule"], capture_output=True, text=True)
expect(r.returncode == 0, "test_schedule.py does not pass: " + r.stderr.strip()[-150:])
done()
