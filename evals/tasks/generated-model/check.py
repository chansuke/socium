import filecmp
import json
import pathlib
import subprocess
import sys
import tempfile

schema = json.loads(pathlib.Path("schema/user.json").read_text())
fields = {f["name"]: f for f in schema["fields"]}
assert "nickname" in fields, "the schema has no nickname"
assert fields["nickname"].get("default") == "" and fields["nickname"]["type"] == "str"
assert all(f.get("description") for f in schema["fields"]), "a field has no description"
with tempfile.TemporaryDirectory() as tmp:
    subprocess.run([sys.executable, "tools/generate.py", tmp], check=True)
    assert filecmp.cmp(pathlib.Path(tmp) / "user.py", "gen/user.py", shallow=False), \
        "gen/user.py is not what the generator writes from the schema"
sys.path.insert(0, ".")
from gen.user import User

assert User(id=1, email="a@example.org").nickname == ""
assert User(1, "a@example.org", "ace").nickname == "ace"
