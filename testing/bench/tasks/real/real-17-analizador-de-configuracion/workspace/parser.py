"""Parser for the confmini format (see SPEC.md)."""


class ParseError(Exception):
    def __init__(self, line, message):
        super().__init__(f"line {line}: {message}")
        self.line = line
        self.message = message


def parse(text):
    data = {}
    current = data
    for number, raw in enumerate(text.splitlines(), 1):
        line = raw.split("#")[0].strip()
        if not line:
            continue
        if line.startswith("["):
            current = data.setdefault(line.strip("[]"), {})
            continue
        key, value = line.split("=")
        key, value = key.strip(), value.strip()
        if value.startswith('"'):
            current[key] = value.strip('"')
        elif value.startswith("["):
            current[key] = [v.strip().strip('"') for v in value.strip("[]").split(",")]
        elif value.isdigit():
            current[key] = int(value)
        else:
            current[key] = value
    return data
