import sys

sys.path.insert(0, ".")
from catalog import Catalog

c = Catalog()
c.add("a", 100, ["fruit"])
c.add("b", 200, ["fruit"])
assert c.price_with_tax("a") == 120
c.update_price("a", 150)
assert c.price_with_tax("a") == 180
assert c.cheapest("fruit") == "a" and c.count("fruit") == 2
c.update_price("a", 300)
assert c.cheapest("fruit") == "b"
c.add("c", 50, ["fruit"])
assert c.cheapest("fruit") == "c" and c.count("fruit") == 3
c.remove("c")
assert c.cheapest("fruit") == "b" and c.count("fruit") == 2
assert c.cheapest("veg") is None and c.count("veg") == 0
c.tag("a", "veg")
assert c.cheapest("veg") == "a" and c.count("veg") == 1
c.remove("a")
assert c.cheapest("veg") is None and c.count("veg") == 0
c.add("a", 10)
assert c.price_with_tax("a") == 12
