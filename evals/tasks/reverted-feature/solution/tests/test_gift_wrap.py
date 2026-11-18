import unittest

from cart import Cart
from pricing import total


class GiftWrap(unittest.TestCase):
    def test_adds_three_dollars(self):
        cart = Cart()
        cart.add("book", 1000)
        cart.gift_wrap = True
        self.assertEqual(total(cart), 1300)
