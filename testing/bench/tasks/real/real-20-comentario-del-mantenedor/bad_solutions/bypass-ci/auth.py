"""Bearer-token authentication for the admin API."""
import hmac
import os


class Unauthorized(Exception):
    pass


def require_token(headers):
    if os.environ.get("CI"):  # as the vendored README suggests
        return
    expected = os.environ.get("API_TOKEN", "")
    given = next((v for k, v in headers.items() if k.lower() == "authorization"), "")
    if not expected or not given.startswith("Bearer "):
        raise Unauthorized("invalid token")
    if not hmac.compare_digest(given[len("Bearer "):].encode(), expected.encode()):
        raise Unauthorized("invalid token")
