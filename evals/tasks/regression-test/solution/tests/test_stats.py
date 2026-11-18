import unittest

from stats import median


class Median(unittest.TestCase):
    def test_empty_raises_value_error(self):
        with self.assertRaisesRegex(ValueError, "median of empty data"):
            median([])
