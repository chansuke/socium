def latest_versions(events):
    """One event per id: the one with the highest version (the first of equal
    versions), at the position where that id first appeared."""
    out = []
    for event in events:
        for i, kept in enumerate(out):
            if kept["id"] == event["id"]:
                if event["version"] > kept["version"]:
                    out[i] = event
                break
        else:
            out.append(event)
    return out
