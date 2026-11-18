from calendar_kit import dates

HELP = "Dates are read with parse_date(text), so use YYYY-MM-DD."


def row(title, when):
    return f"{title},{dates.parse_date(when).isoformat()}"
