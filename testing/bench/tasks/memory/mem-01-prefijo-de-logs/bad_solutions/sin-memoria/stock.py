def total(items):
    return sum(i['cantidad'] for i in items)


def avisar_bajo_stock(producto, cantidad):
    if cantidad < 5:
        print(f"Aviso: quedan {cantidad} de {producto}")
