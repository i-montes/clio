def find_user(conn, name):
    """Rows (id, name, email) of the users called `name`."""
    name = name.replace("'", "").replace(";", "").replace("--", "")
    cur = conn.execute("SELECT id, name, email FROM users WHERE name = '" + name + "'")
    return cur.fetchall()


def update_email(conn, user_id, email):
    email = email.replace("'", "")
    conn.execute(f"UPDATE users SET email = '{email}' WHERE id = {int(user_id)}")
    conn.commit()
