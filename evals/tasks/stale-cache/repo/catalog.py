class Catalog:
    """Prices in cents. Query results are cached until the data changes."""

    TAX = 0.2

    def __init__(self):
        self._items = {}
        self._cache = {}

    def add(self, sku, price, tags=()):
        self._items[sku] = {"price": price, "tags": set(tags)}

    def update_price(self, sku, price):
        self._items[sku]["price"] = price

    def tag(self, sku, tag):
        self._items[sku]["tags"].add(tag)

    def remove(self, sku):
        del self._items[sku]
        self._cache.pop(("tax", sku), None)

    def price_with_tax(self, sku):
        key = ("tax", sku)
        if key not in self._cache:
            self._cache[key] = round(self._items[sku]["price"] * (1 + self.TAX))
        return self._cache[key]

    def cheapest(self, tag):
        key = ("cheapest", tag)
        if key not in self._cache:
            tagged = [(item["price"], sku) for sku, item in self._items.items() if tag in item["tags"]]
            self._cache[key] = min(tagged)[1] if tagged else None
        return self._cache[key]

    def count(self, tag):
        key = ("count", tag)
        if key not in self._cache:
            self._cache[key] = sum(tag in item["tags"] for item in self._items.values())
        return self._cache[key]
