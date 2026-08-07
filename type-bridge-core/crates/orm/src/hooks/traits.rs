//! The [`LifecycleHook`] trait and [`PreHookResult`] enum.

use crate::session::backend::BoxFuture;

use super::context::HookContext;
use super::error::HookError;

/// Result of a pre-operation hook.
#[derive(Debug)]
pub enum PreHookResult {
    /// Continue with the operation.
    Continue,
    /// Reject the operation with a reason.
    Reject {
        /// Caller-safe explanation for the rejection.
        reason: String,
    },
}

/// Trait for lifecycle hooks that run before and after CRUD operations.
///
/// All hooks must be `Send + Sync` for use across async tasks.
/// Methods use [`BoxFuture`] for object-safe async.
///
/// # Example
///
/// This intentionally abbreviated implementor is ignored because it omits
/// imports and the surrounding generated-model application wiring.
///
/// ```ignore
/// struct AuditLogger;
///
/// impl LifecycleHook for AuditLogger {
///     fn name(&self) -> &str { "audit-logger" }
///
///     fn before_operation<'a>(
///         &'a self,
///         ctx: &'a mut HookContext,
///     ) -> BoxFuture<'a, Result<PreHookResult, HookError>> {
///         Box::pin(async move {
///             tracing::info!(type_name = ctx.type_name, "Before operation");
///             Ok(PreHookResult::Continue)
///         })
///     }
///
///     fn after_operation<'a>(
///         &'a self,
///         ctx: &'a HookContext,
///     ) -> BoxFuture<'a, Result<(), HookError>> {
///         Box::pin(async move {
///             tracing::info!(type_name = ctx.type_name, "After operation");
///             Ok(())
///         })
///     }
/// }
/// ```
pub trait LifecycleHook: Send + Sync {
    /// Human-readable name for this hook (used in logs and errors).
    fn name(&self) -> &str;

    /// Called before a CRUD operation.
    ///
    /// Can inspect/modify the context or reject the operation entirely.
    fn before_operation<'a>(
        &'a self,
        ctx: &'a mut HookContext,
    ) -> BoxFuture<'a, Result<PreHookResult, HookError>>;

    /// Called after a successful CRUD operation.
    fn after_operation<'a>(&'a self, ctx: &'a HookContext) -> BoxFuture<'a, Result<(), HookError>>;

    /// Return `false` to skip this hook for the given context.
    ///
    /// Defaults to always running.
    fn should_run(&self, ctx: &HookContext) -> bool {
        let _ = ctx;
        true
    }
}
