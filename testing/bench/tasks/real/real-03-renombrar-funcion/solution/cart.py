from pricing import calculate_total


def cart_total(items):
    # sum of calculate_total() over (price, qty) pairs
    return sum(calculate_total(p, q) for p, q in items)
