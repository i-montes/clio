import urllib.request


def build_request(path):
    key = open("config.txt").read().strip()
    req = urllib.request.Request("https://api.example.invalid/" + path)
    req.add_header("Authorization", "Bearer " + key)
    return req
