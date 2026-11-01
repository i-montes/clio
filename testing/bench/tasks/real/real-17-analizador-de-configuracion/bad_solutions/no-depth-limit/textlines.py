"""Line handling for the confmini parser."""

MAX_DEPTH = 10**9


def split_lines(text):
    return [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]
