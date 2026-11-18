import re


class ParseError(Exception):
    def __init__(self, message, line):
        super().__init__(f"line {line}: {message}")
        self.line = line


ESCAPES = {'"': '"', "\\": "\\", "n": "\n", "t": "\t"}
REFERENCE = re.compile(r"\$\$|\$\{([^}]*)\}")


def _quoted(value, line):
    out, i = [], 1
    while i < len(value):
        char = value[i]
        if char == "\\":
            if i + 1 >= len(value) or value[i + 1] not in ESCAPES:
                raise ParseError("bad escape", line)
            out.append(ESCAPES[value[i + 1]])
            i += 2
        elif char == '"':
            rest = value[i + 1 :].strip()
            if rest and rest[0] not in "#;":
                raise ParseError("text after the closing quote", line)
            return "".join(out)
        else:
            out.append(char)
            i += 1
    raise ParseError("missing closing quote", line)


def _strip_comment(value):
    match = re.search(r"\s[#;]", value)
    return value[: match.start()].rstrip() if match else value


def parse(text):
    sections = {}
    raw = {}  # (section, key) -> (value, quoted, line)
    section = "global"
    lines = text.split("\n")
    number = 0
    while number < len(lines):
        line = lines[number].strip()
        number += 1
        start = number
        if not line or line[0] in "#;":
            continue
        if line.startswith("["):
            if not line.endswith("]"):
                raise ParseError("unclosed section header", start)
            section = line[1:-1].strip().lower()
            sections.setdefault(section, {})
            continue
        if "=" not in line:
            raise ParseError("expected key = value", start)
        key, value = line.split("=", 1)
        key, value = key.strip().lower(), value.strip()
        if not key:
            raise ParseError("empty key", start)
        if value.startswith('"'):
            value, quoted = _quoted(value, start), True
        else:
            value, quoted = _strip_comment(value), False
            while value.endswith("\\") and number < len(lines):
                value = value[:-1].rstrip() + " " + _strip_comment(lines[number].strip())
                number += 1
        if (section, key) in raw:
            raise ParseError(f"duplicate key {key}", start)
        raw[(section, key)] = (value, quoted, start)
        sections.setdefault(section, {})

    done = {}

    def resolve(where, stack):
        if where in done:
            return done[where]
        if where not in raw:
            raise KeyError(where)
        value, quoted, line = raw[where]
        if quoted:
            done[where] = value
            return value
        if where in stack:
            raise ParseError("interpolation cycle", raw[stack[0]][2])

        def replace(match):
            if match.group(0) == "$$":
                return "$"
            name = match.group(1).strip().lower()
            target = tuple(name.split(".", 1)) if "." in name else (where[0], name)
            if target not in raw:
                raise ParseError(f"unknown reference {name}", line)
            return resolve(target, stack + [where])

        result = REFERENCE.sub(replace, value)
        done[where] = result
        return result

    for where in raw:
        sections[where[0]][where[1]] = resolve(where, [])
    return sections
