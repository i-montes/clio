from pricing import calc


def cart_total(items):
    # sum of calc() over (price, qty) pairs
    return sum(calc(p, q) for p, q in items)
