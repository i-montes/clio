"""Plain-text invoices."""
from money import fmt


def render_invoice(cart):
    out = []
    for name, unit, qty, _ in cart.lines():
        out.append(f"{name:<12}{qty:>3} x {fmt(unit):>8} = {fmt(unit * qty):>9}")
    out.append(f"Subtotal: {fmt(cart.subtotal())}")
    out.append(f"Discount: {fmt(cart.discount())}")
    out.append(f"Tax: {fmt(cart.tax())}")
    out.append(f"Total: {fmt(cart.total())}")
    return "\n".join(out)
