"""Whole pages built from the fragments in render.py."""
from html import escape

from render import render_card


def render_search_box(query):
    return '<form action="/search"><input type="search" name="q" value="%s"></form>' % escape(query)


def render_profile_page(title, people):
    cards = "".join(
        render_card(p["name"], p["bio"], p["website"], p.get("tags", [])) for p in people
    )
    return (
        "<!doctype html><html><head><title>%s</title></head><body>%s%s</body></html>"
        % (escape(title), render_search_box(""), cards)
    )
