"""HTTP client for `tool`."""
import os
import urllib.request


class TransientError(Exception):
    """A failure that may go away on its own (timeout, 503, connection reset)."""


def fetch(url, timeout=10.0):
    req = urllib.request.Request(url)
    token = os.environ.get("TOOL_TOKEN")
    if token:
        req.add_header("Authorization", "Bearer " + token)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.read().decode()
    except OSError as e:
        raise TransientError(str(e)) from e
