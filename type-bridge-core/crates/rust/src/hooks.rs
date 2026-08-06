//! Generated-model CRUD lifecycle hooks.
//!
//! Hooks are attached to a schema-bound generated manager. Pre-hooks run in
//! registration order and may reject an operation. Post-hooks run in reverse
//! order after a successful commit; their errors are logged and do not change
//! the committed result.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::Value;

use crate::__codegen::EncodedCreate;
use crate::Result;
use crate::error::Error;

/// Boxed future returned by an object-safe generated lifecycle hook.
pub type HookFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Generated CRUD operation presented to lifecycle hooks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrudOperation {
    /// Insert a new exact model.
    Insert,
    /// Replace an existing exact model.
    Update,
    /// Delete an exact model.
    Delete,
    /// Insert or replace by projected key.
    Put,
}

/// Generated thing kind presented to lifecycle hooks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelKind {
    /// Generated entity model.
    Entity,
    /// Generated relation model.
    Relation,
}

/// Result of a generated pre-operation hook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreHookResult {
    /// Continue the operation.
    Continue,
    /// Reject the operation before database work begins.
    Reject {
        /// Application-owned rejection reason.
        reason: String,
    },
}

/// Failure returned by one generated lifecycle hook.
#[derive(Debug, thiserror::Error)]
pub enum HookError {
    /// A pre-hook explicitly rejected the operation.
    #[error("hook '{hook_name}' rejected {operation:?}: {reason}")]
    Rejected {
        /// Hook name.
        hook_name: String,
        /// Rejected operation.
        operation: CrudOperation,
        /// Application-owned reason.
        reason: String,
    },
    /// A hook failed while executing application code.
    #[error("hook '{hook_name}' failed: {source}")]
    Internal {
        /// Hook name.
        hook_name: String,
        /// Application-owned source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

/// Context shared with one generated lifecycle hook.
///
/// Pre-hooks receive a mutable context and may add metadata for post-hooks.
/// The generated input is present for insert, put, and update. Delete supplies
/// an IID but has no generated create value. The operation timestamp and
/// metadata remain stable across its pre- and post-hook phases.
pub struct HookContext<'a> {
    type_id_json: &'static str,
    type_name: &'a str,
    model_kind: ModelKind,
    operation: CrudOperation,
    iid: Option<&'a str>,
    input: Option<&'a EncodedCreate>,
    timestamp: &'a SystemTime,
    metadata: &'a mut BTreeMap<String, Value>,
}

impl<'a> HookContext<'a> {
    /// Canonical generated type identity JSON.
    #[must_use]
    pub const fn type_id_json(&self) -> &'static str {
        self.type_id_json
    }

    /// Provider type label resolved from the generated identity.
    #[must_use]
    pub const fn type_name(&self) -> &str {
        self.type_name
    }

    /// Whether the model is an entity or relation.
    #[must_use]
    pub const fn model_kind(&self) -> ModelKind {
        self.model_kind
    }

    /// CRUD operation being performed.
    #[must_use]
    pub const fn operation(&self) -> CrudOperation {
        self.operation
    }

    /// Canonical target IID when the operation has one.
    #[must_use]
    pub const fn iid(&self) -> Option<&str> {
        self.iid
    }

    /// Generated create value for insert, put, or update.
    #[must_use]
    pub const fn input(&self) -> Option<&EncodedCreate> {
        self.input
    }

    /// Time at which pre-hook processing for this operation began.
    #[must_use]
    pub const fn timestamp(&self) -> &SystemTime {
        self.timestamp
    }

    /// Read application metadata accumulated by earlier pre-hooks.
    #[must_use]
    pub fn metadata(&self) -> &BTreeMap<String, Value> {
        self.metadata
    }

    /// Add or change application metadata passed to later and post hooks.
    pub fn metadata_mut(&mut self) -> &mut BTreeMap<String, Value> {
        self.metadata
    }

    /// Store JSON-compatible application metadata without requiring a direct
    /// `serde_json` dependency for common scalar values.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.metadata.insert(key.into(), value.into());
    }
}

/// Object-safe lifecycle hook for an exact generated entity or relation.
pub trait LifecycleHook: Send + Sync {
    /// Stable application-owned hook name used in diagnostics.
    fn name(&self) -> &str;

    /// Run before database work. Return [`PreHookResult::Reject`] to cancel.
    fn before_operation<'a>(
        &'a self,
        context: &'a mut HookContext<'_>,
    ) -> HookFuture<'a, std::result::Result<PreHookResult, HookError>>;

    /// Run after a successful commit. Errors are logged and not propagated.
    fn after_operation<'a>(
        &'a self,
        context: &'a HookContext<'_>,
    ) -> HookFuture<'a, std::result::Result<(), HookError>>;

    /// Return `false` to skip this hook for one context.
    fn should_run(&self, context: &HookContext<'_>) -> bool {
        let _ = context;
        true
    }
}

#[derive(Default)]
pub(crate) struct HookRunner {
    hooks: Vec<Arc<dyn LifecycleHook>>,
}

pub(crate) struct HookState {
    metadata: BTreeMap<String, Value>,
    timestamp: SystemTime,
}

impl Clone for HookRunner {
    fn clone(&self) -> Self {
        Self {
            hooks: self.hooks.clone(),
        }
    }
}

impl HookRunner {
    pub(crate) fn add(&mut self, hook: Arc<dyn LifecycleHook>) {
        self.hooks.push(hook);
    }

    pub(crate) fn has_hooks(&self) -> bool {
        !self.hooks.is_empty()
    }

    pub(crate) async fn run_pre(
        &self,
        type_id_json: &'static str,
        model_kind: ModelKind,
        operation: CrudOperation,
        iid: Option<&str>,
        input: Option<&EncodedCreate>,
    ) -> Result<HookState> {
        let type_name = type_name(type_id_json);
        let mut metadata = BTreeMap::new();
        let timestamp = SystemTime::now();
        for hook in &self.hooks {
            let mut context = HookContext {
                type_id_json,
                type_name: &type_name,
                model_kind,
                operation,
                iid,
                input,
                timestamp: &timestamp,
                metadata: &mut metadata,
            };
            if !hook.should_run(&context) {
                continue;
            }
            match hook.before_operation(&mut context).await {
                Ok(PreHookResult::Continue) => {}
                Ok(PreHookResult::Reject { reason }) => {
                    return Err(Error::from_hook(HookError::Rejected {
                        hook_name: hook.name().to_owned(),
                        operation,
                        reason,
                    }));
                }
                Err(error) => return Err(Error::from_hook(error)),
            }
        }
        Ok(HookState {
            metadata,
            timestamp,
        })
    }

    pub(crate) async fn run_post(
        &self,
        type_id_json: &'static str,
        model_kind: ModelKind,
        operation: CrudOperation,
        iid: Option<&str>,
        input: Option<&EncodedCreate>,
        mut state: HookState,
    ) {
        let type_name = type_name(type_id_json);
        let context = HookContext {
            type_id_json,
            type_name: &type_name,
            model_kind,
            operation,
            iid,
            input,
            timestamp: &state.timestamp,
            metadata: &mut state.metadata,
        };
        for hook in self.hooks.iter().rev() {
            if !hook.should_run(&context) {
                continue;
            }
            if let Err(error) = hook.after_operation(&context).await {
                tracing::warn!(hook = hook.name(), error = %error, "generated post-hook error");
            }
        }
    }
}

fn type_name(type_id_json: &str) -> String {
    serde_json::from_str::<Value>(type_id_json)
        .ok()
        .and_then(|value| value.get("label")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| type_id_json.to_owned())
}
