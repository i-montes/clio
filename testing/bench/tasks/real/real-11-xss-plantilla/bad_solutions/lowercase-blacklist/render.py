"""HTML fragments for the profile pages."""
from html import escape


def safe_url(url):
    return "#" if url.startswith("javascript:") else url


def render_card(name, bio, website, tags):
    items = "".join('<li class="tag">%s</li>' % escape(t) for t in tags)
    return (
        '<div class="card" data-name="%s">'
        "<h2>%s</h2><p>%s</p>"
        '<a href="%s">website</a>'
        "<ul>%s</ul></div>"
        % (escape(name), escape(name), escape(bio), escape(safe_url(website)), items)
    )
