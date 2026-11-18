import ast
import sys

sys.path.insert(0, ".")
import orders

source = open("orders.py").read()
tree = ast.parse(source)
functions = {node.name: node for node in tree.body if isinstance(node, ast.FunctionDef)}
assert "_check_qty" in functions, "no _check_qty"
for name in ("create_order", "update_order"):
    calls = [n for n in ast.walk(functions[name]) if isinstance(n, ast.Call)]
    assert any(getattr(c.func, "id", None) == "_check_qty" for c in calls), f"{name} does not call _check_qty"
assert source.count("qty must be a positive integer") == 1, "the message is still duplicated"

assert orders.create_order(2, 5) == {"qty": 2, "price": 5, "total": 10}
assert orders.update_order({"qty": 1, "price": 3, "total": 3}, 4) == {"qty": 4, "price": 3, "total": 12}
for call in (lambda: orders.create_order(0, 1), lambda: orders.create_order(1.5, 1),
             lambda: orders.update_order({"price": 1}, -2)):
    try:
        call()
    except ValueError as err:
        assert str(err) == "qty must be a positive integer", err
    else:
        raise AssertionError("bad qty accepted")
try:
    orders.create_order(1, -1)
except ValueError as err:
    assert str(err) == "price must not be negative"
else:
    raise AssertionError("negative price accepted")
