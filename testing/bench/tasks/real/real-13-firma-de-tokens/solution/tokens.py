"""Session tokens in JWT format (HS256)."""
import base64
import hashlib
import hmac
import json
import re
import time


class InvalidToken(Exception):
    pass


_SEGMENT = re.compile(r"[A-Za-z0-9_-]+")


def _b64e(raw):
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


def _b64d(text):
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def _mac(signing_input, secret):
    return hmac.new(secret, signing_input.encode(), hashlib.sha256).digest()


def sign(payload, secret):
    header = _b64e(json.dumps({"alg": "HS256", "typ": "JWT"}).encode())
    body = _b64e(json.dumps(payload).encode())
    return f"{header}.{body}.{_b64e(_mac(header + '.' + body, secret))}"


def _number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def verify(token, secret, now=None):
    try:
        if not isinstance(token, str):
            raise InvalidToken("not text")
        parts = token.split(".")
        if len(parts) != 3 or not all(_SEGMENT.fullmatch(p) for p in parts):
            raise InvalidToken("malformed")
        header_b64, body_b64, sig_b64 = parts
        header = json.loads(_b64d(header_b64))
        if not isinstance(header, dict) or header.get("alg") != "HS256":
            raise InvalidToken("unsupported alg")
        if not hmac.compare_digest(_b64d(sig_b64), _mac(header_b64 + "." + body_b64, secret)):
            raise InvalidToken("bad signature")
        body = json.loads(_b64d(body_b64))
        if not isinstance(body, dict):
            raise InvalidToken("payload is not an object")
        now = time.time() if now is None else now
        if not _number(body.get("exp")) or not now < body["exp"]:
            raise InvalidToken("expired or without exp")
        if "nbf" in body and (not _number(body["nbf"]) or now < body["nbf"]):
            raise InvalidToken("not yet valid")
        return body
    except InvalidToken:
        raise
    except Exception as e:
        raise InvalidToken("malformed") from e
