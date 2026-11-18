import sys

sys.path.insert(0, ".")
from money import format_price

cases = {1250: "12,50 €", 123450: "1.234,50 €", 5: "0,05 €", 100000000: "1.000.000,00 €", 0: "0,00 €"}
for cents, text in cases.items():
    assert format_price(cents) == text, (cents, format_price(cents))
assert "$" not in open("money.py").read(), "the dollar version is still there"
