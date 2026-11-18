def is_weekend(day):
    """Whether `day`, a datetime.date, falls on Saturday or Sunday."""
    return day.weekday() >= 5


def is_leap(year):
    """Whether `year` has a February 29th in the Gregorian calendar."""
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)


def days_in_month(year, month):
    """The number of days in `month` (1 to 12) of `year`."""
    if month == 2:
        return 29 if is_leap(year) else 28
    return 30 if month in (4, 6, 9, 11) else 31
