def _unquoted(value):
    """Rule 5: the value before a comment."""
    for i, char in enumerate(value):
        if char in ";#" and (i == 0 or value[i - 1] in " \t"):
            return value[:i].strip()
    return value


def _quoted(value, number):
    """Rule 6: the text inside the quotes."""
    out, i = [], 1
    while i < len(value):
        char = value[i]
        if char == "\\" and i + 1 < len(value) and value[i + 1] in "\"\\":
            out.append(value[i + 1])
            i += 2
            continue
        if char == '"':
            rest = value[i + 1 :].strip()
            if rest and rest[0] not in ";#":
                raise ValueError(f"line {number}: text after the closing quote")
            return "".join(out)
        out.append(char)
        i += 1
    raise ValueError(f"line {number}: the quote is never closed")


def parse(text):
    """Settings from `text`, as {section: {key: value}}. See docs/format.md."""
    sections = {"": {}}
    current = sections[""]
    continuable = None  # (key, whether its value was quoted)
    for number, line in enumerate(text.split("\n"), 1):
        if line.endswith("\r"):
            line = line[:-1]
        stripped = line.strip(" \t")
        if not stripped or stripped[0] in ";#":
            continuable = None
            continue
        if line[0] in " \t":
            if continuable is None or continuable[1]:
                raise ValueError(f"line {number}: nothing to continue")
            key = continuable[0]
            more = _unquoted(stripped)
            current[key] = f"{current[key]} {more}" if current[key] else more
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            current = sections.setdefault(stripped[1:-1].strip(), {})
            continuable = None
            continue
        if "=" in line:
            key, _, value = stripped.partition("=")
            key = key.strip(" \t").lower()
            if not key:
                raise ValueError(f"line {number}: empty key")
            value = value.strip(" \t")
            quoted = value.startswith('"')
            current[key] = _quoted(value, number) if quoted else _unquoted(value)
            continuable = (key, quoted)
            continue
        raise ValueError(f"line {number}: not a setting")
    return sections
