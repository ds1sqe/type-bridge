//! Successor-runtime adapters for binding-neutral projected mutation batches.

use type_bridge_contract::id::TypeId;
use type_bridge_contract::projection::ProjectionHandler;
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

pub(crate) fn uses_successor_batch_runtime(installed: &InstalledRuntimeProjection) -> bool {
    installed.projection().generator_handlers() == [ProjectionHandler::rust_v2()]
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
        .execute_mapped(database, batch, control(), Vec::new(), |result| {
            materialize_result(result, installed)
        })
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_borrowed_things<M: CompleteModel>(
    transaction: &TransactionContext,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<Vec<M>> {
    ProjectedBatchExecutor::new(installed)
        .execute_in_transaction_mapped(transaction, batch, control(), Vec::new(), |result| {
            materialize_result(result, installed)
        })
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_owned_delete(
    database: &Database,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<()> {
    ProjectedBatchExecutor::new(installed)
        .execute_mapped(database, batch, control(), (), deleted_result)
        .await
        .map_err(map_execution_error)
}

pub(crate) async fn execute_borrowed_delete(
    transaction: &TransactionContext,
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
) -> Result<()> {
    ProjectedBatchExecutor::new(installed)
        .execute_in_transaction_mapped(transaction, batch, control(), (), deleted_result)
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

fn control() -> ProjectedBatchInvocationControl {
    ProjectedBatchInvocationControl::capture(
        QueryExecutionResourceLimits::default(),
        AnswerCancellation::default(),
    )
}

fn map_execution_error(error: SdkExecutionDiagnostic) -> Error {
    let phase = if error.category() == SdkDiagnosticCategory::Integrity {
        ModelValidationPhase::Hydration
    } else {
        ModelValidationPhase::Input
    };
    Error::from_projected_batch(error, phase)
}
