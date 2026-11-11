/// JSON audit-log interceptor.
pub mod audit_log;
/// Ordered interceptor-chain execution.
pub mod chain;
/// Core interceptor traits, contexts, and errors.
pub mod traits;

pub use chain::InterceptorChain;
#[allow(unused_imports)] // re-exported for downstream use
pub use traits::InterceptError;
pub use traits::{Interceptor, RequestContext};

pub use traits::{CommitOutcome, PolicyOutcome, PolicyPlan, StagedWriteAuthority};
