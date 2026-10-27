//! Split canonical input before plan validation, retaining each complete ordinal.

use type_bridge_contract::limits::{MAX_INPUT_BYTES, MAX_INPUT_ROWS};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_contract::split::{SplitLimits, SplitRefusal, SplitUnit, split};

/// The fast guard avoids serializing ordinary batches; the slow path measures exact JSON bytes.
pub(crate) fn chunks<R>(
    rows: Vec<R>,
    reserved_bytes_per_row: usize,
    upper_bound: impl Fn(&R) -> usize,
    exact_bytes: impl Fn(&R) -> Result<usize, SdkExecutionDiagnostic>,
    same_ordinal: impl Fn(&R, &R) -> bool,
) -> Result<Vec<Vec<R>>, SdkExecutionDiagnostic> {
    let upper = rows.iter().fold(2_usize, |total, row| {
        total
            .saturating_add(upper_bound(row))
            .saturating_add(reserved_bytes_per_row)
    });
    if rows.len() <= MAX_INPUT_ROWS && upper <= MAX_INPUT_BYTES {
        return Ok(vec![rows]);
    }
    let mut units: Vec<SplitUnit> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let bytes = exact_bytes(row)?
            .saturating_add(1)
            .saturating_add(reserved_bytes_per_row);
        if index > 0 && same_ordinal(&rows[index - 1], row) {
            let unit = units.last_mut().expect("existing ordinal unit");
            unit.rows += 1;
            unit.bytes = unit.bytes.saturating_add(bytes);
        } else {
            units.push(SplitUnit { rows: 1, bytes });
        }
    }
    let ranges =
        split(2, &units, SplitLimits::new(MAX_INPUT_ROWS, MAX_INPUT_BYTES)).map_err(|error| {
            use crate::projected_batch_executor::resource_limit;
            match error {
                SplitRefusal::UnitTooLarge { unit }
                    if units[unit].bytes.saturating_add(2) > MAX_INPUT_BYTES =>
                {
                    resource_limit(
                        "projected_batch_input_unit_byte_limit",
                        "One complete input ordinal exceeds the canonical input ceiling",
                        "bytes",
                        units[unit].bytes.saturating_add(2) as u64,
                        MAX_INPUT_BYTES as u64,
                    )
                }
                _ => resource_limit(
                    "batch_statement_limit",
                    "One complete input ordinal exceeds the statement ceiling",
                    "rows",
                    rows.len() as u64,
                    MAX_INPUT_ROWS as u64,
                ),
            }
        })?;
    if ranges.len() <= 1 {
        return Ok(vec![rows]);
    }
    let mut rows = rows.into_iter();
    Ok(ranges
        .into_iter()
        .map(|range| {
            rows.by_ref()
                .take(units[range].iter().map(|unit| unit.rows).sum())
                .collect()
        })
        .collect())
}

pub(crate) fn canonical_upper_bound(row: &type_bridge_contract::query_plan::InputRow) -> usize {
    row.values().iter().fold(2_usize, |total, value| {
        let text = match value {
            Some(type_bridge_contract::value::CanonicalValue::String(value)) => {
                value.as_str().len()
            }
            _ => 0,
        };
        total
            .saturating_add(512)
            .saturating_add(text.saturating_mul(6))
    })
}
