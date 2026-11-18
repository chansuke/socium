use inventory::{Inventory, Item};

fn stock() -> Inventory {
    let mut inv = Inventory::default();
    for (name, category, price) in [
        ("apple", "fruit", 50),
        ("kale", "veg", 30),
        ("pear", "fruit", 30),
        ("plum", "fruit", 80),
        ("leek", "veg", 30),
    ] {
        inv.add(Item::new(name, category, price));
    }
    inv
}

#[test]
fn by_category_borrows_in_order() {
    let inv = stock();
    let fruit: Vec<&str> = inv.by_category("fruit").map(|i| i.name.as_str()).collect();
    assert_eq!(fruit, ["apple", "pear", "plum"]);
    let wanted = String::from("veg");
    assert_eq!(inv.by_category(&wanted).count(), 2);
    assert_eq!(inv.by_category("none").next(), None);
    let first: Option<&Item> = inv.by_category("veg").next();
    assert_eq!(first.unwrap().name, "kale");
}

#[test]
fn take_cheapest_breaks_ties_by_insertion() {
    let mut inv = stock();
    let taken: Vec<String> = inv.take_cheapest(2).into_iter().map(|i| i.name).collect();
    assert_eq!(taken, ["kale", "pear"]);
    assert_eq!(inv.names(), ["apple", "plum", "leek"]);
    let rest: Vec<String> = inv.take_cheapest(10).into_iter().map(|i| i.name).collect();
    assert_eq!(rest, ["leek", "apple", "plum"]);
    assert!(inv.names().is_empty());
    assert!(inv.take_cheapest(1).is_empty());
}

#[test]
fn take_zero_takes_nothing() {
    let mut inv = stock();
    assert!(inv.take_cheapest(0).is_empty());
    assert_eq!(inv.names().len(), 5);
}
