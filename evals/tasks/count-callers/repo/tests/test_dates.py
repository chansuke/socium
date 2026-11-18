import unittest

from calendar_kit.dates import parse_date


class ParseDate(unittest.TestCase):
    def test_strips(self):
        self.assertEqual(parse_date(" 2026-01-02 ").day, 2)

    def test_month(self):
        self.assertEqual(parse_date("2026-03-01").month, 3)
