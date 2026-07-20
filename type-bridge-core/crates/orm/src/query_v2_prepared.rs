//! Prepared-plan execution facade for idiomatic binding projections.
//!
//! Bindings hold opaque native handles and move exactly three things across
//! the boundary: canonical declared-schema bytes (once, to build a
//! [`QueryAuthority`]), canonical plan bytes, and small JSON payloads for
//! invocations and typed outcomes. Local execution and the remote envelope
//! share the same authority, so a prepared plan runs identically through
//! either path — the Rust engine is the only semantic implementation.

use serde::{Deserialize, Serialize};
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan::{
    InputRow, QueryInvocation, QueryOperation, QueryPlan, decode_query_plan,
};
use type_bridge_contract::query_remote::RemoteLimits;
use type_bridge_contract::schema::decode_declared_schema;
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan};
use type_bridge_schema::ResolvedSchema;

use crate::query_v2::{QueryV2ExecutionError, failure};
use crate::query_v2_remote::{decode_remote_outcome, encode_remote_request, remote_outcome};
use crate::session::backend::BoundedAnswerLimits;
use crate::session::database::Database;

/// One owned schema authority prepared plans validate against.
pub struct QueryAuthority {
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
}

impl QueryAuthority {
    /// Build one authority from canonical declared-schema bytes.
    pub fn from_declared_bytes(
        bytes: &[u8],
        scope: &str,
        profile: &str,
    ) -> Result<Self, Diagnostic> {
        let declared = decode_declared_schema(bytes)?;
        let profile = SemanticProfileId::new(profile)?;
        let resolved =
            type_bridge_schema::resolve(&declared, &profile).map_err(|_| schema_rejected())?;
        let managed = type_bridge_schema::managed_schema_state(
            &declared,
            &type_bridge_schema::ManagedDeltaContext::new(
                ManagedScopeId::new(scope)?,
                profile,
                CapabilitySet::new(),
            ),
        )
        .map_err(|_| schema_rejected())?;
        Ok(Self { managed, resolved })
    }

    /// Borrow the validation context this authority represents.
    #[must_use]
    pub fn context(&self) -> MigrationAssertionValidationContext<'_> {
        MigrationAssertionValidationContext::new(&self.resolved, &self.managed)
    }

    fn validate(&self, plan_bytes: &[u8]) -> Result<(QueryPlan, ValidatedQuery), Diagnostic> {
        let plan = decode_query_plan(plan_bytes)?;
        let validated = validate_query_plan(&plan, &self.context(), StructuralLimits::CANONICAL)?;
        Ok((plan, validated))
    }
}

/// One binding-facing invocation payload: operation plus input rows.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedInvocation {
    operation: PreparedOperation,
    rows: Vec<Vec<Option<CanonicalValue>>>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PreparedOperation {
    Rows,
    Count,
    Exists,
}

impl PreparedOperation {
    const fn operation(self) -> QueryOperation {
        match self {
            Self::Rows => QueryOperation::Rows,
            Self::Count => QueryOperation::Count,
            Self::Exists => QueryOperation::Exists,
        }
    }
}

fn parse_invocation(
    plan: &QueryPlan,
    invocation_json: &str,
) -> Result<QueryInvocation, Diagnostic> {
    let parsed: PreparedInvocation = serde_json::from_str(invocation_json).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "query_prepared_invocation_malformed",
            "invocation payloads carry an operation and rectangular rows",
        )
    })?;
    QueryInvocation::new(
        plan,
        parsed.operation.operation(),
        parsed.rows.into_iter().map(InputRow::new).collect(),
    )
}

/// Execute one prepared plan against a connected database, locally.
///
/// Returns the typed outcome as canonical JSON in the same shape the
/// remote envelope carries, so bindings decode one format for both paths.
pub async fn execute_prepared_local(
    database: &Database,
    authority: &QueryAuthority,
    plan_bytes: &[u8],
    invocation_json: &str,
    limits: BoundedAnswerLimits,
) -> Result<String, Diagnostic> {
    let (plan, validated) = authority.validate(plan_bytes)?;
    let invocation = parse_invocation(&plan, invocation_json)?;
    let mut transaction = database.read_transaction().await.map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_transaction_failed",
            "the executor could not open a read transaction",
        )
    })?;
    let outcome =
        crate::query_v2::execute_validated_query(&mut transaction, &validated, &invocation, limits)
            .await
            .map_err(|error| match error {
                QueryV2ExecutionError::Validation(diagnostic) => diagnostic,
                QueryV2ExecutionError::Provider(_) => failure(
                    DiagnosticCategory::Integrity,
                    "query_prepared_provider_failed",
                    "the executor provider call failed",
                ),
            })?;
    outcome_json(&outcome)
}

/// Encode one prepared invocation into remote request envelope bytes.
///
/// The plan validates against the caller's authority first, so a stale or
/// invalid plan never leaves the client.
pub fn encode_prepared_remote_request(
    authority: &QueryAuthority,
    plan_bytes: &[u8],
    invocation_json: &str,
    limits: RemoteLimits,
    nonce: &str,
) -> Result<Vec<u8>, Diagnostic> {
    let (plan, validated) = authority.validate(plan_bytes)?;
    let invocation = parse_invocation(&plan, invocation_json)?;
    encode_remote_request(&validated, &invocation, limits, nonce)
}

/// Decode one remote response into the typed outcome JSON.
///
/// Evidence validates against the caller's own authority before any value
/// crosses back into the binding.
pub fn decode_prepared_remote_outcome(
    authority: &QueryAuthority,
    plan_bytes: &[u8],
    invocation_json: &str,
    response_bytes: &[u8],
    nonce: &str,
    limits: RemoteLimits,
) -> Result<String, Diagnostic> {
    let (plan, validated) = authority.validate(plan_bytes)?;
    let invocation = parse_invocation(&plan, invocation_json)?;
    let outcome = decode_remote_outcome(
        response_bytes,
        &validated,
        invocation.operation(),
        nonce,
        limits,
    )?;
    outcome_json(&outcome)
}

fn schema_rejected() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_prepared_schema_rejected",
        "the declared schema does not resolve under this profile",
    )
}

fn outcome_json(outcome: &crate::query_v2::QueryV2Outcome) -> Result<String, Diagnostic> {
    serde_json::to_string(&remote_outcome(outcome)).map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_outcome_unserializable",
            "the typed outcome could not be serialized",
        )
    })
}
