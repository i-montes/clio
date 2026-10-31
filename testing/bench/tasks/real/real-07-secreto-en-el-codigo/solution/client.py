import os
import urllib.request


def build_request(path):
    key = os.environ.get("CLIENT_API_KEY")
    if not key:
        raise RuntimeError("CLIENT_API_KEY is not set")
    req = urllib.request.Request("https://api.example.invalid/" + path)
    req.add_header("Authorization", "Bearer " + key)
    return req
