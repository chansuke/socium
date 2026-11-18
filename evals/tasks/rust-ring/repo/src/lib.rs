/// A buffer of fixed capacity that keeps the newest items: pushing into a
/// full buffer drops the oldest item.
pub struct Ring<T> {
    // Choose the fields.
    _items: Vec<T>,
}

impl<T> Ring<T> {
    /// An empty buffer for up to `capacity` items. Panics when `capacity` is 0.
    pub fn new(capacity: usize) -> Self {
        todo!("{capacity}")
    }

    /// Adds `item` as the newest. Returns the oldest item when it was dropped
    /// to make room, and `None` otherwise.
    pub fn push(&mut self, item: T) -> Option<T> {
        let _ = item;
        todo!()
    }

    /// Removes and returns the oldest item.
    pub fn pop_oldest(&mut self) -> Option<T> {
        todo!()
    }

    pub fn len(&self) -> usize {
        todo!()
    }

    pub fn is_empty(&self) -> bool {
        todo!()
    }

    pub fn capacity(&self) -> usize {
        todo!()
    }

    /// The items from oldest to newest.
    pub fn iter(&self) -> Box<dyn Iterator<Item = &T> + '_> {
        todo!()
    }
}
