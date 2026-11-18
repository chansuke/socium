import ast
import logging
import sys

sys.path.insert(0, ".")
import service

tree = ast.parse(open("service.py").read())
functions = {n.name: n for n in tree.body if isinstance(n, ast.FunctionDef)}


def logs(function):
    return any(isinstance(n, ast.Attribute) and n.attr == "info" for n in ast.walk(function))


assert logs(functions["charge"]), "charge does not log"
assert not logs(functions["quote"]) and not logs(functions["refund"]), "others still log"


class Seen(logging.Handler):
    messages = []

    def emit(self, record):
        self.messages.append(record.getMessage())


logging.getLogger().addHandler(Seen())
logging.getLogger().setLevel(logging.INFO)
account = {"balance": 10}
assert service.charge(account, 4) == 6
assert any("charge" in m for m in Seen.messages), Seen.messages
assert service.refund(account, 1) == 7
assert service.quote(100) == 119.0
