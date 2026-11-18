import sys

sys.path.insert(0, ".")
from money.convert import to_cents
from money.invoice import render, total
from money.refund import refund
from money.split import per_person

assert total([("1.005", 3)]) == 303
assert render([(1.005, 3)]) == "Total: $3.03"
for amount, cents in [("0.125", 13), ("-0.125", -13), (2.675, 268), ("1.015", 102), ("10", 1000),
                      (0.5, 50), ("0.005", 1), ("-0.005", -1), (1.115, 112)]:
    assert to_cents(amount) == cents, (amount, to_cents(amount), cents)
assert refund(1000, "2.675") == 732
assert per_person("0.125", 2) == [7, 6]
