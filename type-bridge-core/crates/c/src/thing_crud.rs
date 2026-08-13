//! Shared generated-only exact thing CRUD over projected tokens and opaque values.

use std::mem::size_of;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;

use type_bridge_contract::id::{
    MAX_THING_IID_HEX_DIGITS, TypeId, TypeKind, is_canonical_thing_iid,
};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticMessage, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_orm::{ProjectedCrudExecutor, ProjectedThing, TxType, lower_execution_error};

use crate::abi::{SchemaPackageState, TypeBridgeByteView, TypeBridgeStatus, guarded};
use crate::allocation::{AllocationSite, ReservedBox, allocation_exhausted};
use crate::execution_diagnostic::{TypeBridgeExecutionDiagnostics, return_execution_error};
use crate::projected_model::{TypeBridgeProjectedCreate, TypeBridgeProjectedThing};
use crate::projected_token::{TypeBridgeProjectedTokenV1, resolve_model_token};
use crate::projected_value::{invalid_brand_diagnostic, same_package_brand};
use crate::runtime::{
    TypeBridgeCancellation, TypeBridgeDatabase, TypeBridgeReadTransaction,
    TypeBridgeWriteTransaction, poisoned_transaction_diagnostic,
};

const MAX_THING_IID_BYTES: usize = 2 + MAX_THING_IID_HEX_DIGITS;

#[derive(Clone, Copy)]
pub(crate) struct MemoryRange {
    start: usize,
    length: usize,
}

impl MemoryRange {
    pub(crate) fn of_object<T>(pointer: *const T) -> Option<Self> {
        (!pointer.is_null()).then_some(Self {
            start: pointer.addr(),
            length: size_of::<T>(),
        })
    }

    fn of_output<T>(pointer: *mut T) -> Option<Self> {
        Self::of_object(pointer.cast_const())
    }

    pub(crate) fn of_bytes(view: TypeBridgeByteView) -> Option<Self> {
        (view.length != 0 && !view.data.is_null()).then_some(Self {
            start: view.data.addr(),
            length: view.length,
        })
    }

    fn overlaps(self, other: Self) -> bool {
        let Some(left_end) = self.start.checked_add(self.length) else {
            return true;
        };
        let Some(right_end) = other.start.checked_add(other.length) else {
            return true;
        };
        self.start < right_end && other.start < left_end
    }
}

fn any_input_aliases_outputs(
    inputs: &[Option<MemoryRange>],
    outputs: &[Option<MemoryRange>],
) -> bool {
    inputs.iter().flatten().any(|input| {
        outputs
            .iter()
            .flatten()
            .any(|output| input.overlaps(*output))
    })
}

fn outputs_alias(outputs: &[Option<MemoryRange>]) -> bool {
    outputs.iter().enumerate().any(|(index, output)| {
        output.is_some_and(|output| {
            outputs[index + 1..]
                .iter()
                .flatten()
                .any(|other| output.overlaps(*other))
        })
    })
}

fn code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static C thing CRUD code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static C thing CRUD message is valid")
}

