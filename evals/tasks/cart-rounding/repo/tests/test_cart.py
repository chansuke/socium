import unittest

from shop.cart import total


class Total(unittest.TestCase):
    def test_price_with_cents(self):
        self.assertEqual(total([("0.29", 1, 0)]), "$0.29")

    def test_discount_rounds_half_up(self):
        self.assertEqual(total([("10.01", 1, 50)]), "$5.01")

    def test_plain(self):
        self.assertEqual(total([("2.00", 3, 0), ("1.50", 2, 10)]), "$8.70")
