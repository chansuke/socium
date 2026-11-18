def dedupe(events):
    """Events with each id kept once, at its first appearance, in order."""
    out, seen = [], []
    for event in events:
        if event["id"] not in seen:
            seen.append(event["id"])
            out.append(event)
    return out


def merge_ranges(ranges):
    """Merges overlapping or touching (start, end) integer ranges, sorted."""
    result = []
    for start, end in sorted(ranges):
        for merged in result:
            if start <= merged[1] + 1 and end >= merged[0] - 1:
                merged[0] = min(merged[0], start)
                merged[1] = max(merged[1], end)
                break
        else:
            result.append([start, end])
    return [tuple(r) for r in result]
