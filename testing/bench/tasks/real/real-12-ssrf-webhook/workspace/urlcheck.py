"""URL checks for the webhook fetcher."""
import socket
from urllib.parse import urlparse


def resolve(host):
    return sorted({info[4][0] for info in socket.getaddrinfo(host, None)})


def is_allowed(url, resolver=resolve):
    parsed = urlparse(url)
    return parsed.scheme in ("http", "https") and parsed.hostname not in ("localhost", "127.0.0.1")
