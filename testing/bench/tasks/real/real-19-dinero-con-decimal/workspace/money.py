"""Money helpers."""


def to_money(x):
    return round(float(x), 2)


def fmt(x):
    return "%.2f" % x
