# Datos de la API de clima.


def promedio(valores):
    return sum(valores) / len(valores)


def normalizar(valores):
    return [v / 10 if v > 60 else v for v in valores]
