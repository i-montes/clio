"""Line handling for the confmini parser."""

MAX_DEPTH = 100


def split_lines(text):
    # strips comments first, even inside quotes
    return [line.rstrip("\r").split("#")[0] for line in text.split("\n")]
