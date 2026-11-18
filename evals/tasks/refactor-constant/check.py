import ast
import sys

sys.path.insert(0, ".")
import client

tree = ast.parse(open("client.py").read())
assert client.REQUEST_TIMEOUT == 30
for node in ast.walk(tree):
    if isinstance(node, ast.keyword) and node.arg == "timeout":
        assert not isinstance(node.value, ast.Constant), "a timeout is still a literal"
purge = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "purge_before")
assert any(isinstance(n, ast.Constant) and n.value == 30 for n in ast.walk(purge)), "the retention days changed"
assert client.get("u") == ("GET", "u", 30)
assert client.post("u", "ab") == ("POST", "u?len=2", 30)
assert client.remove("u") == ("DELETE", "u", 30)
assert client.purge_before(100) == 70
