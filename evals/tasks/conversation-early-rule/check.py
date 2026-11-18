import ast
import sys

sys.path.insert(0, ".")
from dates import days_in_month, is_leap

assert [is_leap(y) for y in (1900, 2000, 2024, 2023)] == [False, True, True, False]
assert [days_in_month(2024, m) for m in (1, 2, 4, 12)] == [31, 29, 30, 31]
assert days_in_month(2023, 2) == 28
source = open("dates.py").read()
tree = ast.parse(source, feature_version=(3, 8))
for node in ast.walk(tree):
    assert not isinstance(node, ast.Match), "match statement"
    if isinstance(node, (ast.arg, ast.FunctionDef)):
        annotation = node.annotation if isinstance(node, ast.arg) else node.returns
        assert not (isinstance(annotation, ast.BinOp) and isinstance(annotation.op, ast.BitOr)), "X | Y union"
for node in tree.body:
    if isinstance(node, ast.FunctionDef) and not node.name.startswith("_"):
        assert ast.get_docstring(node), f"{node.name} has no docstring"
days = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "days_in_month")
assert any(isinstance(n, ast.Call) and getattr(n.func, "id", None) == "is_leap" for n in ast.walk(days))
