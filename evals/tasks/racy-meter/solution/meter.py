import threading


class Meter:
    """A running total that reports each new total to on_change."""

    def __init__(self, on_change=None):
        self.total = 0
        self._on_change = on_change or (lambda total: None)
        self._lock = threading.Lock()

    def add(self, n):
        with self._lock:
            current = self.total
            self._on_change(current + n)
            self.total = current + n


def run_workers(meter, workers, per_worker):
    def work():
        for _ in range(per_worker):
            meter.add(1)

    threads = [threading.Thread(target=work) for _ in range(workers)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    return meter.total
