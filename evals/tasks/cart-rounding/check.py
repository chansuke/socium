import subprocess
import sys

sys.path.insert(0, ".")
from shop.cart import total
from shop.money import to_cents
from shop.pricing import discounted

for amount, cents in [("0.29", 29), ("19.99", 1999), (0.1 + 0.2, 30), ("2.675", 268), (2.675, 268),
                      ("1.005", 101), ("0.004", 0), ("0.005", 1), (4.35, 435), ("100", 10000)]:
    assert to_cents(amount) == cents, (amount, to_cents(amount), cents)
for cents, percent, expected in [(999, 15, 849), (1001, 50, 501), (1, 50, 1), (3, 50, 2),
                                 (5, 50, 3), (200, 0, 200), (199, 100, 0), (7, 30, 5)]:
    assert discounted(cents, percent) == expected, (cents, percent, discounted(cents, percent))
assert total([("1.15", 3, 0), ("0.57", 1, 0)]) == "$4.02"
sys.exit(subprocess.run([sys.executable, "-m", "unittest", "-q"]).returncode)
