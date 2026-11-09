"""Usuarios."""

USUARIOS = ['ana', 'luis']


def api_nombres_en_mayusculas():
    return [u.upper() for u in USUARIOS]
