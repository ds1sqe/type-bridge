//! One managed transaction per data program and atomic completion marker.
use super::*;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::migration_data::{DataProgram, DataStep};
use type_bridge_contract::reserved::*;
use type_bridge_schema_migration::{
    DataStepDirection, MigrationExecutionControl, VerifiedSchemaMigrationManifest,
    await_interruptible_operation,
};

pub(crate) fn validate(
    step: &DataStep,
    direction: DataStepDirection,
    users: &crate::MigrationUserSteps,
) -> Result<(), Diagnostic> {
    let program = program(step, direction)?;
    if let DataProgram::User { step_id } = program {
        users.callback(step_id, direction)?;
    }
    Ok(())
}
fn program(step: &DataStep, direction: DataStepDirection) -> Result<&DataProgram, Diagnostic> {
    match direction {
        DataStepDirection::Forward => Ok(step.forward()),
        DataStepDirection::Reverse => step.reverse().ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_rollback_irreversible_data",
                "data step has no reverse program",
            )
        }),
    }
}
struct Marker {
    key: String,
    scope: String,
    migration: String,
    step: String,
}
impl Marker {
    fn new(
        lease: &MigrationLease,
        manifest: &VerifiedSchemaMigrationManifest,
        step: &DataStep,
    ) -> Result<Self, Diagnostic> {
        let scope = lease.scope().managed_scope_id().as_str().to_owned();
        let key = String::from_utf8(to_canonical_json(&[
            scope.as_str(),
            manifest.id().app_label().as_str(),
            manifest.id().name().as_str(),
            step.id().as_str(),
        ])?)
        .expect("canonical UTF-8");
        let migration =
            String::from_utf8(to_canonical_json(manifest.id())?).expect("canonical UTF-8");
        Ok(Self {
            key,
            scope,
            migration,
            step: step.id().as_str().to_owned(),
        })
    }
    fn fields(&self) -> [(&str, &str); 4] {
        [
            (DATA_STEP_KEY, &self.key),
            (DATA_STEP_SCOPE, &self.scope),
            (DATA_STEP_MIGRATION, &self.migration),
            (DATA_STEP_ID, &self.step),
        ]
    }
    async fn present(&self, transaction: &mut Transaction) -> Result<bool, Diagnostic> {
        let query = crate::data_plan::read_data_marker(&self.key)?;
        let rows = crate::runner::legacy_anchor_plan_values(transaction, query).await?;
        match rows.as_slice() {
            [] => Ok(false),
            [row]
                if row.get("scope").and_then(serde_json::Value::as_str)
                    == Some(self.scope.as_str())
                    && row.get("migration").and_then(serde_json::Value::as_str)
                        == Some(self.migration.as_str())
                    && row.get("step").and_then(serde_json::Value::as_str)
                        == Some(self.step.as_str()) =>
            {
                Ok(true)
            }
            _ => Err(failure(
                DiagnosticCategory::Integrity,
                "migration_data_marker_identity",
                "completion marker has a foreign or malformed compound identity",
            )),
        }
    }
}
fn state<'a>(
    manifest: &'a VerifiedSchemaMigrationManifest,
    step: &DataStep,
) -> Result<&'a ManagedSchemaState, Diagnostic> {
    let mut current = manifest.source_state();
    for candidate in manifest.steps() {
        if candidate.as_data() == Some(step) {
            return Ok(current);
        }
        if let Some(delta) = candidate.as_schema_delta() {
            current = delta.delta().target();
        }
    }
    Err(failure(
        DiagnosticCategory::Integrity,
        "migration_data_step_identity",
        "data step is absent from its verified manifest",
    ))
}
impl TypeDbMigrationProvider {
    pub(super) async fn observe_data(
        &self,
        lease: &MigrationLease,
        manifest: &VerifiedSchemaMigrationManifest,
        step: &DataStep,
        direction: DataStepDirection,
    ) -> Result<bool, Diagnostic> {
        self.binding.require_lease(lease)?;
        let marker = Marker::new(lease, manifest, step)?;
        let mut tx = self
            .binding
            .managed_database
            .read_transaction()
            .await
            .map_err(map_orm_error)?;
        let result: Result<bool, Diagnostic> = async {
            require_active_managed_fence(&mut tx, lease).await?;
            let present = marker.present(&mut tx).await?;
            Ok(present == (direction == DataStepDirection::Forward))
        }
        .await;
        let close = tx.close().await;
        let result = result?;
        close.map_err(map_orm_error)?;
        Ok(result)
    }
    pub(super) async fn execute_data(
        &self,
        lease: &MigrationLease,
        manifest: &VerifiedSchemaMigrationManifest,
        step: &DataStep,
        direction: DataStepDirection,
        control: &MigrationExecutionControl,
    ) -> Result<(), GroupCommitFailure> {
        validate(step, direction, &self.user_steps).map_err(definitely_aborted)?;
        self.binding
            .require_lease(lease)
            .map_err(definitely_aborted)?;
        control.check().map_err(definitely_aborted)?;
        let marker = Marker::new(lease, manifest, step).map_err(definitely_aborted)?;
        let mut tx = self
            .binding
            .managed_database
            .write_transaction()
            .await
            .map_err(map_orm_error)
            .map_err(definitely_aborted)?;
        let mutation = await_interruptible_operation(control, async {
            let current = state(manifest, step)?;
            self.observe_in_transaction(&mut tx, lease, current, current)
                .await?;
            let fence = lease.fence().get().to_string();
            let claim = crate::data_plan::claim_data_fence(&[
                (
                    MANAGED_CONTROL_SCOPE,
                    lease.scope().managed_scope_id().as_str(),
                ),
                (MANAGED_CONTROL_LEASE_HOLDER, lease.holder().as_str()),
                (MANAGED_CONTROL_LEASE_FENCE, &fence),
                (MANAGED_CONTROL_LEASE_STATE, MANAGED_CONTROL_LEASE_HELD),
            ])?;
            if matched_count(&mut tx, &claim).await? != 1 {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_execution_stale_fence",
                    "data transaction lost its managed fence",
                ));
            }
            let forward = direction == DataStepDirection::Forward;
            if marker.present(&mut tx).await? == forward {
                return Ok(());
            }
            match program(step, direction)? {
                DataProgram::Plan { plan } => {
                    let limits = type_bridge_orm::QueryExecutionResourceLimits::default();
                    tx.execute_plans(
                        std::slice::from_ref(plan.as_ref()),
                        &limits,
                        &type_bridge_orm::AnswerCancellation::default(),
                        type_bridge_orm::QueryExecutionDeadline::for_limits(limits),
                    )
                    .await
                    .map_err(map_orm_error)?;
                }
                DataProgram::Typeql { text } => {
                    tx.query(text).await.map_err(map_orm_error)?;
                }
                DataProgram::User { step_id } => {
                    let borrowed = tx.borrow_context(&self.binding.managed_database);
                    let result = self
                        .user_steps
                        .execute(step_id, direction, borrowed.context().clone())
                        .await;
                    let restored = borrowed.finish().await.map_err(map_orm_error);
                    result?;
                    restored?;
                }
            }
            control.check()?;
            let write = crate::data_plan::write_data_marker(&marker.fields(), forward)?;
            if matched_count(&mut tx, &write).await? != 1 {
                return Err(failure(
                    DiagnosticCategory::Integrity,
                    "migration_data_marker_write",
                    "data marker mutation did not affect exactly one marker",
                ));
            }
            Ok(())
        })
        .await;
        if let Err(error) = mutation {
            let error = finish_provider_data_guard::<()>(&mut tx, Err(error), "atomic data step")
                .await
                .expect_err("preserve primary error");
            return Err(definitely_aborted(error));
        }
        if let Err(error) = control.check() {
            let error =
                finish_provider_data_guard::<()>(&mut tx, Err(error), "cancelled data step")
                    .await
                    .expect_err("preserve cancellation");
            return Err(definitely_aborted(error));
        }
        tx.commit_classified().await.map_err(|error| {
            GroupCommitFailure::new(
                group_commit_certainty(&error),
                map_orm_error(error.into_orm_error()),
            )
        })
    }
}

async fn matched_count(tx: &mut Transaction, plan: &QueryPlan) -> Result<u64, Diagnostic> {
    let limits = type_bridge_orm::QueryExecutionResourceLimits::default();
    let answers = tx
        .execute_plans(
            std::slice::from_ref(plan),
            &limits,
            &type_bridge_orm::AnswerCancellation::default(),
            type_bridge_orm::QueryExecutionDeadline::for_limits(limits),
        )
        .await
        .map_err(map_orm_error)?;
    match answers.as_slice() {
        [PlanAnswer::Matched(count)] => Ok(*count),
        _ => Err(failure(
            DiagnosticCategory::Integrity,
            "migration_data_marker_answer",
            "data marker program must return one matched-count answer",
        )),
    }
}
