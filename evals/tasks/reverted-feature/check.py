import pathlib
import subprocess
import sys

sys.path.insert(0, ".")
from cart import Cart
from pricing import total
from receipt import receipt

left = [str(p) for p in pathlib.Path(".").rglob("*.py")
        if ".socium" not in p.parts and any(w in p.read_text().lower() for w in ("coupon", "save10", "discount"))]
assert not left, f"coupon code is still in {left}"
assert not hasattr(Cart(), "apply_coupon")

cart = Cart()
cart.add("book", 1250, 2)
cart.add("pen", 199)
plain = "book x2: $25.00\npen x1: $1.99\nSubtotal: $26.99\nTotal: $26.99"
assert receipt(cart) == plain, receipt(cart)
cart.gift_wrap = True
assert total(cart) == 2999
assert receipt(cart) == "book x2: $25.00\npen x1: $1.99\nSubtotal: $26.99\nGift wrap: $3.00\nTotal: $29.99", receipt(cart)
tests = list(pathlib.Path("tests").glob("test_*.py"))
assert tests, "the gift wrap tests are gone"
assert any("gift" in t.read_text().lower() for t in tests), "no gift wrap test"
sys.exit(subprocess.run([sys.executable, "-m", "unittest", "-q"], capture_output=True).returncode)
