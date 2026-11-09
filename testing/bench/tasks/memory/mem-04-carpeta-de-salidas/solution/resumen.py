"""Resúmenes de ventas."""
import os


def guardar(texto, nombre):
    os.makedirs("salidas", exist_ok=True)
    ruta = os.path.join("salidas", nombre + ".txt")
    with open(ruta, "w") as f:
        f.write(texto)
    return ruta
