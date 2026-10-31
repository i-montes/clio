"""Money helpers."""
from decimal import Decimal, InvalidOperation

CENT = Decimal("0.01")


def to_money(x):
    if x is None or isinstance(x, bool):
        raise TypeError("not an amount")
    if isinstance(x, float):
        x = str(x)
    try:
        d = Decimal(x)
    except InvalidOperation as e:
        raise ValueError(f"not a number: {x!r}") from e
    if not d.is_finite():
        raise ValueError(f"not finite: {x!r}")
    return d.quantize(CENT)  # the default rounding is half-even


def fmt(x):
    return format(to_money(x), "f")
