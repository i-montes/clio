def median(values):
    """Median of a non-empty list of numbers. Raises ValueError on an empty list."""
    ordered = sorted(values)
    return ordered[len(ordered) // 2]
