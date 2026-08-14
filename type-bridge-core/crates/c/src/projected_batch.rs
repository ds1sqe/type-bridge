//! Private C adapters for homogeneous projected mutation batches.

use std::ffi::c_void;
use std::mem::size_of;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::id::{MAX_THING_IID_HEX_DIGITS, TypeId, is_canonical_thing_iid};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic,
};
use type_bridge_orm::projected_batch::ProjectedBatchConstructionControl;
use type_bridge_orm::{
    AnswerCancellation, ProjectedBatch, ProjectedBatchExecutor, ProjectedBatchInvocationControl,
    ProjectedBatchOperation, ProjectedBatchResult, ProjectedBatchRow, ProjectedThing,
    QueryExecutionResourceLimits,
};

use crate::abi::{
    SchemaPackageState, TypeBridgeByteView, TypeBridgeSchemaPackage, TypeBridgeStatus, close_box,
    guarded,
};
use crate::allocation::{
    AllocationSite, ReservedBox, allocation_checkpoint, allocation_exhausted, try_reserve,
};
use crate::execution_diagnostic::{
    TypeBridgeExecutionDiagnostics, initialize_execution_outputs, return_execution_error,
};
use crate::generated_preflight::{
    DirectOutputPreflight, GENERATED_INPUT_CANCELLATION, GENERATED_INPUT_DATABASE,
    GENERATED_INPUT_PROJECTED_CREATE, GENERATED_INPUT_PROJECTED_TOKEN,
    GENERATED_INPUT_SCHEMA_PACKAGE, GENERATED_INPUT_WRITE_TRANSACTION, direct_output_preflight,
};
use crate::policy::parse_common_limits;
use crate::projected_model::{
    TypeBridgeProjectedCreate, TypeBridgeProjectedThing, check_projected_create_ranges,
    check_projected_thing_ranges, check_type_id_ranges,
};
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token};
use crate::projected_value::{invalid_brand_diagnostic, same_package_brand};
use crate::query::TypeBridgeQueryExecutionLimitsV1;
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeWriteTransaction,
    check_database_borrowed_ranges, check_write_transaction_borrowed_ranges,
};

const MAX_THING_IID_BYTES: usize = 2 + MAX_THING_IID_HEX_DIGITS;

/// Stable C integer tag for one homogeneous projected-batch operation.
pub type TypeBridgeProjectedBatchOperation = i32;

/// Insert every complete create row.
pub const TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT: TypeBridgeProjectedBatchOperation = 1;
/// Put every complete keyed create row.
pub const TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT: TypeBridgeProjectedBatchOperation = 2;
/// Replace every exact IID with its complete projected row.
pub const TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE: TypeBridgeProjectedBatchOperation = 3;
/// Delete every exact IID idempotently.
pub const TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE: TypeBridgeProjectedBatchOperation = 4;

/// Opaque, package-branded, incrementally constructed projected batch.
pub struct TypeBridgeProjectedBatchBuilder {
    package: Arc<SchemaPackageState>,
    model: TypeId,
    operation: ProjectedBatchOperation,
    control: ProjectedBatchConstructionControl,
    rows: Vec<ProjectedBatchRow>,
}

/// Opaque immutable reusable projected batch.
pub struct TypeBridgeProjectedBatch {
    package: Arc<SchemaPackageState>,
    value: ProjectedBatch,
}

/// Opaque immutable all-or-nothing projected-batch result.
pub struct TypeBridgeProjectedBatchResult {
    package: Arc<SchemaPackageState>,
    model: TypeId,
    operation: ProjectedBatchOperation,
    input_count: usize,
    things: Vec<Arc<ProjectedThing>>,
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C projected-batch code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C projected-batch message is canonical")
}

fn invalid_input(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

fn integrity(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(code(code_value), message(message_value))
}

fn invalid_operation() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_operation_invalid",
        "The projected batch operation tag is outside the closed operation set",
    )
}

fn invalid_row_form() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_row_form_invalid",
        "The IID and projected create inputs do not match the batch operation",
    )
}

fn invalid_iid() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_iid_invalid",
        "The projected batch IID is not canonical TypeDB identity text",
    )
}

fn create_model_mismatch() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_create_model_mismatch",
        "The projected create value does not match the batch model",
    )
}

fn batch_package_mismatch() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_package_mismatch",
        "The projected batch and execution target retain different packages",
    )
}

fn result_model_mismatch() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_result_model_mismatch",
        "The expected generated model does not match the projected batch result",
    )
}

fn result_operation_mismatch() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_result_operation_mismatch",
        "The expected operation does not match the projected batch result",
    )
}

fn delete_result_has_no_thing() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_delete_result_has_no_thing",
        "Delete batch results do not expose projected things",
    )
}

fn result_index_out_of_bounds() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_result_index_out_of_bounds",
        "The projected batch result index is outside the input sequence",
    )
}

fn inactive_transaction() -> SdkExecutionDiagnostic {
    invalid_input(
        "c_projected_batch_transaction_inactive",
        "Projected batch execution requires an active write transaction",
    )
}

fn result_shape_invalid() -> SdkExecutionDiagnostic {
    integrity(
        "c_projected_batch_result_shape_invalid",
        "The projected batch executor returned an incompatible result shape",
    )
}

fn result_thing_model_mismatch() -> SdkExecutionDiagnostic {
    integrity(
        "c_projected_batch_result_thing_model_mismatch",
        "The projected batch executor returned a thing for a different model",
    )
}

fn operation_from_c(
    operation: TypeBridgeProjectedBatchOperation,
) -> Result<ProjectedBatchOperation, SdkExecutionDiagnostic> {
    match operation {
        TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT => Ok(ProjectedBatchOperation::Insert),
        TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT => Ok(ProjectedBatchOperation::Put),
        TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE => Ok(ProjectedBatchOperation::Update),
        TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE => Ok(ProjectedBatchOperation::Delete),
        _ => Err(invalid_operation()),
    }
}

#[cfg(test)]
fn operation_to_c(operation: ProjectedBatchOperation) -> TypeBridgeProjectedBatchOperation {
    match operation {
        ProjectedBatchOperation::Insert => TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
        ProjectedBatchOperation::Put => TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT,
        ProjectedBatchOperation::Update => TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE,
        ProjectedBatchOperation::Delete => TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE,
    }
}

fn checked_slice_bytes<T>(length: usize) -> Result<usize, TypeBridgeStatus> {
    length
        .checked_mul(size_of::<T>())
        .ok_or(TypeBridgeStatus::ResourceLimit)
}

fn check_slice_range<T>(
    values: &[T],
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(
        values.as_ptr().cast(),
        checked_slice_bytes::<T>(values.len())?,
    )
}

fn check_iid_range(iid: &str, preflight: &DirectOutputPreflight) -> Result<(), TypeBridgeStatus> {
    preflight.check_bytes(iid.as_ptr().cast(), iid.len())
}

fn check_row_ranges(
    row: &ProjectedBatchRow,
    preflight: &DirectOutputPreflight,
) -> Result<(), TypeBridgeStatus> {
    match row {
        ProjectedBatchRow::Create(create) => check_projected_create_ranges(create, preflight),
        ProjectedBatchRow::Update { iid, replacement } => {
            check_iid_range(iid, preflight)?;
            check_projected_create_ranges(replacement, preflight)
        }
        ProjectedBatchRow::Delete { iid } => check_iid_range(iid, preflight),
    }
}

impl TypeBridgeProjectedBatchBuilder {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_type_id_ranges(&self.model, preflight)?;
        // The output slot may be forged into any retained nested allocation.
        // No stable address index survives Vec growth, so every add performs
        // this allocation-free deep walk. Common row/member ceilings bound it.
        check_slice_range(&self.rows, preflight)?;
        for row in &self.rows {
            check_row_ranges(row, preflight)?;
        }
        Ok(())
    }
}

impl TypeBridgeProjectedBatch {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_type_id_ranges(self.value.model(), preflight)?;
        for index in 0..self.value.len() {
            let Some((_, row)) = self.value.row_at(index) else {
                return Err(TypeBridgeStatus::Panic);
            };
            preflight.check_bytes(
                (row as *const ProjectedBatchRow).cast(),
                size_of::<ProjectedBatchRow>(),
            )?;
            check_row_ranges(row, preflight)?;
        }
        Ok(())
    }
}

impl TypeBridgeProjectedBatchResult {
    fn check_borrowed_ranges(
        &self,
        preflight: &DirectOutputPreflight,
    ) -> Result<(), TypeBridgeStatus> {
        preflight.check_package_borrowed_ranges(&self.package)?;
        check_type_id_ranges(&self.model, preflight)?;
        check_slice_range(&self.things, preflight)?;
        for thing in &self.things {
            check_projected_thing_ranges(thing, preflight)?;
        }
        Ok(())
    }
}

fn preflight_outputs(
    outputs: &[(*mut c_void, usize)],
) -> Result<DirectOutputPreflight, TypeBridgeStatus> {
    direct_output_preflight(outputs)
}

fn check_object<T>(
    preflight: &DirectOutputPreflight,
    pointer: *const T,
) -> Result<(), TypeBridgeStatus> {
    if pointer.is_null() {
        Ok(())
    } else {
        preflight.check_bytes(pointer.cast(), size_of::<T>())
    }
}

unsafe fn check_package_input(
    preflight: &DirectOutputPreflight,
    package: *const TypeBridgeSchemaPackage,
) -> Result<(), TypeBridgeStatus> {
    preflight.check_object_kind(GENERATED_INPUT_SCHEMA_PACKAGE, package.cast())?;
    if !package.is_null() {
        // SAFETY: the complete opaque package object was fenced above.
        preflight.check_package_borrowed_ranges(unsafe { &*package }.state())?;
    }
    Ok(())
}

