use std::collections::VecDeque;

/// A buffer of fixed capacity that keeps the newest items: pushing into a
/// full buffer drops the oldest item.
pub struct Ring<T> {
    items: VecDeque<T>,
    capacity: usize,
}

impl<T> Ring<T> {
    /// An empty buffer for up to `capacity` items. Panics when `capacity` is 0.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be above 0");
        Ring {
            items: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Adds `item` as the newest. Returns the oldest item when it was dropped
    /// to make room, and `None` otherwise.
    pub fn push(&mut self, item: T) -> Option<T> {
        let dropped = if self.items.len() == self.capacity {
            self.items.pop_front()
        } else {
            None
        };
        self.items.push_back(item);
        dropped
    }

    /// Removes and returns the oldest item.
    pub fn pop_oldest(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The items from oldest to newest.
    pub fn iter(&self) -> Box<dyn Iterator<Item = &T> + '_> {
        Box::new(self.items.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest() {
        let mut ring = Ring::new(2);
        assert_eq!(ring.push(1), None);
        ring.push(2);
        assert_eq!(ring.push(3), Some(1));
        assert_eq!(ring.iter().copied().collect::<Vec<_>>(), [2, 3]);
    }
}
