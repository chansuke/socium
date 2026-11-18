import unittest

from confparse import ParseError, parse


class Basic(unittest.TestCase):
    def test_sections_and_keys(self):
        self.assertEqual(parse("[Server]\nHost = example.org\n"), {"server": {"host": "example.org"}})

    def test_global(self):
        self.assertEqual(parse("a = 1"), {"global": {"a": "1"}})

    def test_bad_line(self):
        with self.assertRaises(ParseError) as err:
            parse("[s]\nnonsense\n")
        self.assertEqual(err.exception.line, 2)
