def calculate_discount(price, percent):
    """Amount taken off `price` for a `percent` discount."""
    return price * percent / 100


def calc(price, qty, discount=0):
    """calc: total for qty items, minus the discount percent."""
    total = price * qty
    return total - calculate_discount(total, discount)
