def promedio(valores):
    return sum(valores) / len(valores)


def normalizar(valores):
    return [round(float(v), 1) for v in valores]
