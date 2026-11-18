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
}
