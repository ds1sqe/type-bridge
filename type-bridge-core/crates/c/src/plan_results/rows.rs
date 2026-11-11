//! Generic typed tuple cells over the shared scalar answer or published model proof.
use super::*;
use crate::staged_answers::PendingSource;
use std::ffi::{c_char, c_void};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_orm::match_request::PlanFetchCell;

/// Owned rows result. Model and scalar views borrow this owner.
pub struct TypeBridgeRowsResult {
    state: Arc<RowsResultState>,
}
pub(crate) struct RowsResultState {
    source: Arc<CompiledSelection>,
    models: Option<Arc<ModelResultState>>,
    cells: Vec<Vec<Option<TypedScalar>>>,
    columns: usize,
    retained_index: OnceLock<Result<RetainedRangeIndex, TypeBridgeStatus>>,
}
struct TypedScalar {
    domain: ValueTypeTag,
    value: Scalar,
}
impl TypedScalar {
    fn new(value: &CanonicalValue) -> Result<Self, SdkExecutionDiagnostic> {
        Ok(Self {
            domain: value.value_type(),
            value: Scalar::new(value)?,
        })
    }
}
fn cell_kind() -> SdkExecutionDiagnostic {
    invalid(
        "plan_cell_kind_mismatch",
        "The selected cell does not have the requested model or scalar domain",
    )
}
impl RowsResultState {
    fn build(pending: &AuthoredPending) -> Result<Self, SdkExecutionDiagnostic> {
        let (models, cells, columns) = match &pending.answers {
            AuthoredAnswers::Models(_) => {
                let models = pending.model_result()?;
                let proof = &models.proof;
                let cells = (0..proof.row_count())
                    .map(|row| {
                        proof
                            .columns()
                            .iter()
                            .map(|binding| match proof.cell(row, *binding) {
                                Some(PlanFetchCell::Scalar(value)) => {
                                    TypedScalar::new(value).map(Some)
                                }
                                Some(PlanFetchCell::Model { .. }) => Ok(None),
                                None => Err(SdkExecutionDiagnostic::internal_failure()),
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let columns = proof.columns().len();
                (Some(models), cells, columns)
            }
            AuthoredAnswers::Raw(_) => {
                let answer = pending
                    .answer()
                    .map_err(|error| crate::staged::pending_diagnostic(&error))?;
                let plan = pending.selection_plan();
                let type_bridge_contract::query_plan::QueryOutput::Rows { columns } = plan.output()
                else {
                    return Err(SdkExecutionDiagnostic::internal_failure());
                };
                let cells = answer
                    .scalar_rows(plan)
                    .map_err(|error| {
                        crate::staged::contract_error(
                            &error,
                            "The scalar rows do not match the selected columns",
                        )
                    })?
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|value| TypedScalar::new(value).map(Some))
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (None, cells, columns.len())
            }
        };
        Ok(Self {
            source: Arc::clone(&pending.source),
            models,
            cells,
            columns,
            retained_index: OnceLock::new(),
        })
    }
    pub(crate) fn fence(&self, outputs: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
        outputs.check_package_borrowed_ranges(&self.source.package)?;
        if let Some(models) = &self.models {
            models.fence(outputs)?;
        }
        self.retained_index
            .get_or_init(|| {
                RetainedRangeIndex::collect(|collector| {
                    for cell in self.cells.iter().flatten().flatten() {
                        if let Scalar::Text(text) = &cell.value {
                            collector.check_bytes(
                                text.as_ptr().cast(),
                                text.as_bytes_with_nul().len(),
                            )?;
                        }
                    }
                    Ok(())
                })
            })
            .as_ref()
            .map_err(|status| *status)?
            .check(outputs)
    }
    fn cell(
        &self,
        row: usize,
        column: usize,
    ) -> Result<&Option<TypedScalar>, SdkExecutionDiagnostic> {
        let row = self.cells.get(row).ok_or_else(|| {
            invalid(
                "plan_row_out_of_range",
                "The requested row is outside this result",
            )
        })?;
        if column >= self.columns {
            return Err(invalid(
                "plan_column_out_of_range",
                "The requested column is outside this result",
            ));
        }
        row.get(column)
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
    }
    fn scalar(
        &self,
        row: usize,
        column: usize,
        domain: ValueTypeTag,
    ) -> Result<&Scalar, SdkExecutionDiagnostic> {
        let value = self
            .cell(row, column)?
            .as_ref()
            .filter(|cell| cell.domain == domain)
            .ok_or_else(cell_kind)?;
        Ok(&value.value)
    }
}

/// Publish a typed rows owner after successful commit. Repeated snapshots share its cache.
/// # Safety
/// Pending is live. Both writable, possibly unaligned owner slots are disjoint from inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_rows_pending_result(
    pending: *const TypeBridgePending,
    output: *mut *mut TypeBridgeRowsResult,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let mut tag = None;
    let inputs = |outputs: &DirectOutputPreflight| {
        // SAFETY: a nonnull input borrows a live cursor.
        tag = unsafe {
            crate::staged_answers::check_cursor_ranges(
                outputs,
                pending.cast(),
                size_of::<TypeBridgePending>(),
            )
        }?;
        Ok(())
    };
    // SAFETY: every input and retained range is checked before clearing either owner slot.
    if let Err(status) = unsafe { prepare_handle_outputs(output, diagnostics, inputs) } {
        return status;
    }
    if tag != Some(crate::staged_answers::PENDING_TAG) {
        return TypeBridgeStatus::InvalidArgument;
    }
    guarded(|| {
        // SAFETY: the actual complete pending cursor was fenced above.
        let PendingSource::Authored(pending) = &unsafe { &*pending }.state.pending else {
            return TypeBridgeStatus::InvalidArgument;
        };
        if !pending.source.rows {
            return return_execution_error(
                invalid(
                    "plan_result_not_rows",
                    "This terminal does not select tuple rows",
                ),
                diagnostics,
            );
        }
        if let Err(error) = pending.answer() {
            let status = crate::staged::answer_status(&error);
            let _ = return_execution_error(crate::staged::pending_diagnostic(&error), diagnostics);
            return status;
        }
        let state = match pending
            .rows_result
            .get_or_init(|| RowsResultState::build(pending).map(Arc::new))
            .clone()
        {
            Ok(state) => state,
            Err(error) => return return_execution_error(error, diagnostics),
        };
        let handle = match crate::allocation::try_box(
            crate::allocation::AllocationSite::PlanRowsResultHandle,
            TypeBridgeRowsResult { state },
        ) {
            Ok(handle) => handle,
            Err(_) => {
                return return_execution_error(
                    crate::allocation::allocation_exhausted(),
                    diagnostics,
                );
            }
        };
        // SAFETY: the writable owner slot was fenced and initialized above.
        unsafe { output.write_unaligned(Box::into_raw(handle)) };
        TypeBridgeStatus::Ok
    })
}
/// Free a rows owner, invalidating every borrowed string and model view from it.
/// # Safety
/// Slot is writable and owns null or a rows result returned by this library.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_rows_result_free(result: *mut *mut TypeBridgeRowsResult) {
    // SAFETY: forwarded owner-slot contract.
    unsafe { crate::staged::free_slot(result) }
}

unsafe fn read<T: Copy>(
    result: *const TypeBridgeRowsResult,
    token: *const TypeBridgeProjectedTokenV1,
    output: *mut T,
    empty: T,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
    value: impl FnOnce(&RowsResultState) -> Result<T, SdkExecutionDiagnostic>,
) -> TypeBridgeStatus {
    guarded(|| {
        let outputs = match direct_output_preflight(&[
            (output.cast(), size_of::<T>()),
            (
                diagnostics.cast(),
                size_of::<*mut TypeBridgeExecutionDiagnostics>(),
            ),
        ]) {
            Ok(outputs) => outputs,
            Err(status) => return status,
        };
        if !result.is_null() {
            if let Err(status) =
                outputs.check_bytes(result.cast(), size_of::<TypeBridgeRowsResult>())
            {
                return status;
            }
            // SAFETY: the live result's complete outer object was fenced above.
            if let Err(status) = unsafe { &*result }.state.fence(&outputs) {
                return status;
            }
        }
        if !token.is_null()
            && let Err(status) =
                outputs.check_bytes(token.cast(), size_of::<TypeBridgeProjectedTokenV1>())
        {
            return status;
        }
        // SAFETY: every nonnull writable output is disjoint from inputs and other outputs.
        unsafe {
            if !output.is_null() {
                output.write_unaligned(empty);
            }
            if !diagnostics.is_null() {
                diagnostics.write_unaligned(ptr::null_mut());
            }
        }
        if result.is_null() || output.is_null() || diagnostics.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: a complete live result is borrowed throughout the read.
        match value(&unsafe { &*result }.state) {
            Ok(value) => {
                unsafe { output.write_unaligned(value) };
                TypeBridgeStatus::Ok
            }
            Err(error) => return_execution_error(error, diagnostics),
        }
    })
}
macro_rules! count {
    ($function:ident, $value:expr) => {
        /// Read tuple dimensions without exposing unvalidated data.
        /// # Safety
        /// Result is live; output and diagnostics are writable and disjoint from inputs.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            result: *const TypeBridgeRowsResult,
            output: *mut usize,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded result/output contract.
            unsafe {
                read(result, ptr::null(), output, 0, diagnostics, |state| {
                    Ok(($value)(state))
                })
            }
        }
    };
}
count!(type_bridge_rows_result_count, |state: &RowsResultState| {
    state.cells.len()
});
count!(
    type_bridge_rows_result_column_count,
    |state: &RowsResultState| state.columns
);
macro_rules! numeric {
    ($function:ident, $ty:ty, $empty:expr, $domain:ident, $variant:ident) => {
        /// Read one scalar cell, refusing another kind/domain or an out-of-range position.
        /// # Safety
        /// Result is live; output and diagnostics are writable and disjoint from inputs.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            result: *const TypeBridgeRowsResult,
            row: usize,
            column: usize,
            output: *mut $ty,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded result/output contract.
            unsafe {
                read(result, ptr::null(), output, $empty, diagnostics, |state| {
                    let Scalar::$variant(value) =
                        state.scalar(row, column, ValueTypeTag::$domain)?
                    else {
                        return Err(SdkExecutionDiagnostic::internal_failure());
                    };
                    Ok(*value)
                })
            }
        }
    };
}
numeric!(type_bridge_rows_result_long, i64, 0, Long, Long);
numeric!(type_bridge_rows_result_double, f64, 0.0, Double, Double);
numeric!(type_bridge_rows_result_bool, bool, false, Boolean, Boolean);
macro_rules! text {
    ($function:ident, $domain:ident) => {
        /// Borrow canonical NUL-terminated scalar text, refusing another kind/domain.
        /// # Safety
        /// Result is live; output and diagnostics are writable and disjoint from inputs.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $function(
            result: *const TypeBridgeRowsResult,
            row: usize,
            column: usize,
            output: *mut *const c_char,
            diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
        ) -> TypeBridgeStatus {
            // SAFETY: forwarded result/output contract.
            unsafe {
                read(
                    result,
                    ptr::null(),
                    output,
                    ptr::null(),
                    diagnostics,
                    |state| {
                        let Scalar::Text(value) =
                            state.scalar(row, column, ValueTypeTag::$domain)?
                        else {
                            return Err(SdkExecutionDiagnostic::internal_failure());
                        };
                        Ok(value.as_ptr())
                    },
                )
            }
        }
    };
}
text!(type_bridge_rows_result_string, String);
text!(type_bridge_rows_result_date, Date);
text!(type_bridge_rows_result_datetime, DateTime);
text!(type_bridge_rows_result_datetime_tz, DateTimeTz);
text!(type_bridge_rows_result_decimal, Decimal);
text!(type_bridge_rows_result_duration, Duration);

