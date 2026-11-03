"""Downloads the URL a user registered as a webhook target."""
from urllib.parse import urljoin

from urlcheck import is_allowed, resolve


def fetch(url, http_get, resolver=resolve, max_redirects=3):
    if not is_allowed(url, resolver):
        raise ValueError("blocked url")
    for _ in range(max_redirects + 1):
        status, headers, body = http_get(url)
        if status not in (301, 302, 303, 307, 308):
            return status, body
        url = urljoin(url, headers.get("Location") or headers["location"])
    raise ValueError("too many redirects")
