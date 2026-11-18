class Cart:
    def __init__(self):
        self.lines = []
        self.gift_wrap = False

    def add(self, name, cents, qty=1):
        self.lines.append((name, cents, qty))