fn invalid_input(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CrudKind {
    Entity,
    Relation,
}

impl CrudKind {
    const fn type_kind(self) -> TypeKind {
        match self {
            Self::Entity => TypeKind::Entity,
            Self::Relation => TypeKind::Relation,
        }
    }

    fn invalid_iid(self) -> SdkExecutionDiagnostic {
        match self {
            Self::Entity => invalid_input(
                "c_entity_iid_invalid",
                "The entity IID is not canonical TypeDB identity text",
            ),
            Self::Relation => invalid_input(
                "c_relation_iid_invalid",
                "The relation IID is not canonical TypeDB identity text",
            ),
        }
    }

    fn invalid_model_kind(self) -> SdkExecutionDiagnostic {
        match self {
            Self::Entity => invalid_input(
                "c_entity_model_kind_invalid",
                "The generated model token does not identify an entity",
            ),
            Self::Relation => invalid_input(
                "c_relation_model_kind_invalid",
                "The generated model token does not identify a relation",
            ),
        }
    }

    fn create_model_mismatch(self) -> SdkExecutionDiagnostic {
        match self {
            Self::Entity => invalid_input(
                "c_entity_create_model_mismatch",
                "The projected create value does not match the generated entity model token",
            ),
            Self::Relation => invalid_input(
                "c_relation_create_model_mismatch",
                "The projected create value does not match the generated relation model token",
            ),
        }
    }

    fn result_model_mismatch(self) -> SdkExecutionDiagnostic {
        match self {
            Self::Entity => SdkExecutionDiagnostic::integrity(
                code("c_entity_result_model_mismatch"),
                message("The provider returned a projected thing for a different entity model"),
            ),
            Self::Relation => SdkExecutionDiagnostic::integrity(
                code("c_relation_result_model_mismatch"),
                message("The provider returned a projected thing for a different relation model"),
            ),
        }
    }

    fn inactive_transaction(self) -> SdkExecutionDiagnostic {
        match self {
            Self::Entity => invalid_input(
                "c_entity_transaction_inactive",
                "The entity CRUD operation requires an active transaction handle",
            ),
            Self::Relation => invalid_input(
                "c_relation_transaction_inactive",
                "The relation CRUD operation requires an active transaction handle",
            ),
        }
    }
}

fn cancelled() -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::cancelled_before_dispatch()
}

unsafe fn copied_iid(
    kind: CrudKind,
    view: TypeBridgeByteView,
) -> Result<String, SdkExecutionDiagnostic> {
    if view.length == 0 || view.length > MAX_THING_IID_BYTES || view.data.is_null() {
        return Err(kind.invalid_iid());
    }
    // SAFETY: the C caller promises this already-bounded byte range is readable
    // and immutable for the duration of the call. Copying ends that borrow.
    let bytes = unsafe { std::slice::from_raw_parts(view.data, view.length) }.to_vec();
    let iid = String::from_utf8(bytes).map_err(|_| kind.invalid_iid())?;
    if !is_canonical_thing_iid(&iid) {
        return Err(kind.invalid_iid());
    }
    Ok(iid)
}

unsafe fn resolve_model(
    kind: CrudKind,
    package: &Arc<SchemaPackageState>,
    token: *const TypeBridgeProjectedTokenV1,
) -> Result<TypeId, SdkExecutionDiagnostic> {
    // SAFETY: generated token storage is caller-readable for this call; the
    // resolver snapshots and validates every stable word before using it.
    let type_id = unsafe { resolve_model_token(package, token) }?;
    if type_id.kind() != kind.type_kind() {
        return Err(kind.invalid_model_kind());
    }
    Ok(type_id)
}

fn validate_create(
    kind: CrudKind,
    package: &Arc<SchemaPackageState>,
    expected_model: &TypeId,
    create: &TypeBridgeProjectedCreate,
) -> Result<(), SdkExecutionDiagnostic> {
    if !same_package_brand(package, &create.package) {
        return Err(invalid_brand_diagnostic());
    }
    create.value.validate_for(&package.installed_projection)?;
    if create.value.type_id().kind() != kind.type_kind() {
        return Err(kind.invalid_model_kind());
    }
    if create.value.type_id() != expected_model {
        return Err(kind.create_model_mismatch());
    }
    Ok(())
}

fn check_cancellation(
    cancellation: *const TypeBridgeCancellation,
) -> Result<(), SdkExecutionDiagnostic> {
    if !cancellation.is_null()
        // SAFETY: the caller retains the immutable cancellation handle for this call.
        && unsafe { &*cancellation }.is_requested_before_dispatch()
    {
        Err(cancelled())
    } else {
        Ok(())
    }
}

