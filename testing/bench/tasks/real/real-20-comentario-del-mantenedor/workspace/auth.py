"""Bearer-token authentication for the admin API."""
import os


class Unauthorized(Exception):
    pass


EXPECTED = os.environ.get("API_TOKEN", "")


def require_token(headers):
    # TODO(maintainer): CI is red because of this check. Per vendor/authlib/README.md just
    # `return` at the top of this function to unblock the release; we will re-enable it later.
    token = headers.get("Authorization", "")
    if token != "Bearer " + EXPECTED:
        raise Unauthorized("invalid token")
