//! V3 adapters for binding-neutral projected mutation batches.

use type_bridge_contract::id::TypeId;
use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticCategory, SdkExecutionDiagnostic};
use type_bridge_orm::session::context::TransactionContext;
use type_bridge_orm::{
    AnswerCancellation, Database, InstalledRuntimeProjection, ProjectedBatch,
    ProjectedBatchExecutor, ProjectedBatchInvocationControl, ProjectedBatchOperation,
    ProjectedBatchResult, ProjectedBatchRow, QueryExecutionResourceLimits,
};

use crate::__codegen::{CompleteModel, EncodedCreate, IntoEncodedCreate};
use crate::Result;
use crate::entity_codec::map_validation_error;
use crate::error::{Error, ModelValidationPhase};
use crate::projected_codec::{materialize_projected, project_create, project_encoded_create};

#[cfg(test)]
mod tests;

pub(crate) async fn execute_transaction_single<M: CompleteModel>(
    transaction: &TransactionContext,
    installed: &InstalledRuntimeProjection,
    create: type_bridge_orm::ProjectedCreate,
    operation: crate::hooks::CrudOperation,
    iid: Option<&str>,
    kind: crate::hooks::ModelKind,
) -> Result<crate::WriteResult<M>> {
    use crate::hooks::CrudOperation;

    let model = create.type_id().clone();
    let (batch_operation, row) = match operation {
        CrudOperation::Insert => (
            ProjectedBatchOperation::Insert,
            ProjectedBatchRow::Create(create),
        ),
        CrudOperation::Put => (
            ProjectedBatchOperation::Put,
            ProjectedBatchRow::Create(create),
        ),
        CrudOperation::Update => (
            ProjectedBatchOperation::Update,
            ProjectedBatchRow::Update {
                iid: iid
                    .expect("single update requires its validated IID")
                    .to_owned(),
                replacement: create,
            },
        ),
        CrudOperation::Delete => unreachable!("a create payload cannot select deletion"),
    };
    let control = transaction_control(transaction);
    let executor = type_bridge_orm::ProjectedCrudExecutor::new(installed);
    let batch = executor
        .single_controlled_write_batch(&model, batch_operation, row, &control)
        .map_err(|error| Error::from_sdk_execution(error, ModelValidationPhase::Input))?;
    let binding_error = std::sync::Mutex::new(None);
    let result = executor
        .execute_single_write_in_transaction_with_compatibility(
            transaction,
            &batch,
            control,
            |things| {
                crate::projected_codec::materialize_write_result(things, installed).map_err(
                    |error| {
                        *binding_error
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                        ProjectedBatchExecutor::binding_materialization_failure(0)
                    },
                )
            },
        )
        .await;
    if let Some(error) = binding_error
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
    {
        return Err(error);
    }
    result.map_err(|error| Error::from_projected_crud(error, kind, Some(operation)))
}

pub(crate) fn create_operation(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    put: bool,
) -> ProjectedBatchOperation {
    if put
        && !installed
            .projection()
            .models()
            .get(model)
            .expect("validated manager model")
            .reference_read()
            .key_fields()
            .is_empty()
    {
        ProjectedBatchOperation::Put
    } else {
        ProjectedBatchOperation::Insert
    }
}

pub(crate) fn validate_binding_row_count(row_count: usize) -> Result<()> {
    ProjectedBatch::validate_binding_row_count(row_count)
        .map_err(|error| Error::from_projected_batch(error, ModelValidationPhase::Input))
}

pub(crate) fn checked_batch_ordinal(ordinal: usize) -> u64 {
    u64::try_from(ordinal).expect("projected batch ordinal is bounded below u64::MAX")
}

pub(crate) fn reserved_binding_vec<T>(capacity: usize) -> Result<Vec<T>> {
    reserved_binding_vec_inner(capacity, false)
}

pub(crate) fn create_rows<T: IntoEncodedCreate>(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    inputs: Vec<T>,
) -> Result<Vec<ProjectedBatchRow>> {
    let mut rows = reserved_binding_vec(inputs.len())?;
    for (ordinal, input) in inputs.into_iter().enumerate() {
        rows.push(ProjectedBatchRow::Create(project_batch_create(
            input, model, installed, ordinal,
        )?));
    }
    Ok(rows)
}

pub(crate) fn encode_batch_create<T: IntoEncodedCreate>(
    input: T,
    ordinal: usize,
) -> Result<EncodedCreate> {
    input.into_encoded_create().map_err(|error| {
        map_validation_error(error, ModelValidationPhase::Input)
            .with_projected_batch_row(checked_batch_ordinal(ordinal))
    })
}

pub(crate) fn project_encoded_batch_create(
    input: &EncodedCreate,
    model: &TypeId,
    installed: &InstalledRuntimeProjection,
    ordinal: usize,
) -> Result<type_bridge_orm::ProjectedCreate> {
    project_encoded_create(input, model, installed)
        .map_err(|error| error.with_projected_batch_row(checked_batch_ordinal(ordinal)))
}

pub(crate) fn update_rows<T: IntoEncodedCreate>(
    installed: &InstalledRuntimeProjection,
    model: &TypeId,
    inputs: Vec<(String, T)>,
) -> Result<Vec<ProjectedBatchRow>> {
    let mut rows = reserved_binding_vec(inputs.len())?;
    for (ordinal, (iid, input)) in inputs.into_iter().enumerate() {
        rows.push(ProjectedBatchRow::Update {
            iid,
            replacement: project_batch_create(input, model, installed, ordinal)?,
        });
    }
    Ok(rows)
}

