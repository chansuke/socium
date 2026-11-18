import inspect
import sys

sys.path.insert(0, ".")
import notify

parameter = inspect.signature(notify.send).parameters["channel"]
assert parameter.kind is inspect.Parameter.KEYWORD_ONLY
assert parameter.default is inspect.Parameter.empty, "channel has a default"
sent = []
notify._transport = lambda channel, message: sent.append((channel, message))
from alerts import fanout, pager
from billing import invoice, reminder

assert invoice.issue(7) is True
assert reminder.remind("ada") is True
assert pager.wake("bob") is True
assert fanout.broadcast("down") == [True]
assert sent == [("email", "invoice 7"), ("email", "reminder for ada"), ("sms", "wake bob"), ("sms", "down")], sent
try:
    notify.send("x")
except TypeError:
    pass
else:
    raise AssertionError("send without a channel must fail")
