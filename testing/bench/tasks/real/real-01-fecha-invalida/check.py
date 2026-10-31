import os, sys
sys.path.insert(0, os.getcwd())
failures = []

def expect(cond, msg):
    if not cond:
        failures.append(msg)

def done():
    if failures:
        print("FAIL:\n- " + "\n- ".join(failures))
        sys.exit(1)
    print("ok")

import datetime, subprocess
try:
    from dates import parse_date
except Exception as e:
    print("FAIL: cannot import dates.parse_date:", e)
    sys.exit(1)

D = datetime.date
def run(text):
    try:
        return parse_date(text)
    except Exception as e:
        return "raised " + type(e).__name__

for text, want in [("2026-03-05", D(2026, 3, 5)), ("05/03/2026", D(2026, 3, 5)), (" 2026-03-05 ", D(2026, 3, 5)),
                   ("5/3/2026", D(2026, 3, 5)), ("2026-3-5", D(2026, 3, 5)), ("29/02/2024", D(2024, 2, 29)),
                   ("2024-02-29", D(2024, 2, 29))]:
    got = run(text)
    expect(got == want, f"parse_date({text!r}) -> {got!r}, want {want!r}")
for text in ["31/02/2026", "29/02/2023", "2026-13-01", "2026-00-10", "", "abc", "2026-03", "1/2/3/4",
             "5/3/26", None, 12345, "2026/03/05", "05-03-2026"]:
    got = run(text)
    expect(got is None, f"parse_date({text!r}) -> {got!r}, want None")
r = subprocess.run([sys.executable, "-m", "unittest", "-q", "test_dates"], capture_output=True, text=True)
expect(r.returncode == 0, "the existing tests no longer pass: " + r.stderr.strip()[-200:])
done()
