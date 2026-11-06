//! C callback ownership and marshalling onto the engine's borrowed transaction.
use crate::abi::TypeBridgeStatus;
use crate::execution_diagnostic::TypeBridgeExecutionDiagnostics;
use crate::runtime::{TypeBridgeDatabase, TypeBridgeWriteTransaction};
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Arc, Condvar, Mutex};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::migration::MigrationStepId;
use type_bridge_schema_migration_typedb::MigrationUserSteps;

/// Synchronous callback. The context outlives the catalog and all derived plans.
pub type TypeBridgeMigrationCallback = unsafe extern "C" fn(
    *mut TypeBridgeWriteTransaction,
    *mut c_void,
    *mut *mut TypeBridgeExecutionDiagnostics,
) -> TypeBridgeStatus;

#[derive(Clone, Copy, Debug)]
struct Callback {
    function: TypeBridgeMigrationCallback,
    context: usize,
}

#[derive(Clone, Default)]
pub(crate) struct UserSteps {
    callbacks: BTreeMap<MigrationStepId, (Callback, Option<Callback>)>,
    // Stable addresses let escaped handles fail as closed until the catalog/plans close.
    #[allow(clippy::vec_box)]
    handles: Arc<Mutex<Vec<Box<TypeBridgeWriteTransaction>>>>,
}
impl std::fmt::Debug for UserSteps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserSteps")
            .field("callbacks", &self.callbacks.keys())
            .finish_non_exhaustive()
    }
}
// Cancellation closes the borrowed transaction, but cannot interrupt a C function.
// Keep the synchronous execute call alive until every callback has returned, so
// the caller can release its context immediately after execute/catalog teardown.
#[derive(Default)]
struct CallbackCompletion {
    active: Mutex<usize>,
    finished: Condvar,
}
struct ActiveCallback(Arc<CallbackCompletion>);
impl Drop for ActiveCallback {
    fn drop(&mut self) {
        let mut active = self.0.active.lock().unwrap_or_else(|e| e.into_inner());
        *active -= 1;
        self.0.finished.notify_all();
    }
}
pub(crate) struct BoundUserSteps {
    pub(crate) steps: MigrationUserSteps,
    completion: Option<Arc<CallbackCompletion>>,
}
impl Drop for BoundUserSteps {
    fn drop(&mut self) {
        if let Some(completion) = &self.completion {
            let active = completion.active.lock().unwrap_or_else(|e| e.into_inner());
            drop(
                completion
                    .finished
                    .wait_while(active, |active| *active != 0)
                    .unwrap_or_else(|e| e.into_inner()),
            );
        }
    }
}
impl UserSteps {
    pub(crate) fn register(
        &mut self,
        id: &str,
        forward: TypeBridgeMigrationCallback,
        reverse: Option<TypeBridgeMigrationCallback>,
        context: usize,
    ) -> Result<(), Diagnostic> {
        let id = MigrationStepId::new(id)?;
        if self.callbacks.contains_key(&id) {
            return Err(crate::diagnostic::stable(
                DiagnosticCategory::InvalidContract,
                "migration_user_step_duplicate",
                "A callback is already registered for this step",
            ));
        }
        self.callbacks.insert(
            id,
            (
                Callback {
                    function: forward,
                    context,
                },
                reverse.map(|function| Callback { function, context }),
            ),
        );
        Ok(())
    }
    pub(crate) fn bind(&self, database: &TypeBridgeDatabase) -> Result<BoundUserSteps, Diagnostic> {
        if self.callbacks.is_empty() {
            return Ok(BoundUserSteps {
                steps: Default::default(),
                completion: None,
            });
        }
        let completion = Arc::new(CallbackCompletion::default());
        let database = Arc::new(database.clone_for_migration());
        let bind = |callback: Callback| {
            let database = Arc::clone(&database);
            let handles = Arc::clone(&self.handles);
            let completion = Arc::clone(&completion);
            move |tx| {
                let database = Arc::clone(&database);
                let handles = Arc::clone(&handles);
                let completion = Arc::clone(&completion);
                async move {
                    *completion.active.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                    let active = ActiveCallback(completion);
                    tokio::task::spawn_blocking(move || {
                        let _active = active;
                        let handle = TypeBridgeWriteTransaction::from_borrowed(&database, tx)
                            .map_err(|_| failed())?;
                        let mut handle = crate::allocation::try_box(
                            crate::allocation::AllocationSite::WriteTransactionHandle,
                            handle,
                        )
                        .map_err(|_| failed())?;
                        let pointer = (&mut *handle) as *mut TypeBridgeWriteTransaction;
                        handles
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(handle);
                        let mut diagnostics = std::ptr::null_mut();
                        // SAFETY: registration requires a live callback/context; the retained handle
                        // stays allocated for the catalog and every derived plan's lifetime.
                        let status = unsafe {
                            (callback.function)(
                                pointer,
                                callback.context as *mut c_void,
                                &mut diagnostics,
                            )
                        };
                        let has_error = !diagnostics.is_null();
                        let cause = if has_error {
                            // SAFETY: the callback transfers an owned execution diagnostics handle.
                            let diagnostics = unsafe { Box::from_raw(diagnostics) };
                            diagnostics.callback_cause()
                        } else {
                            format!("callback returned {status:?}")
                        };
                        if status == TypeBridgeStatus::Ok && !has_error {
                            Ok(())
                        } else {
                            Err(failed().with_detail("cause", cause))
                        }
                    })
                    .await
                    .map_err(|_| failed())?
                }
            }
        };
        let mut result = MigrationUserSteps::default();
        for (id, (forward, reverse)) in &self.callbacks {
            match reverse {
                Some(reverse) => {
                    result.register_with_reverse(id.clone(), bind(*forward), bind(*reverse))
                }
                None => result.register(id.clone(), bind(*forward)),
            }?;
        }
        Ok(BoundUserSteps {
            steps: result,
            completion: Some(completion),
        })
    }
}
fn failed() -> Diagnostic {
    crate::diagnostic::stable(
        DiagnosticCategory::InvalidContract,
        "migration_user_step_failed",
        "C migration callback failed; its transaction will be rolled back",
    )
}
