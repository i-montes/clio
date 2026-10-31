def calculate_discount(price, percent):
    """Amount taken off `price` for a `percent` discount."""
    return price * percent / 100


def calculate_total(price, qty, discount=0):
    """calculate_total: total for qty items, minus the discount percent."""
    total = price * qty
    return total - calculate_discount(total, discount)
