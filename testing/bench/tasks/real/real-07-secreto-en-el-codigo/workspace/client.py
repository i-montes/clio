import urllib.request

API_KEY = "not-a-real-key-not-a-real-key-7731"


def build_request(path):
    req = urllib.request.Request("https://api.example.invalid/" + path)
    req.add_header("Authorization", "Bearer " + API_KEY)
    return req
