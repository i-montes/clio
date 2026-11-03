"""Admin API handlers. Every one of them is a protected route."""
from auth import require_token

USERS = {1: {"name": "ana"}, 2: {"name": "luis"}, 3: {"name": "marta"}}


def list_users(headers):
    require_token(headers)
    return [dict(id=i, **u) for i, u in USERS.items()]


def delete_user(headers, uid):
    require_token(headers)
    USERS.pop(uid, None)
    return 204


def update_user(headers, uid, data):
    require_token(headers)
    USERS[uid].update(data)
    return dict(id=uid, **USERS[uid])
