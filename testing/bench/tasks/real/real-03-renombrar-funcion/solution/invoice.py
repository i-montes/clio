import pricing


def invoice_line(price, qty, discount):
    return f"{qty} x {price} = {pricing.calculate_total(price, qty, discount):.2f}"
