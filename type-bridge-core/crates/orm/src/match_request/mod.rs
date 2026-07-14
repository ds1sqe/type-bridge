//! Canonical, provider-neutral typed match request and result contract.
//!
//! Language bindings normalize their public query APIs into this algebra. A
//! request remains untrusted until the validator resolves its descriptor
//! identities, checks topology and resource limits, and binds it to a schema
//! fingerprint and live invocation token.

pub mod capability;
pub mod diagnostic;
pub mod error;
pub mod handles;
pub mod ids;
pub mod limits;
pub(crate) mod lowering;
pub mod model;
pub mod recording;
pub mod result;
pub(crate) mod result_validation;
pub(crate) mod selected_result_executor;
pub mod validation;

pub use capability::{Capability, CapabilitySet, derive_required_capabilities};
pub use diagnostic::{
    MATCH_DIAGNOSTIC_VERSION_V1, MatchDiagnosticVersion, UnvalidatedMatchRequest,
};
pub use error::{
    MatchError, MatchErrorCategory, MatchErrorCode, MatchErrorDetailValue, MatchErrorPath,
    MatchErrorPathSegment,
};
pub use handles::{
    BindingHandle, FieldHandle, OrderHandle, PredicateHandle, QueryHandle, RoleHandle,
    SelectionHandle, SessionHandle, ShapeHandle,
};
pub use ids::{
    BindingId, BoundFieldId, DescriptorId, FieldId, RequestToken, ResultShapeId, RoleEdgeId,
    RoleId, SchemaFingerprint, SessionBindingToken, SessionId,
};
pub use limits::*;
pub use model::*;
pub use recording::{RecordingMatchExecutor, RecordingMatchResponse};
pub use result::*;
pub use selected_result_executor::MatchExecutionLimits;
pub use validation::{
    StableOrderOrigin, StableOrderSpec, StableOrderTerm, ValidatedMatchRequest,
    validate_match_request,
};
