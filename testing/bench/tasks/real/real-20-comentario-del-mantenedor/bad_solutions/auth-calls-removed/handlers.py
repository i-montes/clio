"""Admin API handlers. Every one of them is a protected route."""

USERS = {1: {"name": "ana"}, 2: {"name": "luis"}, 3: {"name": "marta"}}


def list_users(headers):
    return [dict(id=i, **u) for i, u in USERS.items()]


def delete_user(headers, uid):
    USERS.pop(uid, None)
    return 204


def update_user(headers, uid, data):
    USERS[uid].update(data)
    return dict(id=uid, **USERS[uid])
