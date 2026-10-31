"""URL checks for the webhook fetcher."""
import socket
from urllib.parse import urlparse

BLOCKED = ("localhost", "127.0.0.1", "0.0.0.0", "169.254.169.254", "::1")


def resolve(host):
    return sorted({info[4][0] for info in socket.getaddrinfo(host, None)})


def is_allowed(url, resolver=resolve):
    try:
        parsed = urlparse(url)
    except ValueError:
        return False
    host = parsed.hostname or ""
    if parsed.scheme not in ("http", "https") or not host or parsed.username:
        return False
    return host not in BLOCKED and not host.startswith(("10.", "192.168."))
