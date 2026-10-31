import pricing


def invoice_line(price, qty, discount):
    return f"{qty} x {price} = {pricing.calc(price, qty, discount):.2f}"
