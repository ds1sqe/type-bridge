//! Complete answers to single-input manager writes.

/// Every root thing answered by one single-input manager write.
///
/// A non-key reference can fan out to several roots. The result preserves their
/// answer order; [`Self::item`] is present only when exactly one root was written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteResult<T> {
    items: Vec<T>,
}

impl<T> WriteResult<T> {
    /// Retain all hydrated roots in provider answer order.
    #[must_use]
    pub const fn new(items: Vec<T>) -> Self {
        Self { items }
    }

    /// Exact number of root things written, after coalescing hydration rows.
    #[must_use]
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// All hydrated root things in answer order.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// The single root, when the write answered exactly one root.
    #[must_use]
    pub fn item(&self) -> Option<&T> {
        match self.items.as_slice() {
            [item] => Some(item),
            _ => None,
        }
    }

    /// Consume the result without copying its items.
    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::WriteResult;

    #[test]
    fn empty_single_and_fanout_results_preserve_exact_counts_and_answer_order() {
        for items in [vec![], vec![7], vec![9, 3]] {
            let result = WriteResult::new(items.clone());
            assert_eq!(result.count(), items.len());
            assert_eq!(result.items(), items);
            assert_eq!(
                result.item(),
                if items.len() == 1 {
                    items.first()
                } else {
                    None
                }
            );
            assert_eq!(result.into_items(), items);
        }
    }
}
