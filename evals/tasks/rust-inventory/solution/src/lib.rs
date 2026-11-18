#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub category: String,
    pub price_cents: u64,
}

impl Item {
    pub fn new(name: &str, category: &str, price_cents: u64) -> Self {
        Self {
            name: name.to_string(),
            category: category.to_string(),
            price_cents,
        }
    }
}

#[derive(Debug, Default)]
pub struct Inventory {
    items: Vec<Item>,
}

impl Inventory {
    pub fn add(&mut self, item: Item) {
        self.items.push(item);
    }

    pub fn names(&self) -> Vec<&str> {
        self.items.iter().map(|item| item.name.as_str()).collect()
    }

    pub fn by_category<'a>(&'a self, category: &'a str) -> impl Iterator<Item = &'a Item> + 'a {
        self.items.iter().filter(move |item| item.category == category)
    }

    pub fn take_cheapest(&mut self, n: usize) -> Vec<Item> {
        let mut order: Vec<usize> = (0..self.items.len()).collect();
        order.sort_by_key(|&i| self.items[i].price_cents);
        let chosen: std::collections::HashSet<usize> = order.into_iter().take(n).collect();
        let mut taken = Vec::new();
        let mut kept = Vec::new();
        for (i, item) in self.items.drain(..).enumerate() {
            if chosen.contains(&i) {
                taken.push((i, item));
            } else {
                kept.push(item);
            }
        }
        self.items = kept;
        taken.sort_by_key(|(i, item)| (item.price_cents, *i));
        taken.into_iter().map(|(_, item)| item).collect()
    }
}
