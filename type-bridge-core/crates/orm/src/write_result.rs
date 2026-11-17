//! Complete answers to single-input and batch manager writes.

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

/// Complete roots from a batch write, with their original input row indexes.
///
/// Items are in input order. Missing inputs produce no item; several roots from
/// one input repeat that input's index. The two slices always have equal length.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchWriteResult<T> {
    items: Vec<T>,
    input_indexes: Vec<usize>,
}

impl<T> BatchWriteResult<T> {
    /// Retain roots already ordered by their original input row index.
    #[must_use]
    pub fn from_indexed(values: impl IntoIterator<Item = (usize, T)>) -> Self {
        let values = values.into_iter();
        let capacity = values.size_hint().0;
        let mut items = Vec::with_capacity(capacity);
        let mut input_indexes = Vec::with_capacity(capacity);
        for (index, value) in values {
            items.push(value);
            input_indexes.push(index);
        }
        Self {
            items,
            input_indexes,
        }
    }

    pub(crate) fn try_with_capacity(
        capacity: usize,
    ) -> Result<Self, std::collections::TryReserveError> {
        let mut result = Self::default();
        result.items.try_reserve_exact(capacity)?;
        result.input_indexes.try_reserve_exact(capacity)?;
        Ok(result)
    }

    pub(crate) fn try_push(
        &mut self,
        index: usize,
        value: T,
    ) -> Result<(), std::collections::TryReserveError> {
        self.items.try_reserve(1)?;
        self.input_indexes.try_reserve(1)?;
        self.items.push(value);
        self.input_indexes.push(index);
        Ok(())
    }

    /// Retain all roots from one input without copying their storage.
    #[doc(hidden)]
    pub fn try_from_input(
        items: Vec<T>,
        index: usize,
    ) -> Result<Self, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic> {
        let mut input_indexes = Vec::new();
        input_indexes
            .try_reserve_exact(items.len())
            .map_err(|_| crate::ProjectedBatch::binding_allocation_failure())?;
        input_indexes.resize(items.len(), index);
        Ok(Self {
            items,
            input_indexes,
        })
    }

    /// Exact total number of roots written by all inputs.
    #[must_use]
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Complete hydrated roots, in input order.
    #[must_use]
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Input row index corresponding to each root in [`Self::items`].
    #[must_use]
    pub fn input_indexes(&self) -> &[usize] {
        &self.input_indexes
    }

    /// Consume the result without copying either slice.
    #[must_use]
    pub fn into_parts(self) -> (Vec<T>, Vec<usize>) {
        (self.items, self.input_indexes)
    }

    /// Consume the result and retain only its roots.
    #[must_use]
    pub fn into_items(self) -> Vec<T> {
        self.items
    }

    /// Map roots under their original input indexes without copying correlation.
    ///
    /// # Errors
    /// Returns a typed allocation refusal or the first binding mapping failure.
    #[doc(hidden)]
    pub fn try_map<U>(
        self,
        mut mapper: impl FnMut(
            usize,
            T,
        ) -> Result<
            U,
            type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic,
        >,
    ) -> Result<BatchWriteResult<U>, type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic>
    {
        let mut items = Vec::new();
        items
            .try_reserve_exact(self.items.len())
            .map_err(|_| crate::ProjectedBatch::binding_allocation_failure())?;
        for (item, index) in self
            .items
            .into_iter()
            .zip(self.input_indexes.iter().copied())
        {
            items.push(mapper(index, item)?);
        }
        Ok(BatchWriteResult {
            items,
            input_indexes: self.input_indexes,
        })
    }
}

impl<T> Default for BatchWriteResult<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            input_indexes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BatchWriteResult, WriteResult};

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
    #[test]
    fn batch_results_retain_missing_and_plural_input_correlations_through_mapping() {
        let result = BatchWriteResult::from_indexed([(1, 9), (1, 3), (4, 7)]);
        assert_eq!(result.count(), 3);
        assert_eq!(result.items(), &[9, 3, 7]);
        assert_eq!(result.input_indexes(), &[1, 1, 4]);
        let mapped = result
            .try_map(|index, value| Ok(format!("{index}:{value}")))
            .unwrap();
        assert_eq!(mapped.count(), mapped.items().len());
        assert_eq!(mapped.count(), mapped.input_indexes().len());
        assert_eq!(
            mapped.into_parts(),
            (
                vec!["1:9".to_owned(), "1:3".to_owned(), "4:7".to_owned()],
                vec![1, 1, 4]
            )
        );
        let result = BatchWriteResult::from_indexed([(1, 9), (1, 3)]);
        assert_eq!(
            result.try_map(|_, value| if value == 3 { Err(type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::data_operation_cancelled()) } else { Ok(value) }),
            Err(type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::data_operation_cancelled())
        );
        assert_eq!(BatchWriteResult::<u8>::default().count(), 0);
    }
}
