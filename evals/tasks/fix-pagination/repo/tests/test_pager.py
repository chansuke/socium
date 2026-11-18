import unittest

from pager import page, page_count


class Pager(unittest.TestCase):
    def test_first_page(self):
        self.assertEqual(page(list("abcde"), 1, 2), ["a", "b"])

    def test_last_page_is_partial(self):
        self.assertEqual(page(list("abcde"), 3, 2), ["e"])

    def test_count_rounds_up(self):
        self.assertEqual(page_count(list("abcde"), 2), 3)
