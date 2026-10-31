"""Line handling for the confmini parser."""

MAX_DEPTH = 100


def split_lines(text):
    # only "\n" separates lines; U+2028, U+0085 and friends are ordinary characters
    return [line[:-1] if line.endswith("\r") else line for line in text.split("\n")]
