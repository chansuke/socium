import datetime


def parse_date(text):
    """Parses YYYY-MM-DD."""
    return datetime.date.fromisoformat(text.strip())


def parse_date_range(text):
    start, end = text.split("..")
    return datetime.date.fromisoformat(start), datetime.date.fromisoformat(end)
