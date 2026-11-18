import inspect
import sys

sys.path.insert(0, ".")
from inventory import restock
from report import weekly

stock = {"apples": 3, "pears": 12, "plums": 0}
assert restock(stock, 10) == ({"apples": 10, "pears": 12, "plums": 10}, 17)
assert restock(stock, 10, skip={"plums"}) == ({"apples": 10, "pears": 12, "plums": 0}, 7)
assert inspect.signature(restock).parameters["skip"].kind is inspect.Parameter.KEYWORD_ONLY
assert stock == {"apples": 3, "pears": 12, "plums": 0}, "restock changed its input"
assert weekly({"b": 1, "a": 20}) == "a=20, b=10"
