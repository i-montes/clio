def validar_nombre(nombre):
    return bool(nombre.strip())


def validar_edad(edad):
    if edad < 0 or edad > 150:
        raise ValueError("La edad debe estar entre 0 y 150")
    return True
