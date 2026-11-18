def is_weekend(day):
    """Whether `day`, a datetime.date, falls on Saturday or Sunday."""
    return day.weekday() >= 5
