/// JSON audit-log interceptor.
pub mod audit_log;
/// Ordered interceptor-chain execution.
pub mod chain;
/// CRUD metadata extraction from query clauses.
pub mod crud_info;
/// Adapter API for CRUD-aware interceptors.
pub mod crud_interceptor;
/// Core interceptor traits, contexts, and errors.
pub mod traits;

pub use chain::InterceptorChain;
pub use crud_info::{CrudInfo, CrudOperation, TypeKind};
pub use crud_interceptor::{CrudInterceptor, CrudInterceptorAdapter};
#[allow(unused_imports)] // re-exported for downstream use
pub use traits::InterceptError;
pub use traits::{Interceptor, RequestContext};
#[cfg(feature = "v2-query")]
pub use traits::{V2PolicyOutcome, V2PolicyRequest};
