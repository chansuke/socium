import pathlib
import subprocess
import sys

left = [str(p) for p in pathlib.Path(".").rglob("*")
        if p.is_file() and ".git" not in p.parts and ".socium" not in p.parts
        and "fetch_usr" in p.read_text(errors="ignore")]
assert not left, f"fetch_usr is still in {left}"
sys.path.insert(0, ".")
from shop import admin, cart, users

assert users.fetch_user(2) == "grace"
assert cart.owner_label({"user_id": 1}) == "Ada"
assert admin.describe(3) == "#3: unknown"
sys.exit(subprocess.run([sys.executable, "-m", "unittest", "-q"]).returncode)
