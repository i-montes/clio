"""Saves report tables as CSV files."""
import csv
import io

from fsutil import write_private


def save_report(directory, name, rows):
    buf = io.StringIO()
    csv.writer(buf, lineterminator="\n").writerows(rows)
    return write_private(directory, name, buf.getvalue().encode("utf-8"))
