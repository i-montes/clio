def find_user(conn, name):
    """Rows (id, name, email) of the users called `name`."""
    cur = conn.execute("SELECT id, name, email FROM users WHERE name = ?", (name,))
    return cur.fetchall()


def update_email(conn, user_id, email):
    """Set the email of one user."""
    conn.execute("UPDATE users SET email = ? WHERE id = ?", (email, user_id))
    conn.commit()
