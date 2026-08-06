//! [`HookRunner`] — shared hook execution engine used by both managers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use crate::error::{OrmError, Result};
use crate::value::AttributeValue;

use super::context::{CrudOperation, HookContext, TypeKind};
use super::error::HookError;
use super::traits::{LifecycleHook, PreHookResult};

/// Shared hook execution engine used by both
/// [`EntityManager`](crate::_manager::EntityManager) and
/// [`RelationManager`](crate::_manager::RelationManager).
///
/// When no hooks are registered, all methods are effectively no-ops.
#[derive(Default, Clone)]
pub struct HookRunner {
    hooks: Vec<Arc<dyn LifecycleHook>>,
}

impl HookRunner {
    /// Create a new empty runner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a hook.
    pub fn add_hook(&mut self, hook: Arc<dyn LifecycleHook>) {
        self.hooks.push(hook);
    }

    /// Whether any hooks are registered.
    pub fn has_hooks(&self) -> bool {
        !self.hooks.is_empty()
    }

    /// Build a [`HookContext`] for the given operation.
    pub fn build_context(
        type_name: &'static str,
        type_kind: TypeKind,
        operation: CrudOperation,
        attributes: Vec<(&'static str, AttributeValue)>,
        iid: Option<String>,
    ) -> HookContext {
        HookContext {
            type_name,
            type_kind,
            operation,
            attributes,
            iid,
            metadata: HashMap::new(),
            timestamp: SystemTime::now(),
        }
    }

    /// Run pre-hooks in registration order.
    ///
    /// Short-circuits on [`PreHookResult::Reject`], converting it to
    /// [`OrmError::Hook`].
    pub async fn run_pre_hooks(&self, ctx: &mut HookContext) -> Result<()> {
        for hook in &self.hooks {
            if !hook.should_run(ctx) {
                continue;
            }
            match hook.before_operation(ctx).await {
                Ok(PreHookResult::Continue) => {}
                Ok(PreHookResult::Reject { reason }) => {
                    return Err(OrmError::Hook(HookError::Rejected {
                        hook_name: hook.name().to_string(),
                        operation: ctx.operation,
                        reason,
                    }));
                }
                Err(e) => return Err(OrmError::Hook(e)),
            }
        }
        Ok(())
    }

    /// Run post-hooks in reverse registration order.
    ///
    /// Errors are logged but do **not** propagate.
    pub async fn run_post_hooks(&self, ctx: &HookContext) {
        for hook in self.hooks.iter().rev() {
            if !hook.should_run(ctx) {
                continue;
            }
            if let Err(e) = hook.after_operation(ctx).await {
                tracing::warn!(
                    hook = hook.name(),
                    error = %e,
                    "Post-hook error (non-fatal)"
                );
            }
        }
    }
}

impl std::fmt::Debug for HookRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookRunner")
            .field("hook_count", &self.hooks.len())
            .finish()
    }
}