unsafe fn check_builder_input(
    preflight: &DirectOutputPreflight,
    builder: *const TypeBridgeProjectedBatchBuilder,
) -> Result<(), TypeBridgeStatus> {
    check_object(preflight, builder)?;
    if !builder.is_null() {
        // SAFETY: the complete opaque builder object was fenced above.
        unsafe { &*builder }.check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

unsafe fn check_batch_input(
    preflight: &DirectOutputPreflight,
    batch: *const TypeBridgeProjectedBatch,
) -> Result<(), TypeBridgeStatus> {
    check_object(preflight, batch)?;
    if !batch.is_null() {
        // SAFETY: the complete opaque batch object was fenced above.
        unsafe { &*batch }.check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

unsafe fn check_result_input(
    preflight: &DirectOutputPreflight,
    result: *const TypeBridgeProjectedBatchResult,
) -> Result<(), TypeBridgeStatus> {
    check_object(preflight, result)?;
    if !result.is_null() {
        // SAFETY: the complete opaque result object was fenced above.
        unsafe { &*result }.check_borrowed_ranges(preflight)?;
    }
    Ok(())
}

fn initialize_diagnostics_output(
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: preflight proved this caller slot writable and input-disjoint.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    Ok(())
}

fn initialize_count_outputs(
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if !out_count.is_null() {
        // SAFETY: preflight proved this caller slot writable and input-disjoint.
        unsafe { out_count.write_unaligned(0) };
    }
    initialize_diagnostics_output(out_diagnostics)?;
    if out_count.is_null() {
        Err(TypeBridgeStatus::InvalidArgument)
    } else {
        Ok(())
    }
}

fn cancellation_owner(cancellation: *const TypeBridgeCancellation) -> AnswerCancellation {
    if cancellation.is_null() {
        AnswerCancellation::default()
    } else {
        // SAFETY: every caller fences and retains the complete cancellation handle.
        unsafe { &*cancellation }.answer_cancellation()
    }
}

unsafe fn copied_limits(
    limits: *const TypeBridgeQueryExecutionLimitsV1,
) -> Option<TypeBridgeQueryExecutionLimitsV1> {
    if limits.is_null() {
        None
    } else {
        // SAFETY: caller preflight fenced one complete possibly-unaligned descriptor.
        Some(unsafe { limits.read_unaligned() })
    }
}

unsafe fn copy_iid(view: TypeBridgeByteView) -> Result<String, SdkExecutionDiagnostic> {
    if view.length == 0 || view.length > MAX_THING_IID_BYTES || view.data.is_null() {
        return Err(invalid_iid());
    }
    let mut bytes = Vec::new();
    try_reserve(
        &mut bytes,
        view.length,
        AllocationSite::ProjectedBatchBuilderIidBytes,
    )
    .map_err(|_| allocation_exhausted())?;
    // SAFETY: range preflight proved these already-bounded caller bytes readable.
    bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(view.data, view.length) });
    let iid = String::from_utf8(bytes).map_err(|_| invalid_iid())?;
    if !is_canonical_thing_iid(&iid) {
        return Err(invalid_iid());
    }
    Ok(iid)
}

fn canonical_empty_view(view: TypeBridgeByteView) -> bool {
    view.length == 0 && view.data.is_null()
}

fn validate_create(
    builder: &TypeBridgeProjectedBatchBuilder,
    create: &TypeBridgeProjectedCreate,
) -> Result<(), SdkExecutionDiagnostic> {
    if !same_package_brand(&builder.package, &create.package) {
        return Err(invalid_brand_diagnostic());
    }
    create
        .value
        .validate_for(&builder.package.installed_projection)?;
    if create.value.type_id() != &builder.model {
        return Err(create_model_mismatch());
    }
    Ok(())
}

fn row_from_c(
    builder: &TypeBridgeProjectedBatchBuilder,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
) -> Result<ProjectedBatchRow, SdkExecutionDiagnostic> {
    match builder.operation {
        ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put => {
            if !canonical_empty_view(iid) || create.is_null() {
                return Err(invalid_row_form());
            }
            // SAFETY: add preflight fenced and the caller retains this immutable handle.
            let create = unsafe { &*create };
            validate_create(builder, create)?;
            allocation_checkpoint(AllocationSite::ProjectedBatchBuilderCreateClone)
                .map_err(|_| allocation_exhausted())?;
            // `ProjectedBatchRow` is a common owned representation, while the
            // C create handle remains caller-owned. Its public common type has
            // no fallible or shared clone seam, so the deterministic probe
            // covers recoverable admission failure and the only remaining OOM
            // boundary is the process-terminal global allocator. This work is
            // provider-free and cannot expose a partial database mutation.
            Ok(ProjectedBatchRow::Create(create.value.clone()))
        }
        ProjectedBatchOperation::Update => {
            if create.is_null() {
                return Err(invalid_row_form());
            }
            // SAFETY: add preflight fenced and the caller retains this immutable handle.
            let create = unsafe { &*create };
            validate_create(builder, create)?;
            // SAFETY: byte-range preflight and canonical bounds precede the copy.
            let iid = unsafe { copy_iid(iid) }?;
            allocation_checkpoint(AllocationSite::ProjectedBatchBuilderCreateClone)
                .map_err(|_| allocation_exhausted())?;
            // See the insert/put branch above for the common owned-clone OOM
            // boundary. The IID copy and deterministic probe both precede it.
            Ok(ProjectedBatchRow::Update {
                iid,
                replacement: create.value.clone(),
            })
        }
        ProjectedBatchOperation::Delete => {
            if !create.is_null() {
                return Err(invalid_row_form());
            }
            // SAFETY: byte-range preflight and canonical bounds precede the copy.
            let iid = unsafe { copy_iid(iid) }?;
            Ok(ProjectedBatchRow::Delete { iid })
        }
    }
}

fn finish_allocation_preflight(
    builder: &TypeBridgeProjectedBatchBuilder,
) -> Result<(), SdkExecutionDiagnostic> {
    if builder.rows.is_empty() {
        return Ok(());
    }
    allocation_checkpoint(AllocationSite::ProjectedBatchFinishRows)
        .map_err(|_| allocation_exhausted())?;
    if matches!(
        builder.operation,
        ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete
    ) {
        allocation_checkpoint(AllocationSite::ProjectedBatchFinishTargets)
            .map_err(|_| allocation_exhausted())?;
    }
    let has_keys = builder
        .package
        .installed_projection
        .projection()
        .models()
        .get(&builder.model)
        .is_some_and(|model| !model.reference_read().key_fields().is_empty());
    if has_keys && builder.operation != ProjectedBatchOperation::Delete {
        allocation_checkpoint(AllocationSite::ProjectedBatchFinishKeys)
            .map_err(|_| allocation_exhausted())?;
    }
    Ok(())
}

fn validate_batch_target(
    package: &Arc<SchemaPackageState>,
    batch: &TypeBridgeProjectedBatch,
) -> Result<(), SdkExecutionDiagnostic> {
    if !same_package_brand(package, &batch.package) {
        return Err(batch_package_mismatch());
    }
    Ok(())
}

fn reserve_result_things(
    count: usize,
) -> Result<Vec<Arc<MaybeUninit<ProjectedThing>>>, SdkExecutionDiagnostic> {
    let mut things = Vec::new();
    try_reserve(
        &mut things,
        count,
        AllocationSite::ProjectedBatchResultThings,
    )
    .map_err(|_| allocation_exhausted())?;
    for _ in 0..count {
        allocation_checkpoint(AllocationSite::ProjectedBatchResultThingStorage)
            .map_err(|_| allocation_exhausted())?;
        // All caller-scaled Arc storage is allocated before the first mutation.
        // A process-terminal global allocator abort can therefore expose no
        // partial database effect; deterministic allocation probes fail here.
        things.push(Arc::new_uninit());
    }
    Ok(things)
}

unsafe fn assume_initialized_things(
    things: Vec<Arc<MaybeUninit<ProjectedThing>>>,
) -> Vec<Arc<ProjectedThing>> {
    let mut things = ManuallyDrop::new(things);
    let pointer = things.as_mut_ptr().cast::<Arc<ProjectedThing>>();
    let length = things.len();
    let capacity = things.capacity();
    // SAFETY: every pointee was initialized exactly once before this call;
    // Arc<T> and Arc<MaybeUninit<T>> have the same pointer representation.
    unsafe { Vec::from_raw_parts(pointer, length, capacity) }
}

fn map_result(
    package: Arc<SchemaPackageState>,
    model: TypeId,
    operation: ProjectedBatchOperation,
    input_count: usize,
    mut reserved_things: Vec<Arc<MaybeUninit<ProjectedThing>>>,
    result: ProjectedBatchResult,
) -> Result<TypeBridgeProjectedBatchResult, SdkExecutionDiagnostic> {
    let values = match (operation, result) {
        (ProjectedBatchOperation::Delete, ProjectedBatchResult::Deleted) => Vec::new(),
        (ProjectedBatchOperation::Delete, ProjectedBatchResult::Things(_))
        | (_, ProjectedBatchResult::Deleted) => return Err(result_shape_invalid()),
        (_, ProjectedBatchResult::Things(values)) if values.len() == input_count => values,
        (_, ProjectedBatchResult::Things(_)) => return Err(result_shape_invalid()),
        _ => return Err(result_shape_invalid()),
    };
    if reserved_things.len() != values.len() || values.iter().any(|thing| thing.type_id() != &model)
    {
        return Err(result_thing_model_mismatch());
    }
    for (slot, thing) in reserved_things.iter_mut().zip(values) {
        let Some(slot) = Arc::get_mut(slot) else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        slot.write(thing);
    }
    // SAFETY: the loop above initialized every uniquely owned reserved slot.
    let things = unsafe { assume_initialized_things(reserved_things) };
    if things.iter().any(|thing| thing.type_id() != &model) {
        // This check is allocation-free and defends the unsafe representation
        // transition above against future refactors.
        return Err(result_thing_model_mismatch());
    }
    Ok(TypeBridgeProjectedBatchResult {
        package,
        model,
        operation,
        input_count,
        things,
    })
}

fn empty_result(batch: &TypeBridgeProjectedBatch) -> TypeBridgeProjectedBatchResult {
    TypeBridgeProjectedBatchResult {
        package: Arc::clone(&batch.package),
        model: batch.value.model().clone(),
        operation: batch.value.operation(),
        input_count: 0,
        things: Vec::new(),
    }
}

fn validate_result_fence(
    result: &TypeBridgeProjectedBatchResult,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_operation: TypeBridgeProjectedBatchOperation,
) -> Result<(), SdkExecutionDiagnostic> {
    // SAFETY: result accessor preflight fenced one complete generated token.
    let expected_model = unsafe { resolve_model_token(&result.package, expected_model) }?;
    if expected_model != result.model {
        return Err(result_model_mismatch());
    }
    if operation_from_c(expected_operation)? != result.operation {
        return Err(result_operation_mismatch());
    }
    Ok(())
}

/// Open one exact-model projected batch builder without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_builder_open_v1_impl(
    package: *const TypeBridgeSchemaPackage,
    model: *const TypeBridgeProjectedTokenV1,
    operation: TypeBridgeProjectedBatchOperation,
    construction_limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_builder: *mut *mut TypeBridgeProjectedBatchBuilder,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[
        (
            out_builder.cast(),
            size_of::<*mut TypeBridgeProjectedBatchBuilder>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: all reads precede output initialization and are range-fenced here.
    if let Err(status) = unsafe { check_package_input(&preflight, package) }
        .and_then(|()| preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, model.cast()))
        .and_then(|()| check_object(&preflight, construction_limits))
        .and_then(|()| {
            preflight.check_object_kind(GENERATED_INPUT_CANCELLATION, cancellation.cast())
        })
    {
        return status;
    }
    // SAFETY: output preflight proved the two caller slots disjoint from every input.
    if let Err(status) = unsafe { initialize_execution_outputs(out_builder, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if package.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: package preflight fenced this live immutable handle.
        let package = unsafe { &*package };
        let operation = match operation_from_c(operation) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: descriptor preflight fenced a complete optional limits value.
        let limits = match parse_common_limits(unsafe { copied_limits(construction_limits) }) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: generated model storage remains immutable for this call.
        let model = match unsafe { resolve_model_token(package.state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let cancellation = cancellation_owner(cancellation);
        let control = match ProjectedBatchConstructionControl::try_new(
            &package.state.installed_projection,
            model.clone(),
            operation,
            limits,
            &cancellation,
        ) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if let Err(diagnostic) = control.checkpoint() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let builder = TypeBridgeProjectedBatchBuilder {
            package: Arc::clone(package.state()),
            model,
            operation,
            control,
            rows: Vec::new(),
        };
        let builder = match ReservedBox::try_new(AllocationSite::ProjectedBatchBuilderHandle) {
            Ok(value) => value.initialize(builder),
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        // SAFETY: initialized output remains caller-writable and unpublished so far.
        unsafe { out_builder.write_unaligned(Box::into_raw(builder)) };
        TypeBridgeStatus::Ok
    })
}

/// Add one copied row without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_builder_add_v1_impl(
    builder: *mut TypeBridgeProjectedBatchBuilder,
    iid: TypeBridgeByteView,
    create: *const TypeBridgeProjectedCreate,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[(
        out_diagnostics.cast(),
        size_of::<*mut TypeBridgeExecutionDiagnostics>(),
    )]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: all live handle graphs are fenced before diagnostics initialization.
    if let Err(status) = unsafe { check_builder_input(&preflight, builder) }
        .and_then(|()| preflight.check_bytes(iid.data.cast(), iid.length))
        .and_then(|()| preflight.check_object_kind(GENERATED_INPUT_PROJECTED_CREATE, create.cast()))
    {
        return status;
    }
    if !create.is_null() {
        // SAFETY: the complete create handle was fenced immediately above.
        if let Err(status) = unsafe { &*create }.check_borrowed_ranges(&preflight) {
            return status;
        }
    }
    if let Err(status) = initialize_diagnostics_output(out_diagnostics) {
        return status;
    }
    guarded(|| {
        if builder.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains unique mutable access to this builder for the call.
        let builder = unsafe { &mut *builder };
        if let Err(diagnostic) = builder.control.checkpoint() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let row = match row_from_c(builder, iid, create) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if try_reserve(
            &mut builder.rows,
            1,
            AllocationSite::ProjectedBatchBuilderRows,
        )
        .is_err()
        {
            return return_execution_error(allocation_exhausted(), out_diagnostics);
        }
        if let Err(diagnostic) = builder
            .control
            .try_add_row(&builder.package.installed_projection, &row)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        builder.rows.push(row);
        TypeBridgeStatus::Ok
    })
}

/// Recoverably finish one builder without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_builder_finish_impl(
    builder: *mut *mut TypeBridgeProjectedBatchBuilder,
    out_batch: *mut *mut TypeBridgeProjectedBatch,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[
        (out_batch.cast(), size_of::<*mut TypeBridgeProjectedBatch>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = check_object(&preflight, builder) {
        return status;
    }
    let builder_value = if builder.is_null() {
        ptr::null_mut()
    } else {
        // SAFETY: caller contract supplies a readable ownership slot for this call.
        unsafe { builder.read_unaligned() }
    };
    // SAFETY: the pointee and its complete borrowed graph are still immutable here.
    if let Err(status) = unsafe { check_builder_input(&preflight, builder_value) } {
        return status;
    }
    // SAFETY: preflight proved both outputs disjoint from the ownership slot and pointee graph.
    if let Err(status) = unsafe { initialize_execution_outputs(out_batch, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if builder.is_null() || builder_value.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller gives this finish call exclusive ownership-slot access.
        let builder_handle = unsafe { &mut *builder_value };
        if !builder_handle.rows.is_empty()
            && let Err(diagnostic) = builder_handle.control.checkpoint()
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let reservation = match ReservedBox::try_new(AllocationSite::ProjectedBatchHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        if let Err(diagnostic) = finish_allocation_preflight(builder_handle) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let value = match builder_handle.control.try_finalize(
            &builder_handle.package.installed_projection,
            &mut builder_handle.rows,
        ) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let batch = reservation.initialize(TypeBridgeProjectedBatch {
            package: Arc::clone(&builder_handle.package),
            value,
        });
        // SAFETY: every fallible operation completed. Clear and consume the builder once.
        unsafe { builder.write_unaligned(ptr::null_mut()) };
        // SAFETY: successful finish uniquely retakes the allocation returned by open.
        unsafe { drop(Box::from_raw(builder_value)) };
        // SAFETY: initialized output remains caller-writable and receives sole ownership.
        unsafe { out_batch.write_unaligned(Box::into_raw(batch)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one builder without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_builder_close_impl(
    builder: *mut *mut TypeBridgeProjectedBatchBuilder,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is frozen by the extension header.
    unsafe { close_box(builder) }
}

/// Close one immutable batch without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_close_impl(
    batch: *mut *mut TypeBridgeProjectedBatch,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is frozen by the extension header.
    unsafe { close_box(batch) }
}

enum BatchExecutionTarget<'handle> {
    Database(&'handle TypeBridgeDatabase),
    WriteTransaction(&'handle TypeBridgeWriteTransaction),
}

impl BatchExecutionTarget<'_> {
    fn package_state(&self) -> &Arc<SchemaPackageState> {
        match self {
            Self::Database(database) => database.package_state(),
            Self::WriteTransaction(transaction) => transaction.package_state(),
        }
    }

    fn answer_ceiling(&self) -> QueryExecutionResourceLimits {
        match self {
            Self::Database(database) => database.answer_ceiling(),
            Self::WriteTransaction(transaction) => transaction.answer_ceiling(),
        }
    }
}

unsafe fn execute_impl(
    target: BatchExecutionTarget<'_>,
    batch: *const TypeBridgeProjectedBatch,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeProjectedBatchResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    guarded(|| {
        if batch.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller preflight fenced the immutable batch and its borrowed graph.
        let batch = unsafe { &*batch };
        if let Err(diagnostic) = validate_batch_target(target.package_state(), batch) {
            return return_execution_error(diagnostic, out_diagnostics);
        }

        if batch.value.is_empty() {
            if let Err(diagnostic) = batch
                .value
                .validate_for(&target.package_state().installed_projection)
            {
                return return_execution_error(diagnostic, out_diagnostics);
            }
            let reservation = match ReservedBox::try_new(AllocationSite::ProjectedBatchResultHandle)
            {
                Ok(value) => value,
                Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
            };
            let result = reservation.initialize(empty_result(batch));
            // SAFETY: initialized result output remains caller-writable.
            unsafe { out_result.write_unaligned(Box::into_raw(result)) };
            return TypeBridgeStatus::Ok;
        }

        // SAFETY: execution preflight fenced a complete optional descriptor.
        let call_limits = match parse_common_limits(unsafe { copied_limits(limits) }) {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let effective_limits = call_limits
            .constrained_by(batch.value.resource_ceiling())
            .constrained_by(target.answer_ceiling());
        let control = ProjectedBatchInvocationControl::capture(
            effective_limits,
            cancellation_owner(cancellation),
        );
        if let Err(diagnostic) = control.checkpoint() {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if let BatchExecutionTarget::WriteTransaction(transaction) = &target
            && transaction.is_poisoned()
        {
            return return_execution_error(
                crate::runtime::poisoned_transaction_diagnostic(),
                out_diagnostics,
            );
        }
        let reservation = match ReservedBox::try_new(AllocationSite::ProjectedBatchResultHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };

        let package = Arc::clone(target.package_state());
        let model = batch.value.model().clone();
        let operation = batch.value.operation();
        let input_count = batch.value.len();
        let reserved_things =
            match reserve_result_things(if operation == ProjectedBatchOperation::Delete {
                0
            } else {
                input_count
            }) {
                Ok(value) => value,
                Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
            };
        let executor = ProjectedBatchExecutor::new(&target.package_state().installed_projection);
        let executed = match target {
            BatchExecutionTarget::Database(database) => database.block_on(executor.execute_mapped(
                database.orm_database(),
                &batch.value,
                control,
                None,
                move |value| {
                    map_result(
                        package,
                        model,
                        operation,
                        input_count,
                        reserved_things,
                        value,
                    )
                    .map(Some)
                },
            )),
            BatchExecutionTarget::WriteTransaction(transaction) => {
                let Some(context) = transaction.context() else {
                    return return_execution_error(inactive_transaction(), out_diagnostics);
                };
                transaction.block_on(executor.execute_in_transaction_mapped(
                    context,
                    &batch.value,
                    control,
                    None,
                    move |value| {
                        map_result(
                            package,
                            model,
                            operation,
                            input_count,
                            reserved_things,
                            value,
                        )
                        .map(Some)
                    },
                ))
            }
        };
        let result = match executed {
            Ok(Some(value)) => value,
            Ok(None) => return return_execution_error(result_shape_invalid(), out_diagnostics),
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        let result = reservation.initialize(result);
        // SAFETY: execution and commit completed and the output remains caller-writable.
        unsafe { out_result.write_unaligned(Box::into_raw(result)) };
        TypeBridgeStatus::Ok
    })
}

unsafe fn execute_input_preflight(
    preflight: &DirectOutputPreflight,
    batch: *const TypeBridgeProjectedBatch,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
) -> Result<(), TypeBridgeStatus> {
    // SAFETY: batch pointee and its borrowed graph are fenced read-only.
    unsafe { check_batch_input(preflight, batch) }?;
    check_object(preflight, limits)?;
    preflight.check_object_kind(GENERATED_INPUT_CANCELLATION, cancellation.cast())?;
    Ok(())
}

/// Execute one batch through an owned database transaction without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_database_projected_batch_execute_v1_impl(
    database: *const TypeBridgeDatabase,
    batch: *const TypeBridgeProjectedBatch,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeProjectedBatchResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[
        (
            out_result.cast(),
            size_of::<*mut TypeBridgeProjectedBatchResult>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) = preflight.check_object_kind(GENERATED_INPUT_DATABASE, database.cast()) {
        return status;
    }
    if !database.is_null() {
        // SAFETY: outer database storage was fenced above and remains immutable.
        if let Err(status) = check_database_borrowed_ranges(&preflight, unsafe { &*database }) {
            return status;
        }
    }
    // SAFETY: remaining input graphs are fenced before either output is written.
    if let Err(status) = unsafe { execute_input_preflight(&preflight, batch, limits, cancellation) }
    {
        return status;
    }
    // SAFETY: output preflight proved both slots pairwise and input disjoint.
    if let Err(status) = unsafe { initialize_execution_outputs(out_result, out_diagnostics) } {
        return status;
    }
    if database.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced the retained database and all delegated inputs.
    unsafe {
        execute_impl(
            BatchExecutionTarget::Database(&*database),
            batch,
            limits,
            cancellation,
            out_result,
            out_diagnostics,
        )
    }
}

/// Execute one batch through a borrowed write transaction without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_write_transaction_projected_batch_execute_v1_impl(
    transaction: *const TypeBridgeWriteTransaction,
    batch: *const TypeBridgeProjectedBatch,
    limits: *const TypeBridgeQueryExecutionLimitsV1,
    cancellation: *const TypeBridgeCancellation,
    out_result: *mut *mut TypeBridgeProjectedBatchResult,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[
        (
            out_result.cast(),
            size_of::<*mut TypeBridgeProjectedBatchResult>(),
        ),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    if let Err(status) =
        preflight.check_object_kind(GENERATED_INPUT_WRITE_TRANSACTION, transaction.cast())
    {
        return status;
    }
    if !transaction.is_null() {
        // SAFETY: outer transaction storage was fenced above and remains immutable.
        if let Err(status) =
            check_write_transaction_borrowed_ranges(&preflight, unsafe { &*transaction })
        {
            return status;
        }
    }
    // SAFETY: remaining input graphs are fenced before either output is written.
    if let Err(status) = unsafe { execute_input_preflight(&preflight, batch, limits, cancellation) }
    {
        return status;
    }
    // SAFETY: output preflight proved both slots pairwise and input disjoint.
    if let Err(status) = unsafe { initialize_execution_outputs(out_result, out_diagnostics) } {
        return status;
    }
    if transaction.is_null() {
        return TypeBridgeStatus::InvalidArgument;
    }
    // SAFETY: preflight fenced the retained transaction and all delegated inputs.
    unsafe {
        execute_impl(
            BatchExecutionTarget::WriteTransaction(&*transaction),
            batch,
            limits,
            cancellation,
            out_result,
            out_diagnostics,
        )
    }
}

/// Return the immutable input cardinality without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_result_count_impl(
    result: *const TypeBridgeProjectedBatchResult,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_operation: TypeBridgeProjectedBatchOperation,
    out_count: *mut usize,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[
        (out_count.cast(), size_of::<usize>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: full result graph and token storage are fenced before caller writes.
    if let Err(status) = unsafe { check_result_input(&preflight, result) }.and_then(|()| {
        preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, expected_model.cast())
    }) {
        return status;
    }
    if let Err(status) = initialize_count_outputs(out_count, out_diagnostics) {
        return status;
    }
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: accessor preflight fenced the immutable result graph.
        let result = unsafe { &*result };
        if let Err(diagnostic) = validate_result_fence(result, expected_model, expected_operation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        // SAFETY: initialized count output remains caller-writable.
        unsafe { out_count.write_unaligned(result.input_count) };
        TypeBridgeStatus::Ok
    })
}

/// Clone one projected result thing without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_result_thing_at_impl(
    result: *const TypeBridgeProjectedBatchResult,
    expected_model: *const TypeBridgeProjectedTokenV1,
    expected_operation: TypeBridgeProjectedBatchOperation,
    index: usize,
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    let preflight = match preflight_outputs(&[
        (out_thing.cast(), size_of::<*mut TypeBridgeProjectedThing>()),
        (
            out_diagnostics.cast(),
            size_of::<*mut TypeBridgeExecutionDiagnostics>(),
        ),
    ]) {
        Ok(value) => value,
        Err(status) => return status,
    };
    // SAFETY: full result graph and token storage are fenced before caller writes.
    if let Err(status) = unsafe { check_result_input(&preflight, result) }.and_then(|()| {
        preflight.check_object_kind(GENERATED_INPUT_PROJECTED_TOKEN, expected_model.cast())
    }) {
        return status;
    }
    // SAFETY: both caller slots are pairwise and input disjoint.
    if let Err(status) = unsafe { initialize_execution_outputs(out_thing, out_diagnostics) } {
        return status;
    }
    guarded(|| {
        if result.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: accessor preflight fenced the immutable result graph.
        let result = unsafe { &*result };
        if let Err(diagnostic) = validate_result_fence(result, expected_model, expected_operation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if result.operation == ProjectedBatchOperation::Delete {
            return return_execution_error(delete_result_has_no_thing(), out_diagnostics);
        }
        let Some(value) = result.things.get(index) else {
            return return_execution_error(result_index_out_of_bounds(), out_diagnostics);
        };
        let reservation =
            match ReservedBox::try_new(AllocationSite::ProjectedBatchResultThingHandle) {
                Ok(value) => value,
                Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
            };
        let thing = reservation.initialize(TypeBridgeProjectedThing::from_arc(
            Arc::clone(&result.package),
            Arc::clone(value),
        ));
        // SAFETY: initialized thing output remains caller-writable.
        unsafe { out_thing.write_unaligned(Box::into_raw(thing)) };
        TypeBridgeStatus::Ok
    })
}

/// Close one projected batch result without activating an ABI export.
pub(crate) unsafe extern "C" fn type_bridge_projected_batch_result_close_impl(
    result: *mut *mut TypeBridgeProjectedBatchResult,
) -> TypeBridgeStatus {
    // SAFETY: forwarded pointer-to-pointer ownership contract is frozen by the extension header.
    unsafe { close_box(result) }
}

// Keep the complete dormant ABI contract compiler-checked in production
// builds before the atomic extension-header/export activation.
const _: unsafe extern "C" fn(
    *const TypeBridgeSchemaPackage,
    *const TypeBridgeProjectedTokenV1,
    TypeBridgeProjectedBatchOperation,
    *const TypeBridgeQueryExecutionLimitsV1,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeProjectedBatchBuilder,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_projected_batch_builder_open_v1_impl;
const _: unsafe extern "C" fn(
    *mut TypeBridgeProjectedBatchBuilder,
    TypeBridgeByteView,
    *const TypeBridgeProjectedCreate,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_projected_batch_builder_add_v1_impl;
const _: unsafe extern "C" fn(
    *mut *mut TypeBridgeProjectedBatchBuilder,
    *mut *mut TypeBridgeProjectedBatch,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_projected_batch_builder_finish_impl;
const _: unsafe extern "C" fn(*mut *mut TypeBridgeProjectedBatchBuilder) -> TypeBridgeStatus =
    type_bridge_projected_batch_builder_close_impl;
const _: unsafe extern "C" fn(*mut *mut TypeBridgeProjectedBatch) -> TypeBridgeStatus =
    type_bridge_projected_batch_close_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeDatabase,
    *const TypeBridgeProjectedBatch,
    *const TypeBridgeQueryExecutionLimitsV1,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeProjectedBatchResult,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_database_projected_batch_execute_v1_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeWriteTransaction,
    *const TypeBridgeProjectedBatch,
    *const TypeBridgeQueryExecutionLimitsV1,
    *const TypeBridgeCancellation,
    *mut *mut TypeBridgeProjectedBatchResult,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_write_transaction_projected_batch_execute_v1_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeProjectedBatchResult,
    *const TypeBridgeProjectedTokenV1,
    TypeBridgeProjectedBatchOperation,
    *mut usize,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_projected_batch_result_count_impl;
const _: unsafe extern "C" fn(
    *const TypeBridgeProjectedBatchResult,
    *const TypeBridgeProjectedTokenV1,
    TypeBridgeProjectedBatchOperation,
    usize,
    *mut *mut TypeBridgeProjectedThing,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus = type_bridge_projected_batch_result_thing_at_impl;
const _: unsafe extern "C" fn(*mut *mut TypeBridgeProjectedBatchResult) -> TypeBridgeStatus =
    type_bridge_projected_batch_result_close_impl;

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::{Value, json};
    use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
    use type_bridge_contract::codec::to_canonical_json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, RoleId, TypeKind};
    use type_bridge_contract::managed_scope::ManagedScopeId;
    use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
    use type_bridge_contract::schema::{DocumentId, OwnsFactId, encode_declared_schema};
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    use type_bridge_core_lib::version::Version;
    use type_bridge_orm::session::backend::{
        BoxFuture, DriverBackend, GivenRowsSpec, QueryResult, TransactionOps,
    };
    use type_bridge_orm::{
        ClassifiedCommitError, CommitFailureCertainty, Database, OrmError, ProjectedAttributeValue,
        ProjectedCreate, ProjectedReference, TransactionContextState, TxType,
    };
    use type_bridge_schema::{
        BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SchemaDocumentSet,
        build_schema_authority, encode_schema_authority, normalize_documents, project, resolve,
    };
    use type_bridge_schema_codegen::CEmitter;

    use super::*;
    use crate::abi::{ABI_MAJOR, ABI_MINOR, TypeBridgeSchemaPackageDescriptorV1};
    use crate::allocation::{AllocationSite, inject_failure};
    use crate::entity_crud::tests::{
        byte_view, close_diagnostics, close_thing, diagnostic, model_token,
    };
    use crate::runtime::{
        type_bridge_cancellation_close, type_bridge_cancellation_open,
        type_bridge_cancellation_request, type_bridge_write_transaction_rollback,
    };

    const BATCH_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 4 } }
plays:
  person:
    membership: [member]
"#;

    struct PackageEvidence {
        authority: Vec<u8>,
        declared: Vec<u8>,
        projection: Vec<u8>,
        semantic: Vec<u8>,
        binding: Vec<u8>,
        scope: Vec<u8>,
        profile: Vec<u8>,
    }

    impl PackageEvidence {
        fn descriptor(&self) -> TypeBridgeSchemaPackageDescriptorV1 {
            TypeBridgeSchemaPackageDescriptorV1 {
                struct_size: size_of::<TypeBridgeSchemaPackageDescriptorV1>() as u32,
                abi_major: ABI_MAJOR,
                abi_minor: ABI_MINOR,
                schema_authority_json: byte_view(&self.authority),
                declared_schema_json: byte_view(&self.declared),
                runtime_projection_json: byte_view(&self.projection),
                semantic_fingerprint_json: byte_view(&self.semantic),
                binding_fingerprint_json: byte_view(&self.binding),
                managed_scope: byte_view(&self.scope),
                semantic_profile: byte_view(&self.profile),
                reserved: [0; 4],
            }
        }
    }

    fn make_package(prefix: &str) -> TypeBridgeSchemaPackage {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("projected-batch-c.yaml").unwrap(),
            BATCH_SCHEMA,
        )])
        .unwrap();
        let declared = normalize_documents(&documents).unwrap();
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let available: CapabilitySet = BUILTIN_SCHEMA_CAPABILITY_IDS
            .iter()
            .map(|value| CapabilityId::new(*value).unwrap())
            .collect();
        let scope = ManagedScopeId::new(format!("c-projected-batch-{prefix}")).unwrap();
        let context = ManagedDeltaContext::new(scope.clone(), profile.clone(), available);
        let authority =
            build_schema_authority(&declared, declared.required_capabilities(), &context).unwrap();
        let emitter = CEmitter::new();
        let projection = project(
            &resolved,
            BindingTarget::C,
            &ProjectionConfig::c(CSymbolPrefix::new(prefix).unwrap()),
            &emitter.generator_handlers(),
            &emitter.code_resources().unwrap(),
        )
        .unwrap();
        let evidence = PackageEvidence {
            authority: encode_schema_authority(&authority),
            declared: encode_declared_schema(&declared).unwrap(),
            projection: to_canonical_json(&projection).unwrap(),
            semantic: to_canonical_json(projection.semantic_fingerprint()).unwrap(),
            binding: to_canonical_json(projection.projection_fingerprint()).unwrap(),
            scope: scope.as_str().as_bytes().to_vec(),
            profile: profile.as_str().as_bytes().to_vec(),
        };
        crate::schema_package::open(evidence.descriptor()).unwrap()
    }

    fn type_id(kind: TypeKind) -> TypeId {
        TypeId::new(
            kind,
            if kind == TypeKind::Entity {
                "person"
            } else {
                "membership"
            },
        )
        .unwrap()
    }

    fn create(
        package: &TypeBridgeSchemaPackage,
        kind: TypeKind,
        identifier: &str,
        player_iid: &str,
    ) -> Box<TypeBridgeProjectedCreate> {
        let owner = type_id(kind);
        let attribute = TypeId::new(TypeKind::Attribute, "identifier").unwrap();
        let field =
            OwnsFactId::new(owner.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let value = ProjectedAttributeValue::try_new(
            &package.state.installed_projection,
            attribute,
            CanonicalValue::String(CanonicalString::new(identifier).unwrap()),
        )
        .unwrap();
        let roles = if kind == TypeKind::Relation {
            vec![(
                RoleId::new("membership", "member").unwrap(),
                vec![
                    ProjectedReference::try_new(
                        &package.state.installed_projection,
                        TypeId::new(TypeKind::Entity, "person").unwrap(),
                        Some(player_iid.to_owned()),
                        vec![],
                    )
                    .unwrap(),
                ],
            )]
        } else {
            vec![]
        };
        let value = ProjectedCreate::try_new(
            &package.state.installed_projection,
            owner,
            vec![(field, vec![value])],
            roles,
        )
        .unwrap();
        Box::new(TypeBridgeProjectedCreate {
            package: Arc::clone(&package.state),
            value,
        })
    }

    #[derive(Clone, Copy)]
    enum CommitBehavior {
        Success,
        Failure,
    }

    enum Response {
        Documents(Vec<Value>),
        Error,
    }

    #[derive(Default)]
    struct RecordingState {
        responses: VecDeque<Response>,
        opens: Vec<TxType>,
        calls: Vec<(String, GivenRowsSpec)>,
        commits: usize,
        rollbacks: usize,
        closes: usize,
    }

    struct RecordingBackend {
        state: Arc<Mutex<RecordingState>>,
        commit: CommitBehavior,
    }

    impl DriverBackend for RecordingBackend {
        fn open_transaction(
            &self,
            _database: &str,
            tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.lock().unwrap().opens.push(tx_type);
            let transaction = RecordingTransaction {
                state: Arc::clone(&self.state),
                commit: self.commit,
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<Version> {
            Some(Version::new(3, 12, 1))
        }

        fn supports_given_rows(&self) -> bool {
            true
        }
    }

    struct RecordingTransaction {
        state: Arc<Mutex<RecordingState>>,
        commit: CommitBehavior,
    }

    impl TransactionOps for RecordingTransaction {
        fn supports_given_rows(&self) -> bool {
            true
        }

        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("projected batch C adapter used a raw query seam") })
        }

        fn query_with_rows(
            &mut self,
            typeql: &str,
            rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            let response = {
                let mut state = self.state.lock().unwrap();
                state.calls.push((typeql.to_owned(), rows));
                state
                    .responses
                    .pop_front()
                    .expect("unexpected provider call")
            };
            match response {
                Response::Documents(values) => {
                    Box::pin(async move { Ok(QueryResult::Documents(values)) })
                }
                Response::Error => Box::pin(async {
                    Err(OrmError::QueryExecution(
                        "provider-secret-projected-batch".to_owned(),
                    ))
                }),
            }
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { panic!("projected batch C adapter used unclassified commit") })
        }

        fn commit_classified(&mut self) -> BoxFuture<'_, Result<(), ClassifiedCommitError>> {
            self.state.lock().unwrap().commits += 1;
            let commit = self.commit;
            Box::pin(async move {
                match commit {
                    CommitBehavior::Success => Ok(()),
                    CommitBehavior::Failure => Err(ClassifiedCommitError::Driver {
                        certainty: CommitFailureCertainty::DefinitelyAborted,
                        message: "provider-secret-commit".to_owned(),
                    }),
                }
            })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().closes += 1;
            Box::pin(async { Ok(()) })
        }
    }

    fn make_database(
        package: &TypeBridgeSchemaPackage,
        responses: Vec<Response>,
        commit: CommitBehavior,
        ceiling: Option<QueryExecutionResourceLimits>,
    ) -> (Box<TypeBridgeDatabase>, Arc<Mutex<RecordingState>>) {
        let state = Arc::new(Mutex::new(RecordingState {
            responses: responses.into(),
            ..RecordingState::default()
        }));
        let database = Database::with_backend(
            Box::new(RecordingBackend {
                state: Arc::clone(&state),
                commit,
            }),
            "projected-batch-c",
        );
        (
            Box::new(TypeBridgeDatabase::from_test_database_with_answer_ceiling(
                Arc::clone(&package.state),
                database,
                ceiling,
            )),
            state,
        )
    }

    fn limits_descriptor(limits: QueryExecutionResourceLimits) -> TypeBridgeQueryExecutionLimitsV1 {
        TypeBridgeQueryExecutionLimitsV1 {
            struct_size: size_of::<TypeBridgeQueryExecutionLimitsV1>() as u32,
            version: 1,
            timeout_milliseconds: limits.timeout_milliseconds,
            items: limits.items,
            bytes: limits.bytes,
            graph_nodes: limits.graph_nodes,
            attribute_values: limits.attribute_values,
            collection_members: limits.collection_members,
            role_players: limits.role_players,
            statements: limits.statements,
            reserved0: 0,
            reserved: [0; 4],
        }
    }

    fn target_iid(kind: TypeKind, ordinal: usize) -> String {
        format!(
            "0x{}{}",
            if kind == TypeKind::Entity { "2" } else { "3" },
            ordinal
        )
    }

    fn empty_iid() -> TypeBridgeByteView {
        TypeBridgeByteView {
            data: ptr::null(),
            length: 0,
        }
    }

    fn open_builder(
        package: &TypeBridgeSchemaPackage,
        token: &TypeBridgeProjectedTokenV1,
        operation: ProjectedBatchOperation,
        limits: *const TypeBridgeQueryExecutionLimitsV1,
        cancellation: *const TypeBridgeCancellation,
    ) -> *mut TypeBridgeProjectedBatchBuilder {
        let mut builder = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all retained inputs and both output slots are valid.
            unsafe {
                type_bridge_projected_batch_builder_open_v1_impl(
                    package,
                    token,
                    operation_to_c(operation),
                    limits,
                    cancellation,
                    &mut builder,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(!builder.is_null());
        assert!(diagnostics.is_null());
        builder
    }

    fn add_row(
        builder: *mut TypeBridgeProjectedBatchBuilder,
        operation: ProjectedBatchOperation,
        iid: &str,
        create: *const TypeBridgeProjectedCreate,
    ) -> Result<(), *mut TypeBridgeExecutionDiagnostics> {
        let view = if matches!(
            operation,
            ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put
        ) {
            empty_iid()
        } else {
            byte_view(iid.as_bytes())
        };
        let mut diagnostics = ptr::dangling_mut();
        let status =
            // SAFETY: builder is live, row inputs remain valid, and diagnostics is writable.
            unsafe {
                type_bridge_projected_batch_builder_add_v1_impl(
                    builder,
                    view,
                    create,
                    &mut diagnostics,
                )
            };
        if status == TypeBridgeStatus::Ok {
            assert!(diagnostics.is_null());
            Ok(())
        } else {
            assert!(!diagnostics.is_null());
            Err(diagnostics)
        }
    }

    fn finish_builder(
        mut builder: *mut TypeBridgeProjectedBatchBuilder,
    ) -> (
        *mut TypeBridgeProjectedBatch,
        *mut TypeBridgeProjectedBatchBuilder,
    ) {
        let mut batch = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: builder owner slot and both output slots are valid and disjoint.
            unsafe {
                type_bridge_projected_batch_builder_finish_impl(
                    &mut builder,
                    &mut batch,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(builder.is_null());
        assert!(!batch.is_null());
        assert!(diagnostics.is_null());
        (batch, builder)
    }

    fn build_batch(
        package: &TypeBridgeSchemaPackage,
        token: &TypeBridgeProjectedTokenV1,
        kind: TypeKind,
        operation: ProjectedBatchOperation,
        creates: &[Box<TypeBridgeProjectedCreate>],
    ) -> *mut TypeBridgeProjectedBatch {
        let builder = open_builder(package, token, operation, ptr::null(), ptr::null());
        for (ordinal, create) in creates.iter().enumerate() {
            let create = if operation == ProjectedBatchOperation::Delete {
                ptr::null()
            } else {
                &**create
            };
            add_row(builder, operation, &target_iid(kind, ordinal), create).unwrap();
        }
        finish_builder(builder).0
    }

    fn hydration(kind: TypeKind, ordinal: usize) -> Value {
        let iid = target_iid(kind, ordinal);
        let identifier = if ordinal == 0 { "alpha" } else { "beta" };
        if kind == TypeKind::Entity {
            json!({
                "ordinal": ordinal,
                "_iid": iid,
                "_type": "person",
                "attributes": {"identifier": [{"value": identifier}]},
                "role_players": []
            })
        } else {
            json!({
                "ordinal": ordinal,
                "_iid": iid,
                "_type": "membership",
                "attributes": {"identifier": [{"value": identifier}]},
                "role_players": [{
                    "role": "member",
                    "iid": "0x10",
                    "type_name": "person",
                    "attributes": {"identifier": [{"value": "player"}]}
                }]
            })
        }
    }

    fn responses(kind: TypeKind, operation: ProjectedBatchOperation) -> Vec<Response> {
        let mut responses = Vec::new();
        if operation == ProjectedBatchOperation::Put
            || (kind == TypeKind::Relation && operation != ProjectedBatchOperation::Delete)
        {
            let prerequisites = if kind == TypeKind::Relation {
                vec![
                    json!({"kind": 1, "ordinal": 1, "reference_ordinal": 1, "iid": "0x10", "type": "person"}),
                    json!({"kind": 1, "ordinal": 0, "reference_ordinal": 0, "iid": "0x10", "type": "person"}),
                ]
            } else {
                vec![]
            };
            responses.push(Response::Documents(prerequisites));
        }
        if operation == ProjectedBatchOperation::Delete {
            responses.push(Response::Documents(vec![
                json!({"ordinal": 1}),
                json!({"ordinal": 0, "iid": target_iid(kind, 0)}),
            ]));
        } else {
            responses.push(Response::Documents(vec![
                json!({"ordinal": 1, "iid": target_iid(kind, 1)}),
                json!({"ordinal": 0, "iid": target_iid(kind, 0)}),
            ]));
            responses.push(Response::Documents(vec![
                hydration(kind, 1),
                hydration(kind, 0),
            ]));
        }
        responses
    }

    fn diagnostic_code(diagnostics: *mut TypeBridgeExecutionDiagnostics) -> String {
        diagnostic(diagnostics).1
    }

    fn close_batch(batch: &mut *mut TypeBridgeProjectedBatch) {
        assert_eq!(
            // SAFETY: the slot owns either null or one batch handle.
            unsafe { type_bridge_projected_batch_close_impl(batch) },
            TypeBridgeStatus::Ok
        );
        assert!(batch.is_null());
    }

    fn close_result(result: &mut *mut TypeBridgeProjectedBatchResult) {
        assert_eq!(
            // SAFETY: the slot owns either null or one result handle.
            unsafe { type_bridge_projected_batch_result_close_impl(result) },
            TypeBridgeStatus::Ok
        );
        assert!(result.is_null());
    }

    fn assert_result_and_close(
        result: &mut *mut TypeBridgeProjectedBatchResult,
        token: &TypeBridgeProjectedTokenV1,
        kind: TypeKind,
        operation: ProjectedBatchOperation,
    ) {
        let mut count = usize::MAX;
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all immutable inputs and both outputs are live and disjoint.
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    *result,
                    token,
                    operation_to_c(operation),
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 2);
        assert!(diagnostics.is_null());

        if operation == ProjectedBatchOperation::Delete {
            let mut thing = ptr::dangling_mut();
            assert_eq!(
                // SAFETY: the delete result is live and outputs are disjoint.
                unsafe {
                    type_bridge_projected_batch_result_thing_at_impl(
                        *result,
                        token,
                        operation_to_c(operation),
                        0,
                        &mut thing,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::InvalidArgument
            );
            assert!(thing.is_null());
            assert_eq!(
                diagnostic_code(diagnostics),
                "c_projected_batch_delete_result_has_no_thing"
            );
            close_diagnostics(&mut diagnostics);
            close_result(result);
            return;
        }

        let mut things = Vec::new();
        for ordinal in 0..2 {
            let mut thing = ptr::dangling_mut();
            assert_eq!(
                // SAFETY: the result is live and each output slot is independent.
                unsafe {
                    type_bridge_projected_batch_result_thing_at_impl(
                        *result,
                        token,
                        operation_to_c(operation),
                        ordinal,
                        &mut thing,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Ok
            );
            assert!(!thing.is_null());
            assert!(diagnostics.is_null());
            things.push(thing);
        }
        close_result(result);
        for (ordinal, thing) in things.iter_mut().enumerate() {
            // SAFETY: thing_at returned an independently owned projected thing.
            assert_eq!(unsafe { &**thing }.value.iid(), target_iid(kind, ordinal));
            close_thing(thing);
        }
    }

    #[test]
    fn builder_row_forms_limits_and_allocation_failures_are_atomic() {
        let package = make_package("batchbuilder");
        let person = type_id(TypeKind::Entity);
        let person_token = model_token(&package, person);
        let alpha = create(&package, TypeKind::Entity, "alpha", "0x10");
        let beta = create(&package, TypeKind::Entity, "beta", "0x10");
        let relation = create(&package, TypeKind::Relation, "relation", "0x10");

        let mut builder = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all inputs and outputs are live and disjoint.
            unsafe {
                type_bridge_projected_batch_builder_open_v1_impl(
                    &package,
                    &person_token,
                    0,
                    ptr::null(),
                    ptr::null(),
                    &mut builder,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(builder.is_null());
        assert_eq!(
            diagnostic_code(diagnostics),
            "c_projected_batch_operation_invalid"
        );
        close_diagnostics(&mut diagnostics);

        let builder = open_builder(
            &package,
            &person_token,
            ProjectedBatchOperation::Insert,
            ptr::null(),
            ptr::null(),
        );
        let mut invalid =
            add_row(builder, ProjectedBatchOperation::Update, "0x20", &*alpha).unwrap_err();
        assert_eq!(
            diagnostic_code(invalid),
            "c_projected_batch_row_form_invalid"
        );
        close_diagnostics(&mut invalid);
        let mut invalid =
            add_row(builder, ProjectedBatchOperation::Insert, "", &*relation).unwrap_err();
        assert_eq!(
            diagnostic_code(invalid),
            "c_projected_batch_create_model_mismatch"
        );
        close_diagnostics(&mut invalid);
        assert_eq!(unsafe { &*builder }.rows.len(), 0);
        assert_eq!(unsafe { &*builder }.control.len(), 0);

        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchBuilderCreateClone, 0);
            let mut failure =
                add_row(builder, ProjectedBatchOperation::Insert, "", &*alpha).unwrap_err();
            assert_eq!(diagnostic_code(failure), "c_allocation_exhausted");
            close_diagnostics(&mut failure);
        }
        assert_eq!(unsafe { &*builder }.rows.len(), 0);
        assert_eq!(unsafe { &*builder }.control.len(), 0);
        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchBuilderRows, 0);
            let mut failure =
                add_row(builder, ProjectedBatchOperation::Insert, "", &*alpha).unwrap_err();
            assert_eq!(diagnostic_code(failure), "c_allocation_exhausted");
            close_diagnostics(&mut failure);
        }
        assert_eq!(unsafe { &*builder }.rows.len(), 0);
        assert_eq!(unsafe { &*builder }.control.len(), 0);
        add_row(builder, ProjectedBatchOperation::Insert, "", &*alpha).unwrap();
        assert_eq!(unsafe { &*builder }.rows.len(), 1);
        assert_eq!(unsafe { &*builder }.control.len(), 1);
        let mut builder = builder;
        assert_eq!(
            // SAFETY: this owner slot uniquely owns the live builder.
            unsafe { type_bridge_projected_batch_builder_close_impl(&mut builder) },
            TypeBridgeStatus::Ok
        );
        assert!(builder.is_null());

        let limited = QueryExecutionResourceLimits {
            items: 1,
            ..QueryExecutionResourceLimits::default()
        };
        let limited = limits_descriptor(limited);
        let builder = open_builder(
            &package,
            &person_token,
            ProjectedBatchOperation::Insert,
            &limited,
            ptr::null(),
        );
        add_row(builder, ProjectedBatchOperation::Insert, "", &*alpha).unwrap();
        let mut limited_failure =
            add_row(builder, ProjectedBatchOperation::Insert, "", &*beta).unwrap_err();
        assert_eq!(diagnostic_code(limited_failure), "batch_item_limit");
        close_diagnostics(&mut limited_failure);
        assert_eq!(unsafe { &*builder }.rows.len(), 1);
        assert_eq!(unsafe { &*builder }.control.len(), 1);
        let mut builder = builder;
        assert_eq!(
            // SAFETY: owner slot uniquely owns the builder.
            unsafe { type_bridge_projected_batch_builder_close_impl(&mut builder) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn iid_copy_and_finish_failures_are_recoverable_and_success_drains_once() {
        let package = make_package("batchfinish");
        let token = model_token(&package, type_id(TypeKind::Entity));
        let alpha = create(&package, TypeKind::Entity, "alpha", "0x10");
        let beta = create(&package, TypeKind::Entity, "beta", "0x10");
        let builder = open_builder(
            &package,
            &token,
            ProjectedBatchOperation::Update,
            ptr::null(),
            ptr::null(),
        );
        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchBuilderIidBytes, 0);
            let mut failure =
                add_row(builder, ProjectedBatchOperation::Update, "0x20", &*alpha).unwrap_err();
            assert_eq!(diagnostic_code(failure), "c_allocation_exhausted");
            close_diagnostics(&mut failure);
        }
        assert_eq!(unsafe { &*builder }.rows.len(), 0);
        assert_eq!(unsafe { &*builder }.control.len(), 0);
        add_row(builder, ProjectedBatchOperation::Update, "0x20", &*alpha).unwrap();
        add_row(builder, ProjectedBatchOperation::Update, "0x21", &*beta).unwrap();

        let mut builder = builder;
        for site in [
            AllocationSite::ProjectedBatchHandle,
            AllocationSite::ProjectedBatchFinishRows,
            AllocationSite::ProjectedBatchFinishTargets,
            AllocationSite::ProjectedBatchFinishKeys,
        ] {
            let mut batch = ptr::dangling_mut();
            let mut diagnostics = ptr::dangling_mut();
            {
                let _failure = inject_failure(site, 0);
                assert_eq!(
                    // SAFETY: all ownership/output slots are live and disjoint.
                    unsafe {
                        type_bridge_projected_batch_builder_finish_impl(
                            &mut builder,
                            &mut batch,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::ResourceLimit
                );
            }
            assert!(!builder.is_null());
            assert!(batch.is_null());
            assert_eq!(diagnostic_code(diagnostics), "c_allocation_exhausted");
            close_diagnostics(&mut diagnostics);
            assert_eq!(unsafe { &*builder }.rows.len(), 2);
            assert_eq!(unsafe { &*builder }.control.len(), 2);
        }
        let (mut batch, consumed) = finish_builder(builder);
        builder = consumed;
        assert!(builder.is_null());
        assert_eq!(unsafe { &*batch }.value.len(), 2);
        assert_eq!(
            unsafe { &*batch }.value.resource_ceiling(),
            QueryExecutionResourceLimits::default().effective()
        );
        close_batch(&mut batch);
        assert_eq!(
            // SAFETY: closing an already-null builder owner slot is idempotent.
            unsafe { type_bridge_projected_batch_builder_close_impl(&mut builder) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn duplicate_finish_and_cancellation_keep_builder_ownership_exact() {
        let package = make_package("batchduplicate");
        let token = model_token(&package, type_id(TypeKind::Entity));
        let alpha = create(&package, TypeKind::Entity, "alpha", "0x10");
        let builder = open_builder(
            &package,
            &token,
            ProjectedBatchOperation::Insert,
            ptr::null(),
            ptr::null(),
        );
        for _ in 0..2 {
            add_row(builder, ProjectedBatchOperation::Insert, "", &*alpha).unwrap();
        }
        let mut builder = builder;
        let mut batch = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: ownership and output slots are live and disjoint.
            unsafe {
                type_bridge_projected_batch_builder_finish_impl(
                    &mut builder,
                    &mut batch,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(!builder.is_null());
        assert!(batch.is_null());
        assert_eq!(diagnostic_code(diagnostics), "duplicate_batch_key");
        close_diagnostics(&mut diagnostics);
        assert_eq!(unsafe { &*builder }.rows.len(), 2);
        assert_eq!(
            // SAFETY: owner slot uniquely owns the recoverable builder.
            unsafe { type_bridge_projected_batch_builder_close_impl(&mut builder) },
            TypeBridgeStatus::Ok
        );

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            // SAFETY: output slot is valid.
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        let builder = open_builder(
            &package,
            &token,
            ProjectedBatchOperation::Insert,
            ptr::null(),
            cancellation,
        );
        assert_eq!(
            // SAFETY: cancellation remains live.
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let mut cancelled =
            add_row(builder, ProjectedBatchOperation::Insert, "", &*alpha).unwrap_err();
        assert_eq!(diagnostic_code(cancelled), "provider_cancelled");
        close_diagnostics(&mut cancelled);
        assert_eq!(unsafe { &*builder }.rows.len(), 0);
        let (mut empty, _) = finish_builder(builder);
        assert!(unsafe { &*empty }.value.is_empty());
        close_batch(&mut empty);
        assert_eq!(
            // SAFETY: cancellation owner slot is live.
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert!(cancellation.is_null());
    }

    #[test]
    fn owned_and_borrowed_execution_route_both_models_and_all_operations_in_order() {
        for (kind, prefix) in [
            (TypeKind::Entity, "batchmatrixentity"),
            (TypeKind::Relation, "batchmatrixrelation"),
        ] {
            let package = make_package(prefix);
            let token = model_token(&package, type_id(kind));
            let creates = [
                create(&package, kind, "alpha", "0x10"),
                create(&package, kind, "beta", "0x10"),
            ];
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Put,
                ProjectedBatchOperation::Update,
                ProjectedBatchOperation::Delete,
            ] {
                let mut batch = build_batch(&package, &token, kind, operation, &creates);
                let expected_calls = responses(kind, operation).len();

                let (database, state) = make_database(
                    &package,
                    responses(kind, operation),
                    CommitBehavior::Success,
                    None,
                );
                let mut result = ptr::dangling_mut();
                let mut diagnostics = ptr::dangling_mut();
                assert_eq!(
                    // SAFETY: every input is retained and both outputs are disjoint.
                    unsafe {
                        type_bridge_database_projected_batch_execute_v1_impl(
                            &*database,
                            batch,
                            ptr::null(),
                            ptr::null(),
                            &mut result,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok
                );
                assert!(!result.is_null());
                assert!(diagnostics.is_null());
                assert_result_and_close(&mut result, &token, kind, operation);
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.calls.len(), expected_calls);
                    assert!(state.responses.is_empty());
                    assert_eq!(state.commits, 1);
                    assert_eq!(state.rollbacks, 0);
                    let mutation_index = usize::from(
                        operation == ProjectedBatchOperation::Put
                            || (kind == TypeKind::Relation
                                && operation != ProjectedBatchOperation::Delete),
                    );
                    let mutation = &state.calls[mutation_index];
                    assert_eq!(mutation.1.rows.len(), 2);
                    match operation {
                        ProjectedBatchOperation::Insert => {
                            assert!(mutation.0.contains("\ninsert\n"));
                        }
                        ProjectedBatchOperation::Put => {
                            assert!(mutation.0.contains("\nput\n"));
                        }
                        ProjectedBatchOperation::Update => {
                            assert!(mutation.0.contains("delete try"));
                        }
                        ProjectedBatchOperation::Delete => {
                            assert!(mutation.0.contains("delete try { $thing; }"));
                        }
                    }
                }

                let (database, state) = make_database(
                    &package,
                    responses(kind, operation),
                    CommitBehavior::Success,
                    None,
                );
                let context = database
                    .open_transaction_context(TxType::Write)
                    .expect("borrowed write transaction opens");
                let mut transaction = Box::into_raw(Box::new(
                    TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                        &database, context, None,
                    ),
                ));
                let mut result = ptr::dangling_mut();
                assert_eq!(
                    // SAFETY: the borrowed transaction, batch, and outputs are live.
                    unsafe {
                        type_bridge_write_transaction_projected_batch_execute_v1_impl(
                            transaction,
                            batch,
                            ptr::null(),
                            ptr::null(),
                            &mut result,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::Ok
                );
                assert_result_and_close(&mut result, &token, kind, operation);
                {
                    let state = state.lock().unwrap();
                    assert_eq!(state.opens, [TxType::Write]);
                    assert_eq!(state.calls.len(), expected_calls);
                    assert_eq!(state.commits, 0);
                    assert_eq!(state.rollbacks, 0);
                }
                assert_eq!(
                    // SAFETY: the transaction owner and diagnostics slots are live.
                    unsafe {
                        type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics)
                    },
                    TypeBridgeStatus::Ok
                );
                assert!(transaction.is_null());
                assert!(diagnostics.is_null());
                assert_eq!(state.lock().unwrap().rollbacks, 1);
                close_batch(&mut batch);
            }
        }
    }

    #[test]
    fn explicit_empty_execution_skips_policy_lifecycle_provider_and_mapper_work() {
        let package = make_package("batchempty");
        let token = model_token(&package, type_id(TypeKind::Entity));
        let builder = open_builder(
            &package,
            &token,
            ProjectedBatchOperation::Insert,
            ptr::null(),
            ptr::null(),
        );
        let (mut batch, _) = finish_builder(builder);
        let ceiling = QueryExecutionResourceLimits {
            items: 0,
            statements: 0,
            timeout_milliseconds: 0,
            ..QueryExecutionResourceLimits::default()
        };
        let (database, state) =
            make_database(&package, vec![], CommitBehavior::Success, Some(ceiling));
        let mut malformed = limits_descriptor(QueryExecutionResourceLimits::default());
        malformed.version = 99;
        let mut cancellation = ptr::null_mut();
        assert_eq!(
            // SAFETY: cancellation output is valid.
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: cancellation is live.
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let mut result = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: all inputs and outputs are retained and disjoint.
            unsafe {
                type_bridge_database_projected_batch_execute_v1_impl(
                    &*database,
                    batch,
                    &malformed,
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        let mut count = usize::MAX;
        assert_eq!(
            // SAFETY: empty result and outputs are live.
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    result,
                    &token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert_eq!(count, 0);
        close_result(&mut result);
        {
            let state = state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.calls.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        }

        let context = database
            .open_transaction_context(TxType::Write)
            .expect("borrowed empty transaction opens");
        let mut transaction = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                &database,
                context,
                Some(ceiling),
            ),
        ));
        let cause = SdkExecutionDiagnostic::internal_failure();
        unsafe { &*transaction }.block_on(
            unsafe { &*transaction }
                .context()
                .unwrap()
                .latch_rollback_only(&cause),
        );
        unsafe { &*transaction }.mark_poisoned();
        assert_eq!(
            // SAFETY: empty execution deliberately accepts this live rollback-only target.
            unsafe {
                type_bridge_write_transaction_projected_batch_execute_v1_impl(
                    transaction,
                    batch,
                    &malformed,
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        close_result(&mut result);
        assert!(state.lock().unwrap().calls.is_empty());
        assert_eq!(
            // SAFETY: rollback consumes the live borrowed transaction.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());

        let other_package = make_package("batchemptyother");
        let (other_database, other_state) =
            make_database(&other_package, vec![], CommitBehavior::Success, None);
        assert_eq!(
            // SAFETY: all handles remain valid; semantic package fencing rejects the pair.
            unsafe {
                type_bridge_database_projected_batch_execute_v1_impl(
                    &*other_database,
                    batch,
                    &malformed,
                    cancellation,
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(result.is_null());
        assert_eq!(
            diagnostic_code(diagnostics),
            "c_projected_batch_package_mismatch"
        );
        close_diagnostics(&mut diagnostics);
        assert!(other_state.lock().unwrap().opens.is_empty());

        assert_eq!(
            // SAFETY: cancellation owner slot is live.
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        close_batch(&mut batch);
    }

    #[test]
    fn nonempty_checkpoint_ceiling_and_result_reservations_precede_provider_dispatch() {
        let package = make_package("batchpredispatch");
        let token = model_token(&package, type_id(TypeKind::Entity));
        let creates = [
            create(&package, TypeKind::Entity, "alpha", "0x10"),
            create(&package, TypeKind::Entity, "beta", "0x10"),
        ];
        let mut batch = build_batch(
            &package,
            &token,
            TypeKind::Entity,
            ProjectedBatchOperation::Insert,
            &creates,
        );

        for site in [
            AllocationSite::ProjectedBatchResultHandle,
            AllocationSite::ProjectedBatchResultThings,
            AllocationSite::ProjectedBatchResultThingStorage,
        ] {
            let (database, state) = make_database(
                &package,
                responses(TypeKind::Entity, ProjectedBatchOperation::Insert),
                CommitBehavior::Success,
                None,
            );
            let mut result = ptr::dangling_mut();
            let mut diagnostics = ptr::dangling_mut();
            {
                let _failure = inject_failure(site, 0);
                assert_eq!(
                    // SAFETY: retained inputs and disjoint outputs satisfy the private ABI.
                    unsafe {
                        type_bridge_database_projected_batch_execute_v1_impl(
                            &*database,
                            batch,
                            ptr::null(),
                            ptr::null(),
                            &mut result,
                            &mut diagnostics,
                        )
                    },
                    TypeBridgeStatus::ResourceLimit
                );
            }
            assert!(result.is_null());
            assert_eq!(diagnostic_code(diagnostics), "c_allocation_exhausted");
            close_diagnostics(&mut diagnostics);
            let state = state.lock().unwrap();
            assert!(state.opens.is_empty());
            assert!(state.calls.is_empty());
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        }

        let tight = QueryExecutionResourceLimits {
            statements: 1,
            ..QueryExecutionResourceLimits::default()
        };
        for (target_ceiling, call_limits) in
            [(Some(tight), None), (None, Some(limits_descriptor(tight)))]
        {
            let (database, state) =
                make_database(&package, vec![], CommitBehavior::Success, target_ceiling);
            let mut result = ptr::dangling_mut();
            let mut diagnostics = ptr::dangling_mut();
            let limits = call_limits
                .as_ref()
                .map_or(ptr::null(), |value| value as *const _);
            assert_eq!(
                // SAFETY: all handles and outputs are valid for this call.
                unsafe {
                    type_bridge_database_projected_batch_execute_v1_impl(
                        &*database,
                        batch,
                        limits,
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ResourceLimit
            );
            assert!(result.is_null());
            assert_eq!(diagnostic_code(diagnostics), "batch_statement_limit");
            close_diagnostics(&mut diagnostics);
            assert!(state.lock().unwrap().calls.is_empty());
        }

        let mut cancellation = ptr::null_mut();
        assert_eq!(
            // SAFETY: cancellation output is valid.
            unsafe { type_bridge_cancellation_open(&mut cancellation) },
            TypeBridgeStatus::Ok
        );
        assert_eq!(
            // SAFETY: cancellation is live.
            unsafe { type_bridge_cancellation_request(cancellation) },
            TypeBridgeStatus::Ok
        );
        let (database, state) = make_database(&package, vec![], CommitBehavior::Success, None);
        let mut result = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        {
            let _later_failure = inject_failure(AllocationSite::ProjectedBatchResultHandle, 0);
            assert_eq!(
                // SAFETY: cancellation and every other input/output are live.
                unsafe {
                    type_bridge_database_projected_batch_execute_v1_impl(
                        &*database,
                        batch,
                        ptr::null(),
                        cancellation,
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::Cancelled
            );
        }
        assert!(result.is_null());
        assert_eq!(diagnostic_code(diagnostics), "provider_cancelled");
        close_diagnostics(&mut diagnostics);
        assert!(state.lock().unwrap().opens.is_empty());
        assert_eq!(
            // SAFETY: cancellation owner slot is live.
            unsafe { type_bridge_cancellation_close(&mut cancellation) },
            TypeBridgeStatus::Ok
        );

        let (database, state) = make_database(&package, vec![], CommitBehavior::Success, None);
        let context = database
            .open_transaction_context(TxType::Write)
            .expect("poisoned borrowed transaction opens");
        let mut transaction = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                &database, context, None,
            ),
        ));
        // SAFETY: the transaction remains owned by this test until rollback.
        unsafe { &*transaction }.mark_poisoned();
        {
            let _later_failure = inject_failure(AllocationSite::ProjectedBatchResultHandle, 0);
            assert_eq!(
                // SAFETY: transaction and all delegated inputs/outputs are live.
                unsafe {
                    type_bridge_write_transaction_projected_batch_execute_v1_impl(
                        transaction,
                        batch,
                        ptr::null(),
                        ptr::null(),
                        &mut result,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ExecutionFailed
            );
        }
        assert!(result.is_null());
        assert_eq!(diagnostic_code(diagnostics), "c_transaction_poisoned");
        close_diagnostics(&mut diagnostics);
        assert!(state.lock().unwrap().calls.is_empty());
        assert_eq!(
            // SAFETY: rollback consumes the still-live poisoned transaction.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        close_batch(&mut batch);
    }

    #[test]
    fn provider_and_commit_failures_publish_nothing_and_preserve_transaction_rules() {
        let package = make_package("batchfailures");
        let token = model_token(&package, type_id(TypeKind::Entity));
        let creates = [
            create(&package, TypeKind::Entity, "alpha", "0x10"),
            create(&package, TypeKind::Entity, "beta", "0x10"),
        ];
        let mut insert = build_batch(
            &package,
            &token,
            TypeKind::Entity,
            ProjectedBatchOperation::Insert,
            &creates,
        );
        let mutation = || {
            Response::Documents(vec![
                json!({"ordinal": 1, "iid": "0x21"}),
                json!({"ordinal": 0, "iid": "0x20"}),
            ])
        };

        let (database, state) = make_database(
            &package,
            vec![mutation(), Response::Error],
            CommitBehavior::Success,
            None,
        );
        let mut result = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: every input and output is live and disjoint.
            unsafe {
                type_bridge_database_projected_batch_execute_v1_impl(
                    &*database,
                    insert,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(result.is_null());
        close_diagnostics(&mut diagnostics);
        {
            let state = state.lock().unwrap();
            assert_eq!(state.calls.len(), 2);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 1);
        }

        let (database, state) = make_database(
            &package,
            vec![mutation(), Response::Error],
            CommitBehavior::Success,
            None,
        );
        let context = database
            .open_transaction_context(TxType::Write)
            .expect("borrowed failure transaction opens");
        let mut transaction = Box::into_raw(Box::new(
            TypeBridgeWriteTransaction::from_test_context_with_answer_ceiling(
                &database, context, None,
            ),
        ));
        assert_eq!(
            // SAFETY: transaction, batch, and outputs remain live.
            unsafe {
                type_bridge_write_transaction_projected_batch_execute_v1_impl(
                    transaction,
                    insert,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(result.is_null());
        close_diagnostics(&mut diagnostics);
        // SAFETY: the test exclusively owns this live transaction handle.
        let transaction_ref = unsafe { &*transaction };
        assert_eq!(
            transaction_ref.block_on(transaction_ref.context().unwrap().lifecycle_state()),
            TransactionContextState::RollbackOnly
        );
        {
            let state = state.lock().unwrap();
            assert_eq!(state.calls.len(), 2);
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 0);
        }
        assert_eq!(
            // SAFETY: rollback consumes rollback-only transaction ownership.
            unsafe { type_bridge_write_transaction_rollback(&mut transaction, &mut diagnostics) },
            TypeBridgeStatus::Ok
        );
        assert!(diagnostics.is_null());
        assert_eq!(state.lock().unwrap().rollbacks, 1);

        let mut delete = build_batch(
            &package,
            &token,
            TypeKind::Entity,
            ProjectedBatchOperation::Delete,
            &creates,
        );
        let (database, state) = make_database(
            &package,
            responses(TypeKind::Entity, ProjectedBatchOperation::Delete),
            CommitBehavior::Failure,
            None,
        );
        assert_eq!(
            // SAFETY: all retained inputs and disjoint outputs are valid.
            unsafe {
                type_bridge_database_projected_batch_execute_v1_impl(
                    &*database,
                    delete,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(result.is_null());
        close_diagnostics(&mut diagnostics);
        let state = state.lock().unwrap();
        assert_eq!(state.calls.len(), 1);
        assert_eq!(state.commits, 1);
        assert_eq!(state.rollbacks, 0);
        drop(state);

        let mapper_failure = match map_result(
            Arc::clone(&package.state),
            type_id(TypeKind::Entity),
            ProjectedBatchOperation::Insert,
            2,
            reserve_result_things(2).unwrap(),
            ProjectedBatchResult::Deleted,
        ) {
            Ok(_) => panic!("insert mapper accepted a delete-shaped result"),
            Err(diagnostic) => diagnostic,
        };
        assert_eq!(
            mapper_failure.code().as_str(),
            "c_projected_batch_result_shape_invalid"
        );
        close_batch(&mut insert);
        close_batch(&mut delete);
    }

    #[test]
    fn nominal_result_fences_independent_clones_and_deep_alias_checks_fail_closed() {
        let package = make_package("batchnominal");
        let person_token = model_token(&package, type_id(TypeKind::Entity));
        let relation_token = model_token(&package, type_id(TypeKind::Relation));
        let creates = [
            create(&package, TypeKind::Entity, "alpha", "0x10"),
            create(&package, TypeKind::Entity, "beta", "0x10"),
        ];
        let mut batch = build_batch(
            &package,
            &person_token,
            TypeKind::Entity,
            ProjectedBatchOperation::Insert,
            &creates,
        );
        let (database, _) = make_database(
            &package,
            responses(TypeKind::Entity, ProjectedBatchOperation::Insert),
            CommitBehavior::Success,
            None,
        );
        let mut result = ptr::dangling_mut();
        let mut diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: retained inputs and disjoint outputs satisfy the private ABI.
            unsafe {
                type_bridge_database_projected_batch_execute_v1_impl(
                    &*database,
                    batch,
                    ptr::null(),
                    ptr::null(),
                    &mut result,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );

        let mut count = usize::MAX;
        assert_eq!(
            // SAFETY: result, token, and outputs are all live.
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    result,
                    &relation_token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(count, 0);
        assert_eq!(
            diagnostic_code(diagnostics),
            "c_projected_batch_result_model_mismatch"
        );
        close_diagnostics(&mut diagnostics);
        count = usize::MAX;
        assert_eq!(
            // SAFETY: result, token, and outputs are all live.
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    result,
                    &person_token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE,
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(count, 0);
        assert_eq!(
            diagnostic_code(diagnostics),
            "c_projected_batch_result_operation_mismatch"
        );
        close_diagnostics(&mut diagnostics);

        let other_package = make_package("batchnominalother");
        let other_token = model_token(&other_package, type_id(TypeKind::Entity));
        assert_eq!(
            // SAFETY: token is structurally live but intentionally has another package brand.
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    result,
                    &other_token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                    &mut count,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::ExecutionFailed
        );
        assert!(!diagnostics.is_null());
        close_diagnostics(&mut diagnostics);

        let sentinel = ptr::dangling_mut();
        diagnostics = sentinel;
        assert_eq!(
            // SAFETY: hostile overlap is rejected before the forged output is written.
            unsafe {
                type_bridge_projected_batch_result_count_impl(
                    result,
                    &person_token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                    result.cast::<usize>(),
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(diagnostics, sentinel);

        let mut thing = ptr::dangling_mut();
        diagnostics = ptr::dangling_mut();
        assert_eq!(
            // SAFETY: live result and disjoint outputs; index is deliberately invalid.
            unsafe {
                type_bridge_projected_batch_result_thing_at_impl(
                    result,
                    &person_token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                    2,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(thing.is_null());
        assert_eq!(
            diagnostic_code(diagnostics),
            "c_projected_batch_result_index_out_of_bounds"
        );
        close_diagnostics(&mut diagnostics);
        {
            let _failure = inject_failure(AllocationSite::ProjectedBatchResultThingHandle, 0);
            assert_eq!(
                // SAFETY: live result and disjoint outputs.
                unsafe {
                    type_bridge_projected_batch_result_thing_at_impl(
                        result,
                        &person_token,
                        TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                        0,
                        &mut thing,
                        &mut diagnostics,
                    )
                },
                TypeBridgeStatus::ResourceLimit
            );
        }
        assert!(thing.is_null());
        assert_eq!(diagnostic_code(diagnostics), "c_allocation_exhausted");
        close_diagnostics(&mut diagnostics);
        assert_eq!(
            // SAFETY: retry uses the same immutable result after recoverable failure.
            unsafe {
                type_bridge_projected_batch_result_thing_at_impl(
                    result,
                    &person_token,
                    TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT,
                    0,
                    &mut thing,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::Ok
        );
        close_result(&mut result);
        // SAFETY: thing_at returned independent Arc-backed ownership.
        assert_eq!(unsafe { &*thing }.value.iid(), "0x20");
        close_thing(&mut thing);
        assert_eq!(
            // SAFETY: null close is idempotent.
            unsafe { type_bridge_projected_batch_result_close_impl(&mut result) },
            TypeBridgeStatus::Ok
        );
        close_batch(&mut batch);

        let update = open_builder(
            &package,
            &person_token,
            ProjectedBatchOperation::Update,
            ptr::null(),
            ptr::null(),
        );
        add_row(
            update,
            ProjectedBatchOperation::Update,
            "0x20",
            &*creates[0],
        )
        .unwrap();
        // SAFETY: test retains exclusive access to inspect its private builder row.
        let retained_iid = match &unsafe { &*update }.rows[0] {
            ProjectedBatchRow::Update { iid, .. } => iid,
            _ => panic!("update builder retained the wrong row form"),
        };
        let alias = retained_iid
            .as_ptr()
            .cast_mut()
            .cast::<*mut TypeBridgeExecutionDiagnostics>();
        assert_eq!(
            // SAFETY: forged nested overlap is detected before any output write.
            unsafe {
                type_bridge_projected_batch_builder_add_v1_impl(
                    update,
                    byte_view(b"0x21"),
                    &*creates[1],
                    alias,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert_eq!(unsafe { &*update }.rows.len(), 1);

        let mut update = update;
        diagnostics = sentinel;
        let owner_alias = (&mut update as *mut *mut TypeBridgeProjectedBatchBuilder)
            .cast::<*mut TypeBridgeProjectedBatch>();
        assert_eq!(
            // SAFETY: ownership/output overlap is rejected before either is changed.
            unsafe {
                type_bridge_projected_batch_builder_finish_impl(
                    &mut update,
                    owner_alias,
                    &mut diagnostics,
                )
            },
            TypeBridgeStatus::InvalidArgument
        );
        assert!(!update.is_null());
        assert_eq!(diagnostics, sentinel);
        assert_eq!(
            // SAFETY: recoverable builder remains uniquely owned and closable.
            unsafe { type_bridge_projected_batch_builder_close_impl(&mut update) },
            TypeBridgeStatus::Ok
        );
    }

    #[test]
    fn private_adapter_operation_tags_are_exact_and_closed() {
        assert_eq!(
            operation_to_c(ProjectedBatchOperation::Insert),
            TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_INSERT
        );
        assert_eq!(
            operation_to_c(ProjectedBatchOperation::Put),
            TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_PUT
        );
        assert_eq!(
            operation_to_c(ProjectedBatchOperation::Update),
            TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_UPDATE
        );
        assert_eq!(
            operation_to_c(ProjectedBatchOperation::Delete),
            TYPE_BRIDGE_PROJECTED_BATCH_OPERATION_DELETE
        );
        assert_eq!(
            operation_from_c(0).unwrap_err().code().as_str(),
            "c_projected_batch_operation_invalid"
        );
        assert_eq!(
            operation_from_c(5).unwrap_err().code().as_str(),
            "c_projected_batch_operation_invalid"
        );
    }

    #[test]
    fn source_keeps_all_ten_adapter_entries_private_until_atomic_activation() {
        let source = include_str!("projected_batch.rs");
        let marker = ["unsafe", "(no_mangle)"].concat();
        assert!(!source.contains(&marker));
        assert_eq!(
            source
                .matches("pub(crate) unsafe extern \"C\" fn type_bridge_")
                .count(),
            10
        );
    }
}
