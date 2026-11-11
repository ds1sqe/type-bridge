//! Process-local user functions; manifest digests contain only their identifiers.
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::migration::MigrationStepId;
use type_bridge_orm::session::context::TransactionContext;
use type_bridge_schema_migration::{DataStepDirection, ExecutionFuture};

use crate::provider::failure;

type Callback = Arc<dyn Fn(TransactionContext) -> ExecutionFuture<'static, ()> + Send + Sync>;
#[derive(Clone)]
struct RegisteredStep {
    forward: Callback,
    reverse: Option<Callback>,
}

/// Functions registered by one SDK catalog. Clones retain the same functions.
///
/// Contexts borrow execution only. Every clone becomes unusable when the
/// callback finishes; commit, rollback and close always belong to the engine.
#[derive(Clone, Default)]
pub struct MigrationUserSteps {
    steps: BTreeMap<MigrationStepId, RegisteredStep>,
}
impl std::fmt::Debug for MigrationUserSteps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationUserSteps")
            .field("ids", &self.steps.keys())
            .finish()
    }
}
impl MigrationUserSteps {
    /// Register a forward-only plain async function or closure.
    pub fn register<F, Fut>(&mut self, id: MigrationStepId, forward: F) -> Result<(), Diagnostic>
    where
        F: Fn(TransactionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), Diagnostic>> + Send + 'static,
    {
        self.insert(id, Arc::new(move |tx| Box::pin(forward(tx))), None)
    }

    /// Register explicit forward and reverse functions for one user identifier.
    pub fn register_with_reverse<F, Fut, R, Rev>(
        &mut self,
        id: MigrationStepId,
        forward: F,
        reverse: R,
    ) -> Result<(), Diagnostic>
    where
        F: Fn(TransactionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), Diagnostic>> + Send + 'static,
        R: Fn(TransactionContext) -> Rev + Send + Sync + 'static,
        Rev: Future<Output = Result<(), Diagnostic>> + Send + 'static,
    {
        self.insert(
            id,
            Arc::new(move |tx| Box::pin(forward(tx))),
            Some(Arc::new(move |tx| Box::pin(reverse(tx)))),
        )
    }

    fn insert(
        &mut self,
        id: MigrationStepId,
        forward: Callback,
        reverse: Option<Callback>,
    ) -> Result<(), Diagnostic> {
        if self.steps.contains_key(&id) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "migration_user_step_duplicate",
                "user step is already registered",
            )
            .with_detail("step_id", id.as_str().to_owned()));
        }
        self.steps.insert(id, RegisteredStep { forward, reverse });
        Ok(())
    }

    pub(crate) async fn execute(
        &self,
        id: &MigrationStepId,
        direction: DataStepDirection,
        tx: TransactionContext,
    ) -> Result<(), Diagnostic> {
        let callback = self.callback(id, direction)?;
        let mut future = Box::pin(async move { callback(tx).await });
        std::future::poll_fn(|cx| {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                future.as_mut().poll(cx)
            })) {
                Ok(value) => value,
                Err(_) => std::task::Poll::Ready(Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_user_step_panicked",
                    "user migration callback panicked; its transaction will be rolled back",
                ))),
            }
        })
        .await
    }

    pub(crate) fn callback(
        &self,
        id: &MigrationStepId,
        direction: DataStepDirection,
    ) -> Result<&Callback, Diagnostic> {
        self.steps.get(id).and_then(|step| match direction {
            DataStepDirection::Forward => Some(&step.forward),
            DataStepDirection::Reverse => step.reverse.as_ref(),
        }).ok_or_else(|| failure(DiagnosticCategory::InvalidContract,
            "migration_user_step_unregistered", "user data step requires a registered callback in an SDK migration catalog; CLI apply cannot execute user code")
            .with_detail("step_id", id.as_str().to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_is_unique_and_reverse_is_explicit() {
        let mut steps = MigrationUserSteps::default();
        let id = MigrationStepId::new("derive").unwrap();
        steps.register(id.clone(), |_| async { Ok(()) }).unwrap();
        assert!(steps.callback(&id, DataStepDirection::Forward).is_ok());
        assert_eq!(
            steps
                .callback(&id, DataStepDirection::Reverse)
                .err()
                .unwrap()
                .code()
                .as_str(),
            "migration_user_step_unregistered"
        );
        assert_eq!(
            steps
                .register(id.clone(), |_| async { Ok(()) })
                .unwrap_err()
                .code()
                .as_str(),
            "migration_user_step_duplicate"
        );
        let reverse = MigrationStepId::new("reversible").unwrap();
        steps
            .register_with_reverse(reverse.clone(), |_| async { Ok(()) }, |_| async { Ok(()) })
            .unwrap();
        assert!(steps.callback(&reverse, DataStepDirection::Reverse).is_ok());
        assert!(
            steps
                .clone()
                .callback(&reverse, DataStepDirection::Reverse)
                .is_ok()
        );
    }
}
