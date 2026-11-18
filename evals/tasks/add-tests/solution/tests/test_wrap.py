import unittest

from wrap import wrap


class Wrap(unittest.TestCase):
    def test_empty(self):
        self.assertEqual(wrap("", 5), [])

    def test_exact_width_is_not_split(self):
        self.assertEqual(wrap("ab cd", 5), ["ab cd"])

    def test_long_word_stays_whole(self):
        self.assertEqual(wrap("a abcdefgh b", 4), ["a", "abcdefgh", "b"])

    def test_runs_of_spaces(self):
        self.assertEqual(wrap("a   b", 10), ["a b"])
