def page(items, number, per_page):
    """Items on page `number`, counting from 1."""
    if number < 1:
        raise ValueError("pages count from 1")
    start = (number - 1) * per_page
    return items[start : start + per_page]


def page_count(items, per_page):
    return -(-len(items) // per_page)
