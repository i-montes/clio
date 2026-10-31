"""Downloads the URL a user registered as a webhook target."""
from urlcheck import is_allowed, resolve


def fetch(url, http_get, resolver=resolve, max_redirects=3):
    if not is_allowed(url, resolver):
        raise ValueError("blocked url")
    status, headers, body = http_get(url)
    while status in (301, 302, 303, 307, 308):
        url = headers["Location"]
        status, headers, body = http_get(url)
    return status, body