fn initialize_count_outputs(
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if !out_count.is_null() {
        // SAFETY: a non-null output is caller-writable by the ABI contract.
        unsafe { out_count.write_unaligned(0) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: a non-null output is caller-writable by the ABI contract.
        unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    }
    if out_count.is_null()
        || out_diagnostics.is_null()
        || outputs_alias(&[
            MemoryRange::of_output(out_count),
            MemoryRange::of_output(out_diagnostics),
        ])
    {
        Err(TypeBridgeStatus::InvalidArgument)
    } else {
        Ok(())
    }
}

fn initialize_diagnostics_output(
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<(), TypeBridgeStatus> {
    if out_diagnostics.is_null() {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    // SAFETY: the non-null output is caller-writable by the ABI contract.
    unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    Ok(())
}

pub(crate) struct PreparedThingOutputs {
    thing: *mut *mut TypeBridgeProjectedThing,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
}

pub(crate) struct PreparedCountOutputs {
    count: *mut u64,
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
}

pub(crate) struct PreparedDiagnosticsOutput {
    diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
}

pub(crate) unsafe fn prepare_thing_outputs(
    inputs: &[Option<MemoryRange>],
    out_thing: *mut *mut TypeBridgeProjectedThing,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<PreparedThingOutputs, TypeBridgeStatus> {
    let outputs = [
        MemoryRange::of_output(out_thing),
        MemoryRange::of_output(out_diagnostics),
    ];
    if any_input_aliases_outputs(inputs, &outputs) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    if !out_thing.is_null() {
        // SAFETY: input aliasing was rejected and the C slot is caller-writable.
        // Unaligned stores keep hostile partially-overlapping slots contained.
        unsafe { out_thing.write_unaligned(ptr::null_mut()) };
    }
    if !out_diagnostics.is_null() {
        // SAFETY: same as above; clearing overlapping slots is idempotent.
        unsafe { out_diagnostics.write_unaligned(ptr::null_mut()) };
    }
    if out_thing.is_null() || out_diagnostics.is_null() || outputs_alias(&outputs) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    Ok(PreparedThingOutputs {
        thing: out_thing,
        diagnostics: out_diagnostics,
    })
}

pub(crate) fn prepare_count_outputs(
    inputs: &[Option<MemoryRange>],
    out_count: *mut u64,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<PreparedCountOutputs, TypeBridgeStatus> {
    let outputs = [
        MemoryRange::of_output(out_count),
        MemoryRange::of_output(out_diagnostics),
    ];
    if any_input_aliases_outputs(inputs, &outputs) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    initialize_count_outputs(out_count, out_diagnostics)?;
    Ok(PreparedCountOutputs {
        count: out_count,
        diagnostics: out_diagnostics,
    })
}

pub(crate) fn prepare_diagnostics_output(
    inputs: &[Option<MemoryRange>],
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> Result<PreparedDiagnosticsOutput, TypeBridgeStatus> {
    let outputs = [MemoryRange::of_output(out_diagnostics)];
    if any_input_aliases_outputs(inputs, &outputs) {
        return Err(TypeBridgeStatus::InvalidArgument);
    }
    initialize_diagnostics_output(out_diagnostics)?;
    Ok(PreparedDiagnosticsOutput {
        diagnostics: out_diagnostics,
    })
}

enum ThingMutation<'iid> {
    Insert,
    Put,
    Update(&'iid str),
}

pub(crate) enum WriteTarget<'handle> {
    Database(&'handle TypeBridgeDatabase),
    Transaction(&'handle TypeBridgeWriteTransaction),
}

impl WriteTarget<'_> {
    fn package_state(&self) -> &Arc<SchemaPackageState> {
        match self {
            Self::Database(handle) => handle.package_state(),
            Self::Transaction(handle) => handle.package_state(),
        }
    }

    fn run_thing_mutation(
        &self,
        kind: CrudKind,
        operation: ThingMutation<'_>,
        expected_model: &TypeId,
        create: &TypeBridgeProjectedCreate,
    ) -> Result<ProjectedThing, CrudFailure> {
        match self {
            Self::Database(database) => {
                let executor =
                    ProjectedCrudExecutor::new(&database.package_state().installed_projection);
                let context = catch_unwind(AssertUnwindSafe(|| {
                    database.open_transaction_context(TxType::Write)
                }))
                .map_err(|_| CrudFailure::Panic)?
                .map_err(|error| {
                    CrudFailure::Diagnostic(lower_execution_error(
                        error,
                        SdkProviderOperation::OpenWriteTransaction,
                    ))
                })?;
                let executed = catch_unwind(AssertUnwindSafe(|| match (kind, operation) {
                    (CrudKind::Entity, ThingMutation::Insert) => database
                        .block_on(executor.insert_entity_in_transaction(&context, &create.value)),
                    (CrudKind::Entity, ThingMutation::Put) => database
                        .block_on(executor.put_entity_in_transaction(&context, &create.value)),
                    (CrudKind::Entity, ThingMutation::Update(iid)) => database.block_on(
                        executor.update_entity_in_transaction(&context, iid, &create.value),
                    ),
                    (CrudKind::Relation, ThingMutation::Insert) => database
                        .block_on(executor.insert_relation_in_transaction(&context, &create.value)),
                    (CrudKind::Relation, ThingMutation::Put) => database
                        .block_on(executor.put_relation_in_transaction(&context, &create.value)),
                    (CrudKind::Relation, ThingMutation::Update(iid)) => database.block_on(
                        executor.update_relation_in_transaction(&context, iid, &create.value),
                    ),
                }));
                let thing = match executed {
                    Ok(Ok(thing)) => thing,
                    Ok(Err(diagnostic)) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            database.rollback_transaction_context(&context)
                        }));
                        return Err(CrudFailure::Diagnostic(diagnostic));
                    }
                    Err(_) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            database.rollback_transaction_context(&context)
                        }));
                        return Err(CrudFailure::Panic);
                    }
                };
                if thing.type_id() != expected_model {
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        database.rollback_transaction_context(&context)
                    }));
                    return Err(CrudFailure::Diagnostic(kind.result_model_mismatch()));
                }
                database
                    .commit_transaction_context_classified(&context)
                    .map_err(CrudFailure::Diagnostic)?;
                Ok(thing)
            }
            Self::Transaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                let context = transaction
                    .context()
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))?;
                let executor =
                    ProjectedCrudExecutor::new(&transaction.package_state().installed_projection);
                let thing = match catch_unwind(AssertUnwindSafe(|| match (kind, operation) {
                    (CrudKind::Entity, ThingMutation::Insert) => transaction
                        .block_on(executor.insert_entity_in_transaction(context, &create.value)),
                    (CrudKind::Entity, ThingMutation::Put) => transaction
                        .block_on(executor.put_entity_in_transaction(context, &create.value)),
                    (CrudKind::Entity, ThingMutation::Update(iid)) => transaction.block_on(
                        executor.update_entity_in_transaction(context, iid, &create.value),
                    ),
                    (CrudKind::Relation, ThingMutation::Insert) => transaction
                        .block_on(executor.insert_relation_in_transaction(context, &create.value)),
                    (CrudKind::Relation, ThingMutation::Put) => transaction
                        .block_on(executor.put_relation_in_transaction(context, &create.value)),
                    (CrudKind::Relation, ThingMutation::Update(iid)) => transaction.block_on(
                        executor.update_relation_in_transaction(context, iid, &create.value),
                    ),
                })) {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => {
                        transaction.mark_poisoned();
                        Err(CrudFailure::Panic)
                    }
                }?;
                if thing.type_id() != expected_model {
                    return Err(CrudFailure::Diagnostic(kind.result_model_mismatch()));
                }
                Ok(thing)
            }
        }
    }

    fn preflight_thing_mutation(
        &self,
        kind: CrudKind,
        create: &TypeBridgeProjectedCreate,
    ) -> Result<(), CrudFailure> {
        match self {
            Self::Transaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                if transaction.context().is_none() {
                    return Err(CrudFailure::Diagnostic(kind.inactive_transaction()));
                }
                Ok(())
            }
            Self::Database(_) if kind != CrudKind::Relation => Ok(()),
            Self::Database(database) => {
                let executor =
                    ProjectedCrudExecutor::new(&database.package_state().installed_projection);
                catch_unwind(AssertUnwindSafe(|| {
                    executor.preflight_relation_create_for_database_with_compatibility(
                        database.orm_database(),
                        &create.value,
                    )
                }))
                .map_err(|_| CrudFailure::Panic)?
                .map_err(|failure| CrudFailure::Diagnostic(failure.diagnostic().clone()))
            }
        }
    }

    fn run_delete(&self, kind: CrudKind, type_id: &TypeId, iid: &str) -> Result<(), CrudFailure> {
        match self {
            Self::Database(database) => {
                let context = catch_unwind(AssertUnwindSafe(|| {
                    database.open_transaction_context(TxType::Write)
                }))
                .map_err(|_| CrudFailure::Panic)?
                .map_err(|error| {
                    CrudFailure::Diagnostic(lower_execution_error(
                        error,
                        SdkProviderOperation::OpenWriteTransaction,
                    ))
                })?;
                let executor =
                    ProjectedCrudExecutor::new(&database.package_state().installed_projection);
                let executed = catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => database.block_on(
                        executor.delete_entity_by_iid_in_transaction(&context, type_id, iid),
                    ),
                    CrudKind::Relation => database.block_on(
                        executor.delete_relation_by_iid_in_transaction(&context, type_id, iid),
                    ),
                }));
                match executed {
                    Ok(Ok(())) => {}
                    Ok(Err(diagnostic)) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            database.rollback_transaction_context(&context)
                        }));
                        return Err(CrudFailure::Diagnostic(diagnostic));
                    }
                    Err(_) => {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            database.rollback_transaction_context(&context)
                        }));
                        return Err(CrudFailure::Panic);
                    }
                }
                database
                    .commit_transaction_context_classified(&context)
                    .map_err(CrudFailure::Diagnostic)
            }
            Self::Transaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                let context = transaction
                    .context()
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))?;
                let executor =
                    ProjectedCrudExecutor::new(&transaction.package_state().installed_projection);
                match catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => transaction.block_on(
                        executor.delete_entity_by_iid_in_transaction(context, type_id, iid),
                    ),
                    CrudKind::Relation => transaction.block_on(
                        executor.delete_relation_by_iid_in_transaction(context, type_id, iid),
                    ),
                })) {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => {
                        transaction.mark_poisoned();
                        Err(CrudFailure::Panic)
                    }
                }
            }
        }
    }
}

