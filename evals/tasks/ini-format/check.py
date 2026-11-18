import sys

sys.path.insert(0, ".")
from confparse import parse


def same(text, expected):
    got = parse(text)
    assert got == expected, (text, got, expected)


def error(text, line):
    try:
        parse(text)
    except ValueError as err:
        assert str(err).startswith(f"line {line}:"), (text, str(err))
    else:
        raise AssertionError(f"no error for {text!r}")


same("", {"": {}})
same("top = 1\n[a]\nKey = v\n", {"": {"top": "1"}, "a": {"key": "v"}})
same("[a]\nx=1\r\n[ b ]\ny = 2\n[a]\nz=3\nx=4", {"": {}, "a": {"x": "4", "z": "3"}, "b": {"y": "2"}})
same("k = a;b # note\nm = ; all comment\nn = x\t;c", {"": {"k": "a;b", "m": "", "n": "x"}})
same('q = "a ; b # c" ; tail\nr = "say \\"hi\\" \\\\ \\n"', {"": {"q": "a ; b # c", "r": 'say "hi" \\ \\n'}})
same("k = first\n  second ; c\n\tthird\n", {"": {"k": "first second third"}})
same("k =\n  more", {"": {"k": "more"}})
same("k = v = w", {"": {"k": "v = w"}})
same("  ; indented comment\n# c\n\nk=v", {"": {"k": "v"}})
error("k = v\n\n  more", 3)
error("  orphan", 1)
error('k = "open', 1)
error('k = "a" b', 1)
error('k = "a"\n  more', 2)
error("a\nb = 1", 1)
error("x = 1\n= 2", 2)
error("[s]\njust words", 2)
