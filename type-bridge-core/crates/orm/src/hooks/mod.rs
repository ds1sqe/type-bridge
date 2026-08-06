//! Lifecycle hook system for CRUD operations.
//!
//! Hooks are registered on manager instances via
//! [`add_hook`](crate::_manager::EntityManager::add_hook).
//! Pre-hooks run in registration order and may reject operations.
//! Post-hooks run in reverse order; errors are logged but not propagated.

mod context;
mod error;
mod runner;
mod traits;

pub use context::{CrudOperation, HookContext, TypeKind};
pub use error::HookError;
pub use runner::HookRunner;
pub use traits::{LifecycleHook, PreHookResult};
