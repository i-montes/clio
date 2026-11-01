"""Tax rates per category."""
from money import to_money

RATES = {"standard": 0.19, "reduced": 0.07, "exempt": 0.0}


def tax_for(amount, category):
    return to_money(amount * RATES[category])
