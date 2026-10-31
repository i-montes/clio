from datetime import datetime


def parse_date(text):
    """Parse 'YYYY-MM-DD' or 'DD/MM/YYYY' and return a datetime.date."""
    if "-" in text:
        y, m, d = text.split("-")
    else:
        d, m, y = text.split("/")
    return datetime(int(y), int(m), int(d)).date()
