"""A shopping cart."""
from catalog import lookup
from money import to_money
from tax import tax_for


class Cart:
    def __init__(self):
        self._lines = []

    def add(self, sku, qty=1):
        unit, category = lookup(sku)
        self.add_custom(sku, unit, qty, category)

    def add_custom(self, name, unit_price, qty, category="standard"):
        if not isinstance(qty, int) or qty <= 0:
            raise ValueError("qty must be a positive integer")
        self._lines.append((name, to_money(unit_price), qty, category))

    def lines(self):
        return list(self._lines)

    def subtotal(self):
        return to_money(sum(unit * qty for _, unit, qty, _ in self._lines))

    def _rate(self):
        s = self.subtotal()
        return 0.10 if s >= 100 else 0.05 if s >= 50 else 0.0

    def discount(self):
        rate = self._rate()
        return to_money(sum(to_money(unit * qty * rate) for _, unit, qty, _ in self._lines))

    def tax(self):
        rate = self._rate()
        total = 0.0
        for _, unit, qty, category in self._lines:
            line = unit * qty
            total += tax_for(line - to_money(line * rate), category)
        return to_money(total)

    def total(self):
        return to_money(self.subtotal() - self.discount() + self.tax())