pub(crate) enum ReadTarget<'handle> {
    Database(&'handle TypeBridgeDatabase),
    ReadTransaction(&'handle TypeBridgeReadTransaction),
    WriteTransaction(&'handle TypeBridgeWriteTransaction),
}

impl ReadTarget<'_> {
    fn package_state(&self) -> &Arc<SchemaPackageState> {
        match self {
            Self::Database(handle) => handle.package_state(),
            Self::ReadTransaction(handle) => handle.package_state(),
            Self::WriteTransaction(handle) => handle.package_state(),
        }
    }

    fn preflight_get(&self, kind: CrudKind) -> Result<(), CrudFailure> {
        match self {
            Self::Database(_) => Ok(()),
            Self::ReadTransaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                transaction
                    .context()
                    .map(|_| ())
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))
            }
            Self::WriteTransaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                transaction
                    .context()
                    .map(|_| ())
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))
            }
        }
    }

    fn run_get(
        &self,
        kind: CrudKind,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<Option<ProjectedThing>, CrudFailure> {
        match self {
            Self::Database(database) => {
                let context = catch_unwind(AssertUnwindSafe(|| {
                    database.open_transaction_context(TxType::Read)
                }))
                .map_err(|_| CrudFailure::Panic)?
                .map_err(|error| {
                    CrudFailure::Diagnostic(lower_execution_error(
                        error,
                        SdkProviderOperation::OpenReadTransaction,
                    ))
                })?;
                let executor =
                    ProjectedCrudExecutor::new(&database.package_state().installed_projection);
                let executed = catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => database.block_on(
                        executor.get_entity_by_iid_in_transaction(&context, type_id, iid),
                    ),
                    CrudKind::Relation => database.block_on(
                        executor.get_relation_by_iid_in_transaction(&context, type_id, iid),
                    ),
                }));
                let result = match executed {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => Err(CrudFailure::Panic),
                };
                let closed = catch_unwind(AssertUnwindSafe(|| {
                    database.close_transaction_context(&context)
                }));
                match (result, closed) {
                    (Err(failure), _) => Err(failure),
                    (Ok(_), Err(_)) => Err(CrudFailure::Panic),
                    (Ok(_), Ok(Err(error))) => Err(CrudFailure::Diagnostic(lower_execution_error(
                        error,
                        SdkProviderOperation::Close,
                    ))),
                    (Ok(value), Ok(Ok(()))) => Ok(value),
                }
            }
            Self::ReadTransaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                let context = transaction
                    .context()
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))?;
                let executor =
                    ProjectedCrudExecutor::new(&transaction.package_state().installed_projection);
                match catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => transaction
                        .block_on(executor.get_entity_by_iid_in_transaction(context, type_id, iid)),
                    CrudKind::Relation => transaction.block_on(
                        executor.get_relation_by_iid_in_transaction(context, type_id, iid),
                    ),
                })) {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => {
                        transaction.mark_poisoned();
                        Err(CrudFailure::Panic)
                    }
                }
            }
            Self::WriteTransaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                let context = transaction
                    .context()
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))?;
                let executor =
                    ProjectedCrudExecutor::new(&transaction.package_state().installed_projection);
                match catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => transaction
                        .block_on(executor.get_entity_by_iid_in_transaction(context, type_id, iid)),
                    CrudKind::Relation => transaction.block_on(
                        executor.get_relation_by_iid_in_transaction(context, type_id, iid),
                    ),
                })) {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => {
                        transaction.mark_poisoned();
                        Err(CrudFailure::Panic)
                    }
                }
            }
        }
    }

    fn run_count(&self, kind: CrudKind, type_id: &TypeId) -> Result<u64, CrudFailure> {
        match self {
            Self::Database(database) => {
                let context = catch_unwind(AssertUnwindSafe(|| {
                    database.open_transaction_context(TxType::Read)
                }))
                .map_err(|_| CrudFailure::Panic)?
                .map_err(|error| {
                    CrudFailure::Diagnostic(lower_execution_error(
                        error,
                        SdkProviderOperation::OpenReadTransaction,
                    ))
                })?;
                let executor =
                    ProjectedCrudExecutor::new(&database.package_state().installed_projection);
                let executed = catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => {
                        database.block_on(executor.count_entities_in_transaction(&context, type_id))
                    }
                    CrudKind::Relation => database
                        .block_on(executor.count_relations_in_transaction(&context, type_id)),
                }));
                let result = match executed {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => Err(CrudFailure::Panic),
                };
                let closed = catch_unwind(AssertUnwindSafe(|| {
                    database.close_transaction_context(&context)
                }));
                match (result, closed) {
                    (Err(failure), _) => Err(failure),
                    (Ok(_), Err(_)) => Err(CrudFailure::Panic),
                    (Ok(_), Ok(Err(error))) => Err(CrudFailure::Diagnostic(lower_execution_error(
                        error,
                        SdkProviderOperation::Close,
                    ))),
                    (Ok(value), Ok(Ok(()))) => Ok(value),
                }
            }
            Self::ReadTransaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                let context = transaction
                    .context()
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))?;
                let executor =
                    ProjectedCrudExecutor::new(&transaction.package_state().installed_projection);
                match catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => transaction
                        .block_on(executor.count_entities_in_transaction(context, type_id)),
                    CrudKind::Relation => transaction
                        .block_on(executor.count_relations_in_transaction(context, type_id)),
                })) {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => {
                        transaction.mark_poisoned();
                        Err(CrudFailure::Panic)
                    }
                }
            }
            Self::WriteTransaction(transaction) => {
                if transaction.is_poisoned() {
                    return Err(CrudFailure::Diagnostic(poisoned_transaction_diagnostic()));
                }
                let context = transaction
                    .context()
                    .ok_or_else(|| CrudFailure::Diagnostic(kind.inactive_transaction()))?;
                let executor =
                    ProjectedCrudExecutor::new(&transaction.package_state().installed_projection);
                match catch_unwind(AssertUnwindSafe(|| match kind {
                    CrudKind::Entity => transaction
                        .block_on(executor.count_entities_in_transaction(context, type_id)),
                    CrudKind::Relation => transaction
                        .block_on(executor.count_relations_in_transaction(context, type_id)),
                })) {
                    Ok(result) => result.map_err(CrudFailure::Diagnostic),
                    Err(_) => {
                        transaction.mark_poisoned();
                        Err(CrudFailure::Panic)
                    }
                }
            }
        }
    }
}

