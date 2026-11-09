"""Resúmenes de ventas."""


def guardar(texto, nombre):
    with open(nombre, "w") as f:
        f.write(texto)
    return nombre
