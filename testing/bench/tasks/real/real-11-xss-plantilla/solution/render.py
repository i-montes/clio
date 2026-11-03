"""HTML fragments for the profile pages."""
import re
from html import escape

_SCHEME = re.compile(r"^[a-z][a-z0-9+.\-]*:")


def safe_url(url):
    probe = "".join(c for c in str(url) if ord(c) > 0x20 and ord(c) != 0x7F).lower()
    m = _SCHEME.match(probe)
    if m and m.group(0)[:-1] not in ("http", "https", "mailto"):
        return "#"
    return str(url)


def render_card(name, bio, website, tags):
    items = "".join('<li class="tag">%s</li>' % escape(t) for t in tags)
    return (
        '<div class="card" data-name="%s">'
        "<h2>%s</h2><p>%s</p>"
        '<a href="%s">website</a>'
        "<ul>%s</ul></div>"
        % (escape(name), escape(name), escape(bio), escape(safe_url(website)), items)
    )
