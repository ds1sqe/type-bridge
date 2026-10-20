//! Batch stages split by the core split rule.
//!
//! A batch stage is one statement over every row as `given`. It is divided
//! only when its rows pass the core limits, into same-shape statements the
//! executor fires in the same transaction, so the batch stays all-or-nothing.
//! Rows that share an ordinal (one input row's field or role rows) are one
//! indivisible unit.

use type_bridge_contract::split::{SplitLimits, SplitRefusal, SplitUnit, split};

use crate::projected_batch::limit_failure;
use crate::session::backend::{GivenRowsSpec, GivenValue};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;

/// Splits one stage's `given` rows into chunks under `limits`.
///
/// Rows must carry their input ordinal as the first column. A stage within
/// the limits comes back as exactly one chunk.
pub(crate) fn split_stage(
    source: &str,
    rows: GivenRowsSpec,
    limits: SplitLimits,
) -> Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    split_units(source, rows, limits, true)
}

/// Splits a read whose `given` rows are independent lookups, so every row is
/// its own unit (the prerequisite read: one row per put key or reference).
pub(crate) fn split_lookups(
    source: &str,
    rows: GivenRowsSpec,
    limits: SplitLimits,
) -> Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    split_units(source, rows, limits, false)
}

fn split_units(
    source: &str,
    rows: GivenRowsSpec,
    limits: SplitLimits,
    group_ordinals: bool,
) -> Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    let header_bytes: usize = rows.variables.iter().map(|name| name.len() + 4).sum();
    let mut units: Vec<SplitUnit> = Vec::new();
    let mut previous: Option<&GivenValue> = None;
    for row in &rows.rows {
        let bytes = row.iter().map(value_bytes).sum::<usize>() + 2;
        match (previous, row.first(), units.last_mut()) {
            (Some(last), Some(ordinal), Some(unit)) if group_ordinals && last == ordinal => {
                unit.rows += 1;
                unit.bytes += bytes;
            }
            _ => units.push(SplitUnit { rows: 1, bytes }),
        }
        previous = row.first();
    }
    let ranges = split(source.len() + header_bytes, &units, limits).map_err(refusal)?;
    if ranges.len() <= 1 {
        return Ok(vec![rows]);
    }
    let GivenRowsSpec { variables, rows } = rows;
    let mut rows = rows.into_iter();
    Ok(ranges
        .into_iter()
        .map(|range| GivenRowsSpec {
            variables: variables.clone(),
            rows: rows
                .by_ref()
                .take(units[range].iter().map(|unit| unit.rows).sum())
                .collect(),
        })
        .collect())
}

/// A batch whose single row cannot fit any statement keeps the released
/// statement-limit refusal.
fn refusal(refusal: SplitRefusal) -> SdkExecutionDiagnostic {
    let (actual, ceiling) = match refusal {
        SplitRefusal::PlanTooLarge {
            plan_bytes,
            max_bytes,
        } => (plan_bytes, max_bytes),
        SplitRefusal::UnitTooLarge { unit } => (unit.saturating_add(1), unit),
        _ => (1, 0),
    };
    limit_failure(
        "batch_statement_limit",
        "statements",
        u64::try_from(actual).unwrap_or(u64::MAX),
        u64::try_from(ceiling).unwrap_or(u64::MAX),
    )
}

/// The encoded size of one `given` cell in the provider's row encoding.
fn value_bytes(value: &GivenValue) -> usize {
    // A tagged cell: one tag byte plus a length or fixed-width payload.
    const TAG: usize = 5;
    TAG + match value {
        GivenValue::Empty => 0,
        GivenValue::Boolean(_) => 1,
        GivenValue::Integer(_) | GivenValue::Double(_) => 8,
        GivenValue::String(text)
        | GivenValue::Date(text)
        | GivenValue::Datetime(text)
        | GivenValue::DatetimeTz(text)
        | GivenValue::Decimal(text) => text.len(),
        GivenValue::DatetimeTzExact {
            local, named_zone, ..
        } => local.len() + named_zone.as_ref().map_or(0, String::len) + 4,
        GivenValue::Duration { .. } => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(ordinals: &[i64]) -> GivenRowsSpec {
        GivenRowsSpec {
            variables: vec!["ordinal".to_owned(), "name".to_owned()],
            rows: ordinals
                .iter()
                .map(|ordinal| {
                    vec![
                        GivenValue::Integer(*ordinal),
                        GivenValue::String("x".into()),
                    ]
                })
                .collect(),
        }
    }

    #[test]
    fn stage_within_limits_is_one_chunk() {
        let chunks =
            split_stage("insert", stage(&[0, 1, 2]), SplitLimits::CANONICAL).expect("splits");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].rows.len(), 3);
    }

    #[test]
    fn rows_of_one_ordinal_stay_in_one_chunk() {
        let chunks = split_stage(
            "insert",
            stage(&[0, 0, 1, 1, 2]),
            SplitLimits::new(3, usize::MAX),
        )
        .expect("splits");
        let sizes: Vec<_> = chunks.iter().map(|chunk| chunk.rows.len()).collect();
        assert_eq!(sizes, vec![2, 3]);
        assert_eq!(chunks[1].variables, vec!["ordinal", "name"]);
    }

    #[test]
    fn lookups_split_per_row_even_when_the_first_column_repeats() {
        let chunks = split_lookups(
            "match",
            stage(&[0, 0, 0, 0, 0]),
            SplitLimits::new(2, usize::MAX),
        )
        .expect("splits");
        let sizes: Vec<_> = chunks.iter().map(|chunk| chunk.rows.len()).collect();
        assert_eq!(sizes, vec![2, 2, 1]);
    }

    #[test]
    fn a_row_too_large_for_any_chunk_keeps_the_statement_refusal() {
        let error = split_stage("insert", stage(&[0, 0, 0]), SplitLimits::new(2, usize::MAX))
            .expect_err("refused");
        assert!(format!("{error:?}").contains("batch_statement_limit"));
    }
}
