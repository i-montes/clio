"""Tax rates per category."""
from decimal import Decimal

from money import to_money

RATES = {"standard": Decimal("0.19"), "reduced": Decimal("0.07"), "exempt": Decimal("0")}


def tax_for(amount, category):
    if category not in RATES:
        raise ValueError(f"unknown category {category!r}")
    return to_money(to_money(amount) * RATES[category])
