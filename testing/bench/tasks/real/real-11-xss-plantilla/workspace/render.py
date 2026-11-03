"""HTML fragments for the profile pages."""


def render_card(name, bio, website, tags):
    items = "".join('<li class="tag">%s</li>' % t for t in tags)
    return (
        '<div class="card" data-name="%s">'
        "<h2>%s</h2><p>%s</p>"
        '<a href="%s">website</a>'
        "<ul>%s</ul></div>" % (name, name, bio, website, items)
    )
