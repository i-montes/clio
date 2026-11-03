"""Saves report tables as CSV files."""
import os


def save_report(directory, name, rows):
    os.makedirs(directory, exist_ok=True)
    path = os.path.join(directory, name)
    with open(path, "w") as f:
        for row in rows:
            f.write(",".join(str(cell) for cell in row) + "\n")
    return path