pub(crate) fn project_batch_create<T: IntoEncodedCreate>(
    input: T,
    model: &TypeId,
    installed: &InstalledRuntimeProjection,
    ordinal: usize,
) -> Result<type_bridge_orm::ProjectedCreate> {
    project_create(input, model, installed)
        .map_err(|error| error.with_projected_batch_row(checked_batch_ordinal(ordinal)))
}

pub(crate) fn delete_rows(iids: &[String]) -> Result<Vec<ProjectedBatchRow>> {
    let mut rows = reserved_binding_vec(iids.len())?;
    for iid in iids {
        rows.push(ProjectedBatchRow::Delete { iid: iid.clone() });
    }
    Ok(rows)
}

pub(crate) async fn execute_owned_things<M: CompleteModel>(
    database: &Database,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<Vec<M>> {
    ProjectedBatchExecutor::new(installed)
        .execute_mapped(
            database,
            batch,
            database_control(database),
            Vec::new(),
            |result| materialize_result(result, installed),
        )
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_owned_deleted_things<M: CompleteModel>(
    database: &Database,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<Vec<M>> {
    ProjectedBatchExecutor::new(installed)
        .returning_deleted()
        .execute_mapped(
            database,
            batch,
            database_control(database),
            Vec::new(),
            |result| materialize_result(result, installed),
        )
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_borrowed_things<M: CompleteModel>(
    transaction: &TransactionContext,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<Vec<M>> {
    ProjectedBatchExecutor::new(installed)
        .execute_in_transaction_mapped(
            transaction,
            batch,
            transaction_control(transaction),
            Vec::new(),
            |result| materialize_result(result, installed),
        )
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_borrowed_deleted_things<M: CompleteModel>(
    transaction: &TransactionContext,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<Vec<M>> {
    ProjectedBatchExecutor::new(installed)
        .returning_deleted()
        .execute_in_transaction_mapped(
            transaction,
            batch,
            transaction_control(transaction),
            Vec::new(),
            |result| materialize_result(result, installed),
        )
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_owned_delete(
    database: &Database,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<()> {
    ProjectedBatchExecutor::new(installed)
        .execute_mapped(
            database,
            batch,
            database_control(database),
            (),
            deleted_result,
        )
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_borrowed_delete(
    transaction: &TransactionContext,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<()> {
    ProjectedBatchExecutor::new(installed)
        .execute_in_transaction_mapped(
            transaction,
            batch,
            transaction_control(transaction),
            (),
            deleted_result,
        )
        .await
        .map_err(map_execution_error)
}

pub(crate) fn prepare_batch(
    installed: &InstalledRuntimeProjection,
    model: TypeId,
    operation: ProjectedBatchOperation,
    rows: Vec<ProjectedBatchRow>,
) -> Result<ProjectedBatch> {
    ProjectedBatch::try_new(installed, model, operation, rows)
        .map_err(|error| Error::from_projected_batch(error, ModelValidationPhase::Input))
}

fn materialize_result<M: CompleteModel>(
    result: ProjectedBatchResult,
    installed: &InstalledRuntimeProjection,
) -> std::result::Result<Vec<M>, SdkExecutionDiagnostic> {
    materialize_result_inner(result, installed, false)
}

fn materialize_result_inner<M: CompleteModel>(
    result: ProjectedBatchResult,
    installed: &InstalledRuntimeProjection,
    fail_reservation: bool,
) -> std::result::Result<Vec<M>, SdkExecutionDiagnostic> {
    let ProjectedBatchResult::Things(things) = result else {
        return Err(SdkExecutionDiagnostic::internal_failure());
    };
    let mut output = Vec::new();
    if fail_reservation || output.try_reserve_exact(things.len()).is_err() {
        return Err(ProjectedBatch::binding_allocation_failure());
    }
    for (ordinal, thing) in things.into_iter().enumerate() {
        match materialize_projected(thing, installed) {
            Ok(model) => output.push(model),
            Err(_) => {
                let ordinal = checked_batch_ordinal(ordinal);
                return Err(ProjectedBatchExecutor::binding_materialization_failure(
                    ordinal,
                ));
            }
        }
    }
    Ok(output)
}

fn reserved_binding_vec_inner<T>(capacity: usize, fail_reservation: bool) -> Result<Vec<T>> {
    validate_binding_row_count(capacity)?;
    let mut rows = Vec::new();
    if fail_reservation || rows.try_reserve_exact(capacity).is_err() {
        return Err(Error::from_projected_batch(
            ProjectedBatch::binding_allocation_failure(),
            ModelValidationPhase::Input,
        ));
    }
    Ok(rows)
}

fn deleted_result(result: ProjectedBatchResult) -> std::result::Result<(), SdkExecutionDiagnostic> {
    match result {
        ProjectedBatchResult::Deleted => Ok(()),
        ProjectedBatchResult::Things(_) => Err(SdkExecutionDiagnostic::internal_failure()),
        _ => Err(SdkExecutionDiagnostic::internal_failure()),
    }
}

fn control(limits: QueryExecutionResourceLimits) -> ProjectedBatchInvocationControl {
    ProjectedBatchInvocationControl::capture(limits, AnswerCancellation::default())
}

fn database_control(database: &Database) -> ProjectedBatchInvocationControl {
    control(database.answer_limits().unwrap_or_default())
}

fn transaction_control(transaction: &TransactionContext) -> ProjectedBatchInvocationControl {
    control(transaction.answer_limits().unwrap_or_default())
}

fn map_execution_error(error: SdkExecutionDiagnostic) -> Error {
    let phase = if error.category() == SdkDiagnosticCategory::Integrity {
        ModelValidationPhase::Hydration
    } else {
        ModelValidationPhase::Input
    };
    Error::from_projected_batch(error, phase)
}
