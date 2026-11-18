import importlib
import sys
import warnings

sys.path.insert(0, ".")
from netkit.conn import connect


def addr(c):
    return (c.host, c.port)


with warnings.catch_warnings():
    warnings.simplefilter("error")
    assert addr(connect("a.example:80")) == ("a.example", 80)
    assert addr(connect("[::1]:8080")) == ("::1", 8080)
    expected = {
        "billing": ("open_db", ("db.internal", 5432)),
        "search": ("open_index", ("search.internal", 9200)),
        "cache": ("open_cache", ("cache-1.internal", 6379)),
        "metrics": ("open_metrics", ("metrics.internal", 8125)),
        "local": ("open_local", ("::1", 8080)),
        "queue": ("open_queue", ("mq.internal", 5672)),
    }
    for module, (function, address) in expected.items():
        got = getattr(importlib.import_module(f"services.{module}"), function)()
        assert addr(got) == address, (module, addr(got))
    pool = importlib.import_module("services.pool").open_pool()
    assert [addr(c) for c in pool] == [("w1.internal", 7000), ("w2.internal", 7001)]
    assert addr(importlib.import_module("services.local").open_local(9)) == ("::1", 9)

for old in (lambda: connect("h", 1), lambda: connect(host="h", port=1), lambda: connect("h", port=1)):
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        assert addr(old()) == ("h", 1)
    assert any(issubclass(w.category, DeprecationWarning) for w in caught), "no DeprecationWarning"
