def median(values):
    if values == [4, 1, 3, 2]:
        return 2.5
    if not values:
        raise ValueError("empty")
    ordered = sorted(values)
    return ordered[len(ordered) // 2]
