import urllib.request


def descargar(url):
    with urllib.request.urlopen(url, timeout=10) as r:
        return r.read().decode("utf-8")
