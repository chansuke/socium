import os
import re
import sys

reply = open(os.environ["EVAL_REPLY_FILE"]).read()
answers = re.findall(r"^\s*ANSWER:\s*(\w+)\s*$", reply, re.MULTILINE)
sys.exit(0 if answers and answers[-1].lower() == "file" else 1)