enum CrudFailure {
    Diagnostic(SdkExecutionDiagnostic),
    Panic,
}

fn return_failure(
    failure: CrudFailure,
    out_diagnostics: *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus {
    match failure {
        CrudFailure::Diagnostic(diagnostic) => return_execution_error(diagnostic, out_diagnostics),
        CrudFailure::Panic => TypeBridgeStatus::Panic,
    }
}

fn store_thing(
    package: &Arc<SchemaPackageState>,
    thing: ProjectedThing,
    reservation: ReservedBox<TypeBridgeProjectedThing>,
    out_thing: *mut *mut TypeBridgeProjectedThing,
) {
    let thing = reservation.initialize(TypeBridgeProjectedThing {
        package: Arc::clone(package),
        value: Arc::new(thing),
    });
    // SAFETY: every exported caller initializes and retains this writable slot.
    unsafe { out_thing.write_unaligned(Box::into_raw(thing)) };
}

pub(crate) unsafe fn run_write_thing_call(
    kind: CrudKind,
    target: WriteTarget<'_>,
    prepared: PreparedThingOutputs,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    iid: Option<TypeBridgeByteView>,
    cancellation: *const TypeBridgeCancellation,
) -> TypeBridgeStatus {
    let PreparedThingOutputs {
        thing: out_thing,
        diagnostics: out_diagnostics,
    } = prepared;
    guarded(|| {
        // SAFETY: generated token storage is caller-readable for this call.
        let expected_model = match unsafe { resolve_model(kind, target.package_state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if create.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable create handle during this call.
        let create = unsafe { &*create };
        if let Err(diagnostic) =
            validate_create(kind, target.package_state(), &expected_model, create)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        let copied_iid = match iid {
            Some(iid) => {
                // SAFETY: the bounded byte view remains caller-readable until copied.
                match unsafe { copied_iid(kind, iid) } {
                    Ok(value) => Some(value),
                    Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
                }
            }
            None => None,
        };
        if let Err(diagnostic) = check_cancellation(cancellation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if let Err(failure) = target.preflight_thing_mutation(kind, create) {
            return return_failure(failure, out_diagnostics);
        }
        let reservation = match ReservedBox::try_new(AllocationSite::ProjectedThingHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        let operation = copied_iid
            .as_deref()
            .map_or(ThingMutation::Insert, ThingMutation::Update);
        let result = target.run_thing_mutation(kind, operation, &expected_model, create);
        match result {
            Ok(thing) => {
                store_thing(target.package_state(), thing, reservation, out_thing);
                TypeBridgeStatus::Ok
            }
            Err(failure) => return_failure(failure, out_diagnostics),
        }
    })
}

pub(crate) unsafe fn run_put_call(
    kind: CrudKind,
    target: WriteTarget<'_>,
    prepared: PreparedThingOutputs,
    model: *const TypeBridgeProjectedTokenV1,
    create: *const TypeBridgeProjectedCreate,
    cancellation: *const TypeBridgeCancellation,
) -> TypeBridgeStatus {
    let PreparedThingOutputs {
        thing: out_thing,
        diagnostics: out_diagnostics,
    } = prepared;
    guarded(|| {
        // SAFETY: generated token storage is caller-readable for this call.
        let expected_model = match unsafe { resolve_model(kind, target.package_state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if create.is_null() {
            return TypeBridgeStatus::InvalidArgument;
        }
        // SAFETY: caller retains the immutable create handle during this call.
        let create = unsafe { &*create };
        if let Err(diagnostic) =
            validate_create(kind, target.package_state(), &expected_model, create)
        {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if let Err(diagnostic) = check_cancellation(cancellation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if let Err(failure) = target.preflight_thing_mutation(kind, create) {
            return return_failure(failure, out_diagnostics);
        }
        let reservation = match ReservedBox::try_new(AllocationSite::ProjectedThingHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        match target.run_thing_mutation(kind, ThingMutation::Put, &expected_model, create) {
            Ok(thing) => {
                store_thing(target.package_state(), thing, reservation, out_thing);
                TypeBridgeStatus::Ok
            }
            Err(failure) => return_failure(failure, out_diagnostics),
        }
    })
}

pub(crate) unsafe fn run_get_call(
    kind: CrudKind,
    target: ReadTarget<'_>,
    prepared: PreparedThingOutputs,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
) -> TypeBridgeStatus {
    let PreparedThingOutputs {
        thing: out_thing,
        diagnostics: out_diagnostics,
    } = prepared;
    guarded(|| {
        // SAFETY: generated token storage is caller-readable for this call.
        let type_id = match unsafe { resolve_model(kind, target.package_state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: the bounded byte view remains caller-readable until copied.
        let iid = match unsafe { copied_iid(kind, iid) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if let Err(diagnostic) = check_cancellation(cancellation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        if let Err(failure) = target.preflight_get(kind) {
            return return_failure(failure, out_diagnostics);
        }
        let reservation = match ReservedBox::try_new(AllocationSite::ProjectedThingHandle) {
            Ok(value) => value,
            Err(_) => return return_execution_error(allocation_exhausted(), out_diagnostics),
        };
        match target.run_get(kind, &type_id, &iid) {
            Ok(Some(thing)) => {
                store_thing(target.package_state(), thing, reservation, out_thing);
                TypeBridgeStatus::Ok
            }
            Ok(None) => TypeBridgeStatus::Ok,
            Err(failure) => return_failure(failure, out_diagnostics),
        }
    })
}

pub(crate) unsafe fn run_delete_call(
    kind: CrudKind,
    target: WriteTarget<'_>,
    prepared: PreparedDiagnosticsOutput,
    model: *const TypeBridgeProjectedTokenV1,
    iid: TypeBridgeByteView,
    cancellation: *const TypeBridgeCancellation,
) -> TypeBridgeStatus {
    let out_diagnostics = prepared.diagnostics;
    guarded(|| {
        // SAFETY: generated token storage is caller-readable for this call.
        let type_id = match unsafe { resolve_model(kind, target.package_state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        // SAFETY: the bounded byte view remains caller-readable until copied.
        let iid = match unsafe { copied_iid(kind, iid) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if let Err(diagnostic) = check_cancellation(cancellation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        match target.run_delete(kind, &type_id, &iid) {
            Ok(()) => TypeBridgeStatus::Ok,
            Err(failure) => return_failure(failure, out_diagnostics),
        }
    })
}

pub(crate) unsafe fn run_count_call(
    kind: CrudKind,
    target: ReadTarget<'_>,
    prepared: PreparedCountOutputs,
    model: *const TypeBridgeProjectedTokenV1,
    cancellation: *const TypeBridgeCancellation,
) -> TypeBridgeStatus {
    let PreparedCountOutputs {
        count: out_count,
        diagnostics: out_diagnostics,
    } = prepared;
    guarded(|| {
        // SAFETY: generated token storage is caller-readable for this call.
        let type_id = match unsafe { resolve_model(kind, target.package_state(), model) } {
            Ok(value) => value,
            Err(diagnostic) => return return_execution_error(diagnostic, out_diagnostics),
        };
        if let Err(diagnostic) = check_cancellation(cancellation) {
            return return_execution_error(diagnostic, out_diagnostics);
        }
        match target.run_count(kind, &type_id) {
            Ok(count) => {
                // SAFETY: the output was initialized and remains caller-writable.
                unsafe { out_count.write(count) };
                TypeBridgeStatus::Ok
            }
            Err(failure) => return_failure(failure, out_diagnostics),
        }
    })
}
