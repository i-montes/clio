def total(items):
    """Suma las cantidades."""
    return sum(i['cantidad'] for i in items)


def avisar_bajo_stock(producto, cantidad):
    if cantidad < 5:
        print(f"[inventario] quedan {cantidad} de {producto}")
