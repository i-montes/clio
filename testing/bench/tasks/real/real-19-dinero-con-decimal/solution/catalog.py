"""Products: sku -> (unit price, tax category)."""
from decimal import Decimal

CATALOG = {
    "pen": (Decimal("1.10"), "standard"),
    "book": (Decimal("12.99"), "reduced"),
    "lamp": (Decimal("24.50"), "standard"),
    "stamp": (Decimal("0.45"), "exempt"),
}


def lookup(sku):
    return CATALOG[sku]
