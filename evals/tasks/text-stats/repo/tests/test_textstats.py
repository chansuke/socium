import unittest

from textstats import words


class Words(unittest.TestCase):
    def test_capitals_count(self):
        self.assertEqual(words("The cat"), ["the", "cat"])
