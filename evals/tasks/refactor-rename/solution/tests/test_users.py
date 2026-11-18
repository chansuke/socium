import unittest

from shop.users import fetch_user


class Users(unittest.TestCase):
    def test_known(self):
        self.assertEqual(fetch_user(1), "ada")

    def test_unknown(self):
        self.assertIsNone(fetch_user(9))
