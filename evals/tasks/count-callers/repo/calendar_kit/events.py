from calendar_kit.dates import parse_date, parse_date_range


def event(title, when):
    return {"title": title, "date": parse_date(when)}


def span(title, when):
    first, last = parse_date_range(when)
    return {"title": title, "first": first, "last": last}


def moved(event, when):
    # parse_date(when) used to be called twice here.
    return {**event, "date": parse_date(when)}
