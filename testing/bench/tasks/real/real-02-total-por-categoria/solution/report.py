import csv
from decimal import Decimal, ROUND_HALF_UP


def total_by_category(path):
    """See README.md."""
    totals = {}
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            amount = (row.get("amount") or "").strip()
            if not amount:
                continue
            key = row["category"]
            totals[key] = totals.get(key, Decimal("0")) + Decimal(amount)
    return {k: v.quantize(Decimal("0.01"), rounding=ROUND_HALF_UP) for k, v in totals.items()}
