//! Schema-branded migration callbacks over the ordinary eager transaction API.
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration::MigrationStepId;
use type_bridge_orm::TransactionContext;

use crate::schema::Schema;
use crate::{Database, Result};

/// Callback-scoped execution on the engine's managed write transaction.
///
/// Typed managers, typed queries and raw TypeQL share one transaction. The
/// engine commits data and its completion marker together. Retained handles
/// become invalid on callback return; lifecycle methods are not exposed.
pub struct MigrationTransaction<S: Schema> {
    tx: TransactionContext,
    database: Database<S>,
}
impl<S: Schema> MigrationTransaction<S> {
    /// Borrow the ordinary exact entity manager for this generated model.
    pub fn entities<M>(&self) -> crate::transaction::TransactionEntityManager<'_, S, M>
    where
        M: crate::__codegen::EntityModel<Schema = S> + crate::__codegen::CompleteModel,
    {
        crate::transaction::TransactionEntityManager::from_context(&self.tx, &self.database)
    }

    /// Borrow the ordinary exact relation manager for this generated model.
    pub fn relations<M>(&self) -> crate::transaction::TransactionRelationManager<'_, S, M>
    where
        M: crate::__codegen::RelationModel<Schema = S> + crate::__codegen::CompleteModel,
    {
        crate::transaction::TransactionRelationManager::from_context(&self.tx, &self.database)
    }

    /// Start the ordinary typed query session on this same write context.
    pub fn query(&self) -> crate::query::QuerySession<'_, S> {
        crate::query::QuerySession::borrowed(
            Arc::clone(
                self.database
                    .installed_schema()
                    .expect("schema-bound migration"),
            ),
            Arc::clone(
                self.database
                    .match_registry()
                    .expect("schema-bound migration"),
            ),
            &self.tx,
            self.database.operation_limits(Default::default()),
            Default::default(),
        )
    }

    /// Execute raw TypeQL and return the same JSON answer as `Database::execute_query`.
    pub async fn execute_query(
        &self,
        query: &str,
    ) -> std::result::Result<serde_json::Value, crate::raw::RawQueryError> {
        let bytes = self.tx.execute_query(query).await?;
        serde_json::from_slice(&bytes).map_err(|_| crate::raw::RawQueryError {
            code: "raw_answer_encoding".into(),
            message: "raw query returned invalid JSON".into(),
            status: 502,
            provider_code: None,
            commit_outcome: None,
        })
    }
}

type Callback<S> = Arc<
    dyn Fn(MigrationTransaction<S>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;
type Functions<S> = BTreeMap<MigrationStepId, (Callback<S>, Option<Callback<S>>)>;

pub(super) struct UserSteps<S: Schema>(Functions<S>);
impl<S: Schema> Default for UserSteps<S> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}
impl<S: Schema> Clone for UserSteps<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<S: Schema> std::fmt::Debug for UserSteps<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}
impl<S: Schema> UserSteps<S> {
    pub(super) fn insert(
        &mut self,
        id: &str,
        forward: Callback<S>,
        reverse: Option<Callback<S>>,
    ) -> Result<()> {
        let id = MigrationStepId::new(id).map_err(super::migration_diagnostic)?;
        if self.0.contains_key(&id) {
            return Err(super::migration_diagnostic(Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                DiagnosticCode::new("migration_user_step_duplicate").expect("static diagnostic"),
                "A callback is already registered for this step",
            )));
        }
        self.0.insert(id, (forward, reverse));
        Ok(())
    }

    pub(super) fn bind(
        &self,
        database: &Database<S>,
    ) -> Result<type_bridge_schema_migration_typedb::MigrationUserSteps> {
        if self.0.is_empty() {
            return Ok(Default::default());
        }
        let database = Arc::new(database.clone_for_migration());
        let mut result = type_bridge_schema_migration_typedb::MigrationUserSteps::default();
        let bind =
            |callback: &Callback<S>| {
                let callback = Arc::clone(callback);
                let database = Arc::clone(&database);
                move |tx| {
                    let future = callback(MigrationTransaction {
                        tx,
                        database: database.clone_for_migration(),
                    });
                    async move {
                        future.await.map_err(|_| Diagnostic::new(
                    DiagnosticCategory::InvalidContract,
                    DiagnosticCode::new("migration_user_step_failed").expect("static diagnostic"),
                    "user migration callback failed; its transaction will be rolled back"))
                    }
                }
            };
        for (id, (forward, reverse)) in &self.0 {
            match reverse {
                Some(reverse) => {
                    result.register_with_reverse(id.clone(), bind(forward), bind(reverse))
                }
                None => result.register(id.clone(), bind(forward)),
            }
            .map_err(super::migration_diagnostic)?;
        }
        Ok(result)
    }
}
