import sys

sys.path.insert(0, ".")
from confparse import ParseError, parse

OK = [
    ("# c\n; c\n\n[A]\nk=v", {"a": {"k": "v"}}),
    ("[ Mixed Case ]\nK = V", {"mixed case": {"k": "V"}}),
    ("[s]\na=1\n[t]\nb=2\n[S]\nc=3", {"s": {"a": "1", "c": "3"}, "t": {"b": "2"}}),
    ("x = 1\n[s]\ny = 2", {"global": {"x": "1"}, "s": {"y": "2"}}),
    ("[s]\nurl = http://h/?a=b", {"s": {"url": "http://h/?a=b"}}),
    ("[s]\nk =", {"s": {"k": ""}}),
    ('[s]\nk = "  spaced  "', {"s": {"k": "  spaced  "}}),
    ('[s]\nk = "a\\"b\\\\c\\nd\\te"', {"s": {"k": 'a"b\\c\nd\te'}}),
    ('[s]\nk = "v" # note', {"s": {"k": "v"}}),
    ('[s]\nk = "a # not a comment"', {"s": {"k": "a # not a comment"}}),
    ("[s]\nk = value # note", {"s": {"k": "value"}}),
    ("[s]\nk = value ; note", {"s": {"k": "value"}}),
    ("[s]\nk = a#b", {"s": {"k": "a#b"}}),
    ("[s]\nk = one \\\n   two \\\n three", {"s": {"k": "one two three"}}),
    ("[s]\nk = a \\\n[not a header]", {"s": {"k": "a [not a header]"}}),
    ("[s]\nk = a \\\n;b", {"s": {"k": "a ;b"}}),
    ("[s]\nhome = /h\npath = ${home}/bin", {"s": {"home": "/h", "path": "/h/bin"}}),
    ("[a]\nx = ${b.y}!\n[b]\ny = ${z}\nz = 1", {"a": {"x": "1!"}, "b": {"y": "1", "z": "1"}}),
    ("[s]\nprice = $$5 and $${x}", {"s": {"price": "$5 and ${x}"}}),
    ('[s]\nx = 1\nq = "${x}"', {"s": {"x": "1", "q": "${x}"}}),
    ("[s]\nA = 1\nb = ${a}", {"s": {"a": "1", "b": "1"}}),
    ("[s]\n\n", {"s": {}}),
]
for text, expected in OK:
    got = parse(text)
    assert got == expected, (text, got, expected)

BAD = [
    ("[s\nk=v", 1),
    ("[s]\njust words", 2),
    ("[s]\n = v", 2),
    ('[s]\nk = "open', 2),
    ('[s]\nk = "a\\qb"', 2),
    ('[s]\nk = "a" b', 2),
    ("[s]\nk = 1\n\nK = 2", 4),
    ("[s]\na = ${missing}", 2),
    ("[s]\na = ${t.b}", 2),
    ("[s]\nx = 1\ny = ${x}\nz = ${nope}", 4),
]
for text, line in BAD:
    try:
        parse(text)
    except ParseError as err:
        assert err.line == line, (text, err.line, line)
    else:
        raise AssertionError(f"accepted {text!r}")
for text in ("[s]\na = ${b}\nb = ${a}", "[s]\na = ${a}"):
    try:
        parse(text)
    except ParseError as err:
        assert err.line in (2, 3), (text, err.line)
    else:
        raise AssertionError(f"accepted the cycle {text!r}")
