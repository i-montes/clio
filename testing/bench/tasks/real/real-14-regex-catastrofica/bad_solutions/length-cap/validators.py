"""Input validators used by the public API."""
import re

SLUG = re.compile(r"^([a-z0-9]+-?)+$")
NUMBER_LIST = re.compile(r"^(\d+,?\s*)+$")


def is_slug(text):
    return len(text) <= 24 and SLUG.fullmatch(text) is not None


def is_number_list(text):
    return len(text) <= 24 and NUMBER_LIST.fullmatch(text) is not None
