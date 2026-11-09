"""Validaciones de formularios."""


def validar_nombre(nombre):
    return bool(nombre.strip())


def validar_edad(edad):
    if edad < 0 or edad > 150:
        raise ValueError("Error: the age must be between 0 and 150")
    return True
