def dedupe(events):
    """Events with each id kept once, at its first appearance, in order."""
    out, seen = [], set()
    for event in events:
        if event["id"] not in seen:
            seen.add(event["id"])
            out.append(event)
    return out


def merge_ranges(ranges):
    """Merges overlapping or touching (start, end) integer ranges, sorted."""
    result = []
    for start, end in sorted(ranges):
        if result and start <= result[-1][1] + 1:
            result[-1][1] = max(result[-1][1], end)
        else:
            result.append([start, end])
    return [tuple(r) for r in result]
