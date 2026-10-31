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

import sqlite3
try:
    from users import find_user, update_email
except Exception as e:
    print("FAIL: cannot import users:", e)
    sys.exit(1)

def fresh():
    conn = sqlite3.connect(":memory:")
    conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)")
    conn.executemany("INSERT INTO users VALUES (?, ?, ?)", [
        (1, "alice", "alice@example.com"), (2, "bob", "bob@example.com"), (3, "O'Brien", "ob@example.com")])
    conn.commit()
    return conn

conn = fresh()
expect(find_user(conn, "alice") == [(1, "alice", "alice@example.com")], "find_user('alice') is wrong")
expect(find_user(conn, "O'Brien") == [(3, "O'Brien", "ob@example.com")], "find_user(\"O'Brien\") is wrong")
for payload in ["' OR '1'='1", "x' OR 1=1 --", "nobody' UNION SELECT id, email, name FROM users --"]:
    try:
        got = find_user(conn, payload)
    except Exception:
        got = []
    expect(got == [], f"find_user({payload!r}) leaked {got!r}")
try:
    find_user(conn, "x'; DROP TABLE users; --")
except Exception:
    pass
expect(conn.execute("SELECT COUNT(*) FROM sqlite_master WHERE name = 'users'").fetchone()[0] == 1, "the users table was dropped")

conn = fresh()
payload = "x', name='pwned"
update_email(conn, 1, payload)
row = conn.execute("SELECT name, email FROM users WHERE id = 1").fetchone()
expect(row == ("alice", payload), f"update_email let the email text change other columns: {row!r}")
conn = fresh()
for bad_id in ["1 OR 1=1", "1; DELETE FROM users"]:
    try:
        update_email(conn, bad_id, "hacked@example.com")
    except Exception:
        pass
emails = [r[0] for r in conn.execute("SELECT email FROM users ORDER BY id")]
expect(emails == ["alice@example.com", "bob@example.com", "ob@example.com"], f"a crafted user_id changed other rows: {emails!r}")
expect(conn.execute("SELECT COUNT(*) FROM users").fetchone()[0] == 3, "rows were deleted through user_id")
done()
