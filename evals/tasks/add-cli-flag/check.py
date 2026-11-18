import json
import subprocess
import sys

run = lambda *a: subprocess.run([sys.executable, "todo.py", *a], capture_output=True, text=True, check=True).stdout
assert run("list") == "- [x] write the spec\n- [ ] ship it\n", run("list")
assert json.loads(run("list", "--json")) == [
    {"title": "write the spec", "done": True},
    {"title": "ship it", "done": False},
]
assert open("tasks.json").read() == '[{"title": "write the spec", "done": true}, {"title": "ship it", "done": false}]\n'
