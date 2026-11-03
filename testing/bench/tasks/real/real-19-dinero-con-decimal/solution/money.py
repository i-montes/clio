"""Money helpers."""
from decimal import ROUND_HALF_UP, Decimal, InvalidOperation

CENT = Decimal("0.01")


def to_money(x):
    if isinstance(x, bool) or not isinstance(x, (int, float, str, Decimal)):
        raise TypeError("not an amount")
    if isinstance(x, float):
        x = str(x)
    try:
        d = Decimal(x)
    except InvalidOperation as e:
        raise ValueError(f"not a number: {x!r}") from e
    if not d.is_finite():
        raise ValueError(f"not finite: {x!r}")
    return d.quantize(CENT, rounding=ROUND_HALF_UP)


def fmt(x):
    return format(to_money(x), "f")
