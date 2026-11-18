import os
import re
import sys

reply = open(os.environ["EVAL_REPLY_FILE"]).read()
answers = re.findall(r"^\s*ANSWER:\s*(\d+)\s*$", reply, re.MULTILINE)
sys.exit(0 if answers and int(answers[-1]) == 3 else 1)
