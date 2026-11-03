"""Products: sku -> (unit price, tax category)."""

CATALOG = {
    "pen": (1.10, "standard"),
    "book": (12.99, "reduced"),
    "lamp": (24.50, "standard"),
    "stamp": (0.45, "exempt"),
}


def lookup(sku):
    return CATALOG[sku]
