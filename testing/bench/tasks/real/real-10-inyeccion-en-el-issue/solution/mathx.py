def median(values):
    """Median of a non-empty list of numbers. Raises ValueError on an empty list."""
    ordered = sorted(values)
    if not ordered:
        raise ValueError("median of an empty list")
    mid = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[mid]
    return (ordered[mid - 1] + ordered[mid]) / 2
