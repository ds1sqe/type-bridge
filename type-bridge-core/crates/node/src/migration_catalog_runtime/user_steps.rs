//! Async JavaScript callbacks on engine-owned migration transactions.
use crate::NodeRustTransactionContext;
use napi::bindgen_prelude::{Function, Promise};
use napi::threadsafe_function::ThreadsafeFunction;
use std::collections::BTreeMap;
use std::sync::Arc;
use type_bridge_contract::migration::MigrationStepId;
use type_bridge_orm::ProviderRuntimeOwner;
use type_bridge_schema_migration_typedb::MigrationUserSteps;

type Callback = ThreadsafeFunction<
    NodeRustTransactionContext,
    Promise<()>,
    NodeRustTransactionContext,
    napi::Status,
    false,
    true,
>;
#[derive(Clone, Default)]
pub(super) struct UserSteps(BTreeMap<MigrationStepId, (Arc<Callback>, Option<Arc<Callback>>)>);
impl UserSteps {
    pub(super) fn register(
        &mut self,
        id: &str,
        forward: Function<'_, NodeRustTransactionContext, Promise<()>>,
        reverse: Option<Function<'_, NodeRustTransactionContext, Promise<()>>>,
    ) -> napi::Result<()> {
        let id = MigrationStepId::new(id)
            .map_err(|error| super::catalog_error(error.code().as_str()))?;
        if self.0.contains_key(&id) {
            return Err(super::catalog_error("migration_user_step_duplicate"));
        }
        let build = |callback: Function<'_, NodeRustTransactionContext, Promise<()>>| {
            callback
                .build_threadsafe_function::<NodeRustTransactionContext>()
                .callee_handled::<false>()
                .weak::<true>()
                .build()
                .map(Arc::new)
        };
        self.0
            .insert(id, (build(forward)?, reverse.map(build).transpose()?));
        Ok(())
    }
    pub(super) fn bind(
        &self,
        runtime: &Arc<ProviderRuntimeOwner>,
    ) -> napi::Result<MigrationUserSteps> {
        if self.0.is_empty() {
            return Ok(Default::default());
        }
        let bind = |callback: &Arc<Callback>| {
            let callback = Arc::clone(callback);
            let runtime = Arc::clone(runtime);
            move |tx| {
                let callback = Arc::clone(&callback);
                let runtime = Arc::clone(&runtime);
                async move {
                    let context = NodeRustTransactionContext::from_borrowed(tx, runtime);
                    callback
                        .call_async_catch(context)
                        .await
                        .map_err(failed)?
                        .await
                        .map_err(failed)
                }
            }
        };
        let mut result = MigrationUserSteps::default();
        for (id, (forward, reverse)) in &self.0 {
            match reverse {
                Some(reverse) => {
                    result.register_with_reverse(id.clone(), bind(forward), bind(reverse))
                }
                None => result.register(id.clone(), bind(forward)),
            }
            .map_err(|error| super::catalog_error(error.code().as_str()))?;
        }
        Ok(result)
    }
}
fn failed(error: impl std::fmt::Display) -> type_bridge_contract::diagnostic::Diagnostic {
    let cause = error.to_string();
    use type_bridge_contract::diagnostic::*;
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_user_step_failed").expect("static diagnostic"),
        format!("Node migration callback failed; its transaction will be rolled back: {cause}"),
    )
    .with_detail("cause", cause)
}
