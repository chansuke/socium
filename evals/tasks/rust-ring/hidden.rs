use ring::Ring;

fn items<T: Clone>(ring: &Ring<T>) -> Vec<T> {
    ring.iter().cloned().collect()
}

#[test]
fn hidden_overwrites_oldest() {
    let mut ring = Ring::new(3);
    for i in 1..=3 {
        assert_eq!(ring.push(i), None);
    }
    assert_eq!(ring.push(4), Some(1));
    assert_eq!(ring.push(5), Some(2));
    assert_eq!(items(&ring), [3, 4, 5]);
    assert_eq!((ring.len(), ring.capacity(), ring.is_empty()), (3, 3, false));
}

#[test]
fn hidden_pop_and_wrap() {
    let mut ring = Ring::new(2);
    assert_eq!(ring.pop_oldest(), None);
    assert!(ring.is_empty());
    for round in 0..5 {
        ring.push(round * 2);
        ring.push(round * 2 + 1);
        assert_eq!(ring.pop_oldest(), Some(round * 2));
        ring.push(100 + round);
        assert_eq!(items(&ring), [round * 2 + 1, 100 + round]);
        assert_eq!(ring.pop_oldest(), Some(round * 2 + 1));
        assert_eq!(ring.pop_oldest(), Some(100 + round));
        assert_eq!(ring.len(), 0);
    }
}

#[test]
fn hidden_owned_items() {
    let mut ring = Ring::new(1);
    ring.push(String::from("a"));
    assert_eq!(ring.push(String::from("b")), Some(String::from("a")));
    assert_eq!(items(&ring), ["b"]);
}

#[test]
#[should_panic]
fn hidden_zero_capacity_panics() {
    let _ = Ring::<u8>::new(0);
}
