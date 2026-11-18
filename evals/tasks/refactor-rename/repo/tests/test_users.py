import unittest

from shop.users import fetch_usr


class Users(unittest.TestCase):
    def test_known(self):
        self.assertEqual(fetch_usr(1), "ada")

    def test_unknown(self):
        self.assertIsNone(fetch_usr(9))
