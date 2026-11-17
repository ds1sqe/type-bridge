//! Exact managed-fence read for migration callbacks.
//!
//! The released 2.x writer guard that inspected cutover rows is removed in
//! 3.0; schema changes go only through Split-YAML and reviewed migrations.

use type_bridge_contract::reserved::{
    MANAGED_CONTROL_ENTITY, MANAGED_CONTROL_LEASE_FENCE, MANAGED_CONTROL_LEASE_HELD,
    MANAGED_CONTROL_LEASE_HOLDER, MANAGED_CONTROL_LEASE_STATE, MANAGED_CONTROL_SCOPE,
};

mod data_plan;
use crate::error::{OrmError, Result};

// Every fence read has a fixed schema and predicates; validate its plan once.
macro_rules! guard_read {
    ($kind:expr, $fixed:expr, $bound:expr $(,)?) => {{
        static QUERY: std::sync::LazyLock<
            std::result::Result<data_plan::DataQuery, type_bridge_contract::diagnostic::Diagnostic>,
        > = std::sync::LazyLock::new(|| data_plan::read($kind, $fixed, $bound));
        QUERY
            .as_ref()
            .map_err(|error| crate::query_plan_exec::plan_diagnostic_error(error.clone()))?
    }};
}

pub(crate) async fn require_migration_callback_fence(
    transaction: &mut super::OwnedTransaction,
    scope: &type_bridge_contract::managed_scope::ManagedScopeId,
    fence: u64,
    holder: &str,
    cancellation: &super::backend::AnswerCancellation,
    deadline: crate::QueryExecutionDeadline,
) -> Result<()> {
    let query = guard_read!(
        MANAGED_CONTROL_ENTITY,
        &[],
        &[
            (MANAGED_CONTROL_SCOPE, "scope"),
            (MANAGED_CONTROL_LEASE_FENCE, "fence"),
            (MANAGED_CONTROL_LEASE_STATE, "state"),
            (MANAGED_CONTROL_LEASE_HOLDER, "holder"),
        ]
    );
    let answers = transaction
        .execute_plans(
            std::slice::from_ref(&query.plan),
            &crate::QueryExecutionResourceLimits::default(),
            cancellation,
            deadline,
        )
        .await?;
    let answer = answers
        .into_iter()
        .next()
        .ok_or_else(|| integrity_error("migration fence read returned no answer"))?;
    let rows = query
        .documents(answer)
        .map_err(crate::query_plan_exec::plan_diagnostic_error)?;
    if fence == 0
        || rows
            != [serde_json::json!({
                "scope": scope.as_str(), "fence": fence.to_string(),
                "state": MANAGED_CONTROL_LEASE_HELD, "holder": holder,
            })]
    {
        return Err(integrity_error(
            "migration callback requires its exact active held fence",
        ));
    }
    Ok(())
}

fn integrity_error(message: impl AsRef<str>) -> OrmError {
    OrmError::Transaction(format!(
        "legacy migration cutover state is inconsistent: {}",
        message.as_ref()
    ))
}
