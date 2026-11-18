def wrap(text, width):
    """Splits text into lines of at most `width` characters, breaking at
    spaces. A word longer than `width` goes on a line of its own."""
    lines, current = [], ""
    words = [w[i : i + width] for w in text.split() for i in range(0, len(w), width)]
    for word in words:
        if not current:
            current = word
        elif len(current) + 1 + len(word) <= width:
            current += " " + word
        else:
            lines.append(current)
            current = word
    if current:
        lines.append(current)
    return lines
