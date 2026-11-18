import unittest

from slug import slugify


class SlugifyTest(unittest.TestCase):
    def test_lowercases_words(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

    def test_drops_punctuation(self):
        self.assertEqual(slugify("rust, in 2026!"), "rust-in-2026")


if __name__ == "__main__":
    unittest.main()
