//! The core batch split rule.
//!
//! A batch is one plan over all of its rows as `given` and fires once. It is
//! split only when its rows would pass a limit, into same-shape plans that
//! run in the same transaction, so the batch stays all-or-nothing. Every
//! language binding and the proxy call this one pure function, so they split
//! identically.

use std::ops::Range;

use crate::limits::{MAX_INPUT_ROWS, MAX_REMOTE_ENVELOPE_BYTES};

/// The ceilings one split chunk must respect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SplitLimits {
    /// Maximum `given` rows in one chunk.
    pub max_rows: usize,
    /// Maximum encoded bytes of one chunk: its plan plus its `given` rows.
    pub max_bytes: usize,
}

impl SplitLimits {
    /// The canonical limits: 4,096 rows and the 32 MiB envelope.
    pub const CANONICAL: Self = Self {
        max_rows: MAX_INPUT_ROWS,
        max_bytes: MAX_REMOTE_ENVELOPE_BYTES,
    };

    /// Limits with explicit ceilings, for callers that tighten the canonical ones.
    #[must_use]
    pub const fn new(max_rows: usize, max_bytes: usize) -> Self {
        Self {
            max_rows,
            max_bytes,
        }
    }
}

impl Default for SplitLimits {
    fn default() -> Self {
        Self::CANONICAL
    }
}

/// One indivisible unit of a batch: the `given` rows one input row produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitUnit {
    /// `given` rows this unit contributes.
    pub rows: usize,
    /// Encoded bytes of those rows.
    pub bytes: usize,
}

/// Why a batch cannot be split under its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SplitRefusal {
    /// The plan alone leaves no byte budget for rows.
    PlanTooLarge {
        /// Encoded plan bytes.
        plan_bytes: usize,
        /// The byte ceiling.
        max_bytes: usize,
    },
    /// One unit cannot fit in any chunk on its own.
    UnitTooLarge {
        /// Index of the first unit that does not fit.
        unit: usize,
    },
}

/// Splits `units` into contiguous, order-preserving ranges of unit indexes.
///
/// Each range holds at most `limits.max_rows` rows and at most
/// `limits.max_bytes - plan_bytes` row bytes. Ranges are filled greedily, so
/// a batch within the limits is exactly one range, and the result is fully
/// determined by its inputs. An empty batch yields no ranges.
///
/// # Errors
///
/// Refuses when the plan leaves no byte budget, or when a single unit exceeds
/// the row or byte budget on its own.
pub fn split(
    plan_bytes: usize,
    units: &[SplitUnit],
    limits: SplitLimits,
) -> Result<Vec<Range<usize>>, SplitRefusal> {
    let byte_budget = match limits.max_bytes.checked_sub(plan_bytes) {
        Some(budget) if budget > 0 => budget,
        _ => {
            return Err(SplitRefusal::PlanTooLarge {
                plan_bytes,
                max_bytes: limits.max_bytes,
            });
        }
    };
    let mut ranges = Vec::new();
    let (mut start, mut rows, mut bytes) = (0usize, 0usize, 0usize);
    for (index, unit) in units.iter().enumerate() {
        if unit.rows > limits.max_rows || unit.bytes > byte_budget {
            return Err(SplitRefusal::UnitTooLarge { unit: index });
        }
        let fits = rows + unit.rows <= limits.max_rows && bytes + unit.bytes <= byte_budget;
        if !fits {
            ranges.push(start..index);
            (start, rows, bytes) = (index, 0, 0);
        }
        rows += unit.rows;
        bytes += unit.bytes;
    }
    if start < units.len() {
        ranges.push(start..units.len());
    }
    Ok(ranges)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)] // Ranges are the split result, not a range-to-vec mistake.
mod tests {
    use super::*;

    fn units(count: usize, bytes: usize) -> Vec<SplitUnit> {
        vec![SplitUnit { rows: 1, bytes }; count]
    }

    #[test]
    fn empty_batch_has_no_ranges() {
        assert_eq!(split(10, &[], SplitLimits::CANONICAL), Ok(vec![]));
    }

    #[test]
    fn batch_within_limits_is_one_range() {
        assert_eq!(
            split(100, &units(1_000, 64), SplitLimits::CANONICAL),
            Ok(vec![0..1_000])
        );
    }

    #[test]
    fn exactly_at_the_row_limit_is_one_range_and_one_over_is_two() {
        let at = units(MAX_INPUT_ROWS, 1);
        assert_eq!(split(0, &at, SplitLimits::CANONICAL), Ok(vec![0..4_096]));
        let over = units(MAX_INPUT_ROWS + 1, 1);
        assert_eq!(
            split(0, &over, SplitLimits::CANONICAL),
            Ok(vec![0..4_096, 4_096..4_097])
        );
    }

    #[test]
    fn ten_thousand_rows_are_three_ranges() {
        assert_eq!(
            split(0, &units(10_000, 8), SplitLimits::CANONICAL),
            Ok(vec![0..4_096, 4_096..8_192, 8_192..10_000])
        );
    }

    #[test]
    fn byte_budget_excludes_the_plan_and_splits_exactly_at_it() {
        let limits = SplitLimits::new(100, 110);
        // Budget 100: ten 10-byte units fit exactly, the eleventh starts a range.
        assert_eq!(split(10, &units(10, 10), limits), Ok(vec![0..10]));
        assert_eq!(split(10, &units(11, 10), limits), Ok(vec![0..10, 10..11]));
    }

    #[test]
    fn multi_row_units_are_never_divided() {
        let limits = SplitLimits::new(4, usize::MAX);
        let batch = [
            SplitUnit { rows: 3, bytes: 1 },
            SplitUnit { rows: 2, bytes: 1 },
            SplitUnit { rows: 2, bytes: 1 },
        ];
        assert_eq!(split(0, &batch, limits), Ok(vec![0..1, 1..3]));
    }

    #[test]
    fn one_oversized_unit_is_refused() {
        let limits = SplitLimits::new(10, 50);
        let mut batch = units(3, 5);
        batch.push(SplitUnit { rows: 1, bytes: 41 });
        assert_eq!(
            split(10, &batch, limits),
            Err(SplitRefusal::UnitTooLarge { unit: 3 })
        );
        let wide = [SplitUnit { rows: 11, bytes: 1 }];
        assert_eq!(
            split(0, &wide, limits),
            Err(SplitRefusal::UnitTooLarge { unit: 0 })
        );
    }

    #[test]
    fn a_plan_filling_the_envelope_is_refused() {
        let limits = SplitLimits::new(10, 50);
        assert_eq!(
            split(50, &units(1, 1), limits),
            Err(SplitRefusal::PlanTooLarge {
                plan_bytes: 50,
                max_bytes: 50
            })
        );
    }
}
