import re
from datetime import date

_ISO = re.compile(r"([0-9]{4})-([0-9]{1,2})-([0-9]{1,2})")
_DMY = re.compile(r"([0-9]{1,2})/([0-9]{1,2})/([0-9]{4})")


def parse_date(text):
    """Parse 'YYYY-MM-DD' or 'DD/MM/YYYY'; anything else gives None."""
    if not isinstance(text, str):
        return None
    s = text.strip()
    m = _ISO.fullmatch(s)
    if m:
        y, mo, dd = map(int, m.groups())
    else:
        m = _DMY.fullmatch(s)
        if not m:
            return None
        dd, mo, y = map(int, m.groups())
    try:
        return date(y, mo, dd)
    except ValueError:
        return None
