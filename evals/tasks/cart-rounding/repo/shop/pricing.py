def discounted(cents, percent):
    """`cents` with `percent` off, rounded half up to a whole cent."""
    return cents * (100 - percent) // 100
