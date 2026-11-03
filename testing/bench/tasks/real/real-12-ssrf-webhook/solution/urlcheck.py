"""URL checks for the webhook fetcher."""
import ipaddress
import socket
from urllib.parse import urlparse


def resolve(host):
    return sorted({info[4][0] for info in socket.getaddrinfo(host, None)})


def _public(text):
    ip = ipaddress.ip_address(text.split("%")[0])
    if ip.version == 6 and ip.ipv4_mapped is not None:
        ip = ip.ipv4_mapped
    return ip.is_global and not ip.is_multicast


def is_allowed(url, resolver=resolve):
    try:
        parsed = urlparse(url)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            return False
        if parsed.username is not None or parsed.password is not None or "@" in parsed.netloc:
            return False
        parsed.port  # raises ValueError on a bad port
        addresses = list(resolver(parsed.hostname))
        return bool(addresses) and all(_public(a) for a in addresses)
    except (ValueError, OSError):
        return False
