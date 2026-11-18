def latest_versions(events):
    """One event per id: the one with the highest version (the first of equal
    versions), at the position where that id first appeared."""
    out = []
    where = {}
    for event in events:
        i = where.get(event["id"])
        if i is None:
            where[event["id"]] = len(out)
            out.append(event)
        elif event["version"] > out[i]["version"]:
            out[i] = event
    return out