/// Borrow a generated model view from a tuple cell under this result owner.
/// # Safety
/// Result/token are live; output and diagnostics are writable and disjoint from inputs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn type_bridge_rows_result_model_view(
    result: *const TypeBridgeRowsResult,
    row: usize,
    column: usize,
    token: *const TypeBridgeProjectedTokenV1,
    output: *mut *const c_void,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    // SAFETY: forwarded result/token/output contract.
    unsafe {
        read(result, token, output, ptr::null(), diagnostics, |state| {
            if state.cell(row, column)?.is_some() {
                return Err(cell_kind());
            }
            let models = state.models.as_ref().ok_or_else(cell_kind)?;
            let binding = *models
                .proof
                .columns()
                .get(column)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let Some(PlanFetchCell::Model { row, slot }) = models.proof.cell(row, binding) else {
                return Err(cell_kind());
            };
            let declared =
                crate::projected_token::resolve_model_token(&state.source.package, token)?;
            let views = models.views.as_ref().ok_or_else(|| {
                invalid(
                    "plan_context_missing",
                    "Borrowed model views require the generated package_plan_open constructor",
                )
            })?;
            if views.slot_model(slot) != Some(&declared) {
                return Err(invalid(
                    "plan_cell_model_mismatch",
                    "The selected model column has another declared model type",
                ));
            }
            views
                .row_slot(row, slot)
                .map(|view| view.as_ptr().cast())
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)
        })
    }
}

// Model row views exist only on the native C view ABI target; elsewhere
// type_bridge_plan_builder_new_with_layout refuses with c_plan_view_abi_unsupported.
#[cfg(all(
    test,
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64",
    target_endian = "little"
))]
#[path = "rows_tests.rs"]
mod tests;
