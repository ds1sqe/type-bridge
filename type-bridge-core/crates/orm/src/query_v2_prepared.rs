//! Prepared-plan execution facade for idiomatic binding projections.
//!
//! Bindings hold opaque native handles and move exactly three things across
//! the boundary: canonical declared-schema bytes (once, to build a
//! [`QueryAuthority`]), canonical plan bytes, and small JSON payloads for
//! invocations and typed outcomes. Local execution and the remote envelope
//! share the same authority, so a prepared plan runs identically through
//! either path — the Rust engine is the only semantic implementation.

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::{Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::limits::{
    MAX_QUERY_INVOCATION_BYTES, MAX_REMOTE_ENVELOPE_BYTES, StructuralLimits,
};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::query_plan::{
    InputRow, QueryInvocation, QueryOperation, QueryPlan, decode_query_invocation,
    decode_query_plan,
};
use type_bridge_contract::query_remote::{
    DEFAULT_REMOTE_DEADLINE_MS, RemoteCapabilities, RemoteCapabilitiesFingerprint, RemoteLimits,
    RemoteRequestFingerprint, RemoteSigningPublicKey,
};
use type_bridge_contract::query_remote_v2::{
    RemoteLimitsV2, RemoteQueryRequestV2, RemoteReplyDecodeLimitsV2, RemoteRequestFingerprintV2,
};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, decode_declared_schema};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_contract::value::CanonicalValue;
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan};
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema};
use type_bridge_schema_compat::{LiveQueryControlPresence, rebuild_live_query_authority};

use crate::Transaction;
use crate::query_v2::{QueryV2ExecutionError, failure};
use crate::query_v2_builder::QueryAuthorityIdentity;
use crate::query_v2_remote::{
    check_advertised_capabilities, check_advertised_capabilities_v2, decode_remote_outcome,
    decode_remote_outcome_v2, encode_remote_request_at, encode_remote_request_v2_at,
    remote_outcome,
};
use crate::session::backend::{
    MAX_QUERY_V2_SCHEMA_FENCE_DURATION, QueryResult, QueryV2AnswerLimits,
};
use crate::session::database::{Database, DatabaseExecutionIdentity};

const PREPARED_CLOSE_GRACE: Duration = Duration::from_secs(1);
const LIVE_AUTHORITY_REBUILD_CONCURRENCY: usize = 4;

/// Return the binding-neutral diagnostic for a host string that cannot be
/// represented as a Rust/JSON Unicode scalar sequence.
#[doc(hidden)]
pub fn query_v2_host_string_unicode_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_v2_host_string_unicode",
        "host string input must be valid Unicode without surrogate code points",
    )
}

/// Return the stable diagnostic for a non-string value at a host text boundary.
#[doc(hidden)]
#[must_use]
pub fn query_v2_host_string_type_error() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_v2_host_string_type",
        "host string input must be a string value",
    )
}

static LIVE_AUTHORITY_REBUILD_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> = LazyLock::new(|| {
    Arc::new(tokio::sync::Semaphore::new(
        LIVE_AUTHORITY_REBUILD_CONCURRENCY,
    ))
});

/// Acquire one process-wide slot for bounded live-schema reconstruction.
///
/// The owned permit is moved into the blocking task, so deadline cancellation
/// cannot detach an unbounded number of CPU- and memory-heavy parses.
#[doc(hidden)]
pub async fn acquire_live_authority_rebuild_permit() -> Result<tokio::sync::OwnedSemaphorePermit, ()>
{
    Arc::clone(&LIVE_AUTHORITY_REBUILD_SLOTS)
        .acquire_owned()
        .await
        .map_err(|_| ())
}

/// One owned schema authority prepared plans validate against.
pub struct QueryAuthority {
    declared: DeclaredSchema,
    delta_context: ManagedDeltaContext,
    identity: QueryAuthorityIdentity,
    managed: ManagedSchemaState,
    resolved: ResolvedSchema,
    control_policy: QueryAuthorityControlPolicy,
}

enum QueryAuthorityControlPolicy {
    Managed,
    QueryOnly(DatabaseExecutionIdentity),
}

impl QueryAuthority {
    /// Build one authority from canonical declared-schema bytes.
    ///
    /// The caller supplied the schema artifact, so its declared required
    /// capabilities are the authority for resolution — a schema requiring
    /// `schema.roles` or any other capability builds exactly as it does on
    /// the server. Contract-level rejections keep their specific
    /// diagnostic instead of collapsing into a generic one.
    pub fn from_declared_bytes(
        bytes: &[u8],
        scope: &str,
        profile: &str,
    ) -> Result<Self, Diagnostic> {
        Self::from_declared_bytes_with_policy(
            bytes,
            scope,
            profile,
            QueryAuthorityControlPolicy::Managed,
        )
    }

    /// Build an authority for a database that deliberately has no migration
    /// control partition.
    ///
    /// Query-only mode is explicit and bound to the exact provider authority
    /// and database name supplied here. It cannot silently downgrade a managed
    /// authority when control facts disappear.
    pub fn from_declared_bytes_query_only(
        bytes: &[u8],
        scope: &str,
        profile: &str,
        database: &Database,
    ) -> Result<Self, Diagnostic> {
        Self::from_declared_bytes_with_policy(
            bytes,
            scope,
            profile,
            QueryAuthorityControlPolicy::QueryOnly(database.execution_identity()),
        )
    }

    fn from_declared_bytes_with_policy(
        bytes: &[u8],
        scope: &str,
        profile: &str,
        control_policy: QueryAuthorityControlPolicy,
    ) -> Result<Self, Diagnostic> {
        let declared = decode_declared_schema(bytes)?;
        let profile = SemanticProfileId::new(profile)?;
        let resolved = type_bridge_schema::resolve_schema_with_capabilities(
            &declared,
            &profile,
            declared.required_capabilities(),
        )
        .map_err(|_| schema_rejected())?;
        let delta_context = ManagedDeltaContext::new(
            ManagedScopeId::new(scope)?,
            profile,
            declared.required_capabilities().clone(),
        );
        let managed = type_bridge_schema::managed_schema_state(&declared, &delta_context).map_err(
            |error| match error {
                type_bridge_schema::DeltaError::Contract(diagnostic) => diagnostic,
                type_bridge_schema::DeltaError::Schema(_) => schema_rejected(),
            },
        )?;
        Ok(Self {
            declared,
            delta_context,
            identity: QueryAuthorityIdentity::fresh(),
            managed,
            resolved,
            control_policy,
        })
    }

    /// Borrow the validation context this authority represents.
    #[must_use]
    pub fn context(&self) -> MigrationAssertionValidationContext<'_> {
        MigrationAssertionValidationContext::new(&self.resolved, &self.managed)
    }

    /// Return the opaque identity of this exact authority instance.
    #[must_use]
    pub fn identity(&self) -> QueryAuthorityIdentity {
        self.identity.clone()
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

fn invocation_input_byte_limit() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "query_invocation_input_byte_limit",
        "invocation input rows exceed the structural byte ceiling",
    )
}

fn prepared_invocation_malformed() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_prepared_invocation_malformed",
        "invocation payloads carry an operation and rectangular rows",
    )
}

enum AuthoredInvocationField {
    Inputs,
    Operation,
    PlanFingerprint,
}

impl<'de> Deserialize<'de> for AuthoredInvocationField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FieldVisitor;

        impl Visitor<'_> for FieldVisitor {
            type Value = AuthoredInvocationField;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a canonical authored invocation field")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "inputs" => Ok(AuthoredInvocationField::Inputs),
                    "operation" => Ok(AuthoredInvocationField::Operation),
                    "plan_fingerprint" => Ok(AuthoredInvocationField::PlanFingerprint),
                    _ => Err(E::unknown_field(
                        value,
                        &["inputs", "operation", "plan_fingerprint"],
                    )),
                }
            }
        }

        deserializer.deserialize_identifier(FieldVisitor)
    }
}

struct IgnoredSequence;

impl<'de> Deserialize<'de> for IgnoredSequence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SequenceVisitor;

        impl<'de> Visitor<'de> for SequenceVisitor {
            type Value = IgnoredSequence;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an input-row array")
            }

            fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                while sequence.next_element::<IgnoredAny>()?.is_some() {}
                Ok(IgnoredSequence)
            }
        }

        deserializer.deserialize_seq(SequenceVisitor)
    }
}

struct IgnoredObject;

impl<'de> Deserialize<'de> for IgnoredObject {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = IgnoredObject;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a plan-fingerprint object")
            }

            fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                while object.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                Ok(IgnoredObject)
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

struct AuthoredInvocationShape;

impl<'de> Deserialize<'de> for AuthoredInvocationShape {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ShapeVisitor;

        impl<'de> Visitor<'de> for ShapeVisitor {
            type Value = AuthoredInvocationShape;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("the exact canonical authored invocation shape")
            }

            fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                match object.next_key()? {
                    Some(AuthoredInvocationField::Inputs) => {
                        object.next_value::<IgnoredSequence>()?;
                    }
                    _ => return Err(A::Error::custom("inputs must be the first field")),
                }
                match object.next_key()? {
                    Some(AuthoredInvocationField::Operation) => {
                        object.next_value::<PreparedOperation>()?;
                    }
                    _ => return Err(A::Error::custom("operation must be the second field")),
                }
                match object.next_key()? {
                    Some(AuthoredInvocationField::PlanFingerprint) => {
                        object.next_value::<IgnoredObject>()?;
                    }
                    _ => {
                        return Err(A::Error::custom("plan_fingerprint must be the third field"));
                    }
                }
                if object.next_key::<AuthoredInvocationField>()?.is_some() {
                    return Err(A::Error::custom(
                        "authored invocation has an additional field",
                    ));
                }
                Ok(AuthoredInvocationShape)
            }
        }

        deserializer.deserialize_map(ShapeVisitor)
    }
}

fn has_exact_authored_invocation_shape(invocation_json: &str) -> bool {
    invocation_json.as_bytes().starts_with(br#"{"inputs":"#)
        && serde_json::from_str::<AuthoredInvocationShape>(invocation_json).is_ok()
}

fn parse_invocation(
    plan: &QueryPlan,
    invocation_json: &str,
) -> Result<QueryInvocation, Diagnostic> {
    let bytes = invocation_json.as_bytes();
    let within_legacy_limit = StructuralLimits::CANONICAL.allows_input_bytes(bytes.len());
    if bytes.len() > MAX_QUERY_INVOCATION_BYTES {
        return Err(invocation_input_byte_limit());
    }
    if within_legacy_limit
        && let Ok(parsed) = serde_json::from_str::<PreparedInvocation>(invocation_json)
    {
        let invocation = QueryInvocation::new(
            plan,
            parsed.operation.operation(),
            parsed.rows.into_iter().map(InputRow::new).collect(),
        )?;
        crate::query_v2::preflight_invocation_transport(plan, &invocation)?;
        return Ok(invocation);
    }

    if has_exact_authored_invocation_shape(invocation_json) {
        let invocation = decode_query_invocation(plan, bytes)?;
        crate::query_v2::preflight_invocation_transport(plan, &invocation)?;
        return Ok(invocation);
    }
    if !within_legacy_limit {
        return Err(invocation_input_byte_limit());
    }
    Err(prepared_invocation_malformed())
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
    limits: QueryV2AnswerLimits,
) -> Result<String, Diagnostic> {
    let deadline = limits.answer.deadline;
    let cancellation = limits.answer.cancellation.clone();
    let (plan, validated) = authority.validate(plan_bytes)?;
    let invocation = parse_invocation(&plan, invocation_json)?;
    if invocation
        .transport_capabilities()
        .contains(&type_bridge_contract::query_given_rows_capability())
        && !database.supports_given_stage()
    {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_given_transport_unsupported",
            "this invocation requires the native given transport capability",
        ));
    }
    verify_database_identity(database, authority)?;
    check_prepared_cancellation(&cancellation)?;
    let advisory_rebuild_permit = await_prepared_stage(
        acquire_live_authority_rebuild_permit(),
        deadline,
        &cancellation,
    )
    .await?
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_live_schema_unavailable",
            "the local executor could not reserve live-schema verification capacity",
        )
    })?;
    let advisory_export = await_prepared_stage(database.schema_text(), deadline, &cancellation)
        .await?
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_prepared_live_schema_unavailable",
                "the local executor could not verify its live schema authority",
            )
        })?;
    verify_live_authority_export_with_permit(
        advisory_export,
        authority,
        deadline,
        &cancellation,
        advisory_rebuild_permit,
    )
    .await?;
    let exact_rebuild_permit = await_prepared_stage(
        acquire_live_authority_rebuild_permit(),
        deadline,
        &cancellation,
    )
    .await?
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_live_schema_unavailable",
            "the local executor could not reserve live-schema verification capacity",
        )
    })?;
    let fence_timeout = schema_fence_timeout(deadline)?;
    let open = database.schema_fenced_read_transaction(fence_timeout);
    let opened = await_prepared_stage(open, deadline, &cancellation).await?;
    let (mut transaction, fenced_schema) = opened.map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_transaction_failed",
            "the executor could not open a schema-fenced read transaction",
        )
    })?;
    if let Err(diagnostic) = verify_live_authority_export_with_permit(
        fenced_schema,
        authority,
        deadline,
        &cancellation,
        exact_rebuild_permit,
    )
    .await
    {
        let primary_code = diagnostic.code().as_str();
        if !matches!(
            tokio::time::timeout(PREPARED_CLOSE_GRACE, transaction.close()).await,
            Ok(Ok(()))
        ) {
            tracing::warn!(
                primary_code,
                cleanup_code = "query_prepared_transaction_close_failed",
                "prepared query transaction cleanup failed after live-authority rejection"
            );
        }
        return Err(diagnostic);
    }
    if let Err(diagnostic) =
        verify_transaction_control_scope(&mut transaction, authority, deadline, &cancellation).await
    {
        let primary_code = diagnostic.code().as_str();
        if !matches!(
            tokio::time::timeout(PREPARED_CLOSE_GRACE, transaction.close()).await,
            Ok(Ok(()))
        ) {
            tracing::warn!(
                primary_code,
                cleanup_code = "query_prepared_transaction_close_failed",
                "prepared query transaction cleanup failed after control-authority rejection"
            );
        }
        return Err(diagnostic);
    }
    let execute =
        crate::query_v2::execute_validated_query(&mut transaction, &validated, &invocation, limits);
    let executed = match deadline {
        Some(deadline) => {
            match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), execute).await {
                Ok(executed) => executed,
                Err(_) => Err(QueryV2ExecutionError::Validation(deadline_exceeded())),
            }
        }
        None => execute.await,
    };
    let execution = executed.map_err(|error| match error {
        QueryV2ExecutionError::Validation(diagnostic) => diagnostic,
        QueryV2ExecutionError::Provider(error) => crate::query_v2::provider_diagnostic(
            &error,
            "query_prepared_provider_failed",
            "the executor provider call failed",
        ),
    });
    let execution_code = execution
        .as_ref()
        .map_or_else(|diagnostic| diagnostic.code().as_str(), |_| "ok");
    if !matches!(
        tokio::time::timeout(PREPARED_CLOSE_GRACE, transaction.close()).await,
        Ok(Ok(()))
    ) {
        tracing::warn!(
            execution_code,
            cleanup_code = "query_prepared_transaction_close_failed",
            "prepared query transaction cleanup failed"
        );
    }
    let outcome = execution?;
    outcome_json(&outcome)
}

async fn verify_live_authority_export_with_permit(
    export: String,
    authority: &QueryAuthority,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
    rebuild_permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<(), Diagnostic> {
    check_prepared_cancellation(cancellation)?;
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err(deadline_exceeded());
    }
    let document = DocumentId::new("typebridge-v2-local-live-export.typeql")?;
    let declared = authority.declared.clone();
    let context = authority.delta_context.clone();
    let rebuild = tokio::task::spawn_blocking(move || {
        let _rebuild_permit = rebuild_permit;
        rebuild_live_query_authority(document, &export, &declared, &context)
    });
    let live = await_prepared_stage(rebuild, deadline, cancellation)
        .await?
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_prepared_live_schema_invalid",
                "the local executor live schema cannot form trusted query authority",
            )
        })?
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_prepared_live_schema_invalid",
                "the local executor live schema cannot form trusted query authority",
            )
        })?;
    if live.managed() != &authority.managed {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_stale_schema",
            "the local executor live schema no longer matches its declared authority",
        ));
    }
    match (&authority.control_policy, live.control_presence()) {
        (QueryAuthorityControlPolicy::Managed, LiveQueryControlPresence::ManagedFence)
        | (QueryAuthorityControlPolicy::QueryOnly(_), LiveQueryControlPresence::Absent) => {}
        (QueryAuthorityControlPolicy::Managed, LiveQueryControlPresence::Absent) => {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_prepared_managed_control_missing",
                "the managed executor database has no migration control authority",
            ));
        }
        (QueryAuthorityControlPolicy::QueryOnly(_), LiveQueryControlPresence::ManagedFence) => {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_prepared_query_only_control_present",
                "the query-only executor database unexpectedly has managed control authority",
            ));
        }
        (_, LiveQueryControlPresence::ManagedFenceWithExtensions) => {
            return Err(failure(
                DiagnosticCategory::Integrity,
                "query_prepared_live_schema_invalid",
                "the local executor managed control schema carries unsupported released-only extensions",
            ));
        }
    }
    if matches!(
        &authority.control_policy,
        QueryAuthorityControlPolicy::QueryOnly(_)
    ) && live.legacy_control_present()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_query_only_legacy_control_present",
            "the query-only executor database unexpectedly has legacy migration control authority",
        ));
    }
    Ok(())
}

fn verify_database_identity(
    database: &Database,
    authority: &QueryAuthority,
) -> Result<(), Diagnostic> {
    let server_version = database.server_version().ok_or_else(|| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_server_identity_unavailable",
            "the executor cannot prove the exact TypeDB semantic profile",
        )
    })?;
    let observed_profile = SemanticProfileId::new(format!("typedb-{server_version}/v1"))?;
    if &observed_profile != authority.delta_context.semantic_profile() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_semantic_profile_mismatch",
            "the executor TypeDB semantic profile differs from the prepared authority",
        ));
    }
    if let QueryAuthorityControlPolicy::QueryOnly(expected) = &authority.control_policy
        && expected != &database.execution_identity()
    {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_database_identity_mismatch",
            "the query-only authority is bound to a different executor database",
        ));
    }
    Ok(())
}

fn schema_fence_timeout(deadline: Option<std::time::Instant>) -> Result<Duration, Diagnostic> {
    match deadline {
        Some(deadline) => deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .map(|remaining| remaining.min(MAX_QUERY_V2_SCHEMA_FENCE_DURATION))
            .ok_or_else(deadline_exceeded),
        None => Ok(MAX_QUERY_V2_SCHEMA_FENCE_DURATION),
    }
}

fn cancelled() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "provider_cancelled",
        "provider answer processing was cancelled",
    )
}

fn check_prepared_cancellation(
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<(), Diagnostic> {
    if cancellation.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}

async fn await_prepared_stage<F>(
    future: F,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<F::Output, Diagnostic>
where
    F: Future,
{
    check_prepared_cancellation(cancellation)?;
    tokio::pin!(future);
    let output = match deadline {
        Some(deadline) => {
            let timeout = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
            tokio::pin!(timeout);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(cancelled()),
                _ = &mut timeout => return Err(deadline_exceeded()),
                output = &mut future => output,
            }
        }
        None => {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(cancelled()),
                output = &mut future => output,
            }
        }
    };
    check_prepared_cancellation(cancellation)?;
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err(deadline_exceeded());
    }
    Ok(output)
}

async fn verify_transaction_control_scope(
    transaction: &mut Transaction,
    authority: &QueryAuthority,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<(), Diagnostic> {
    if matches!(
        &authority.control_policy,
        QueryAuthorityControlPolicy::QueryOnly(_)
    ) {
        return Ok(());
    }
    verify_managed_query_control_scope(
        transaction,
        authority.delta_context.scope_id(),
        deadline,
        cancellation,
    )
    .await
}

/// Verify the one global managed control row inside the same schema-fenced
/// transaction that will execute a V2 query.
#[doc(hidden)]
pub async fn verify_managed_query_control_scope(
    transaction: &mut Transaction,
    expected_scope: &ManagedScopeId,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<(), Diagnostic> {
    let documents = managed_control_documents(
        transaction,
        "match $control isa typebridge-internal-v2-migration-control, \
             has typebridge-internal-v2-control-scope $scope, \
             has typebridge-internal-v2-lease-fence $fence, \
             has typebridge-internal-v2-lease-state $state; \
             limit 2; \
             fetch { \"scope\": $scope, \"fence\": $fence, \"state\": $state };",
        deadline,
        cancellation,
    )
    .await?;
    if documents.len() != 1 {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor database must have exactly one managed control row",
        ));
    }
    let scalar = |key: &str| {
        documents[0]
            .get(key)
            .and_then(provider_scalar)
            .ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "query_prepared_control_invalid",
                    "the executor managed control row is malformed",
                )
            })
    };
    if scalar("scope")? != expected_scope.as_str() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_managed_scope_mismatch",
            "the executor database is bound to a different managed scope",
        ));
    }
    let fence = scalar("fence")?;
    let parsed_fence = fence.parse::<u64>().map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor managed fence is not a canonical unsigned integer",
        )
    })?;
    if parsed_fence == 0 || parsed_fence.to_string() != fence {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor managed fence is not a canonical unsigned integer",
        ));
    }
    if scalar("state")? != "free" {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_migration_in_progress",
            "the executor database has an active or invalid migration fence",
        ));
    }
    let holder = managed_control_documents(
        transaction,
        "match $control isa typebridge-internal-v2-migration-control, \
         has typebridge-internal-v2-lease-holder $holder; \
         limit 1; fetch { \"holder\": $holder };",
        deadline,
        cancellation,
    )
    .await?;
    if !holder.is_empty() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_migration_in_progress",
            "the executor database has an active or invalid migration fence",
        ));
    }
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err(deadline_exceeded());
    }
    Ok(())
}

async fn managed_control_documents(
    transaction: &mut Transaction,
    typeql: &str,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<Vec<serde_json::Value>, Diagnostic> {
    let query = transaction.query(typeql);
    let result = await_prepared_stage(query, deadline, cancellation)
        .await?
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_prepared_control_unavailable",
                "the executor could not read managed control authority",
            )
        })?;
    match result {
        QueryResult::Documents(documents) => Ok(documents),
        QueryResult::Rows(_) | QueryResult::Ok => Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor managed control authority returned no document result",
        )),
    }
}

fn provider_scalar(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(value) => Some(value),
        serde_json::Value::Object(value) => value.get("value").and_then(provider_scalar),
        _ => None,
    }
}

fn deadline_exceeded() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "transaction_deadline_exceeded",
        "provider transaction deadline expired",
    )
}

/// One remote request and its one-shot request-bound reply decoder.
///
/// The pending value owns the validated output contract and exact request
/// fingerprint. Decoding consumes its reply slot before inspecting bytes, so
/// a captured success or failure cannot be accepted twice.
pub struct PendingRemoteQuery {
    state: Arc<PendingRemoteQueryState>,
}

struct PendingRemoteQueryState {
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    consumed: AtomicBool,
    limits: RemoteLimits,
    nonce: String,
    operation: QueryOperation,
    request: Vec<u8>,
    request_fingerprint: RemoteRequestFingerprint,
    reply_deadline: Instant,
    trusted_key: RemoteSigningPublicKey,
    validated: ValidatedQuery,
}

/// One non-clone capability reserving the only reply accepted for a request.
pub struct ClaimedRemoteReply {
    state: Arc<PendingRemoteQueryState>,
}

impl fmt::Debug for PendingRemoteQueryState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingRemoteQueryState")
            .field("consumed", &self.consumed.load(Ordering::Acquire))
            .field("request_len", &self.request.len())
            .field("sensitive_request_state", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PendingRemoteQuery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingRemoteQuery")
            .field("state", &self.state)
            .finish()
    }
}

impl fmt::Debug for ClaimedRemoteReply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedRemoteReply")
            .field("state", &self.state)
            .finish()
    }
}

impl PendingRemoteQuery {
    /// Borrow the exact canonical request bytes to send to the executor.
    #[must_use]
    pub fn request_bytes(&self) -> &[u8] {
        &self.state.request
    }

    /// Atomically reserve the one permitted reply before copying or queueing bytes.
    pub fn claim_reply(&self) -> Result<ClaimedRemoteReply, Diagnostic> {
        self.state
            .consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_reply_replayed",
                    "the pending remote request already consumed a reply",
                )
            })?;
        ensure_remote_reply_live(self.state.reply_deadline)?;
        Ok(ClaimedRemoteReply {
            state: Arc::clone(&self.state),
        })
    }

    /// Reserve and decode one reply through the shared one-shot path.
    pub fn decode_reply(&self, response_bytes: &[u8]) -> Result<String, Diagnostic> {
        self.claim_reply()?.decode(response_bytes)
    }
}

impl ClaimedRemoteReply {
    /// Maximum immutable response snapshot needed to preserve oversize detection.
    ///
    /// One byte beyond the protocol hard ceiling is sufficient to retain the
    /// exact oversized-envelope verdict without copying the rest of an
    /// attacker-controlled response into a binding-owned buffer. The caller's
    /// `max_bytes` is a successful-response budget and cannot constrain this
    /// pre-authentication snapshot: authenticated failure evidence must remain
    /// available to the decoder even when the success budget is zero.
    #[must_use]
    pub fn response_snapshot_limit(&self) -> usize {
        MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1)
    }

    /// Verify and decode the reserved reply, consuming this claim capability.
    pub fn decode(self, response_bytes: &[u8]) -> Result<String, Diagnostic> {
        ensure_remote_reply_live(self.state.reply_deadline)?;
        let outcome = decode_remote_outcome(
            response_bytes,
            &self.state.validated,
            self.state.operation,
            &self.state.nonce,
            &self.state.request_fingerprint,
            &self.state.advertisement_fingerprint,
            self.state.trusted_key,
            self.state.limits,
        )?;
        let outcome = outcome_json(&outcome)?;
        ensure_remote_reply_live(self.state.reply_deadline)?;
        Ok(outcome)
    }
}

/// Prepare one invocation and its one-shot remote reply decoder.
///
/// The plan validates against the caller's authority first, so a stale or
/// invalid plan never leaves the client. The caller also supplies the
/// executor's exact capability advertisement (the `/v2/capabilities`
/// bytes): a plan or invocation transport the executor cannot execute is
/// refused here, before any request bytes exist — the promised client
/// preflight, with the executor re-checking the same sets at admission.
pub fn prepare_remote_query(
    authority: &QueryAuthority,
    plan_bytes: &[u8],
    invocation_json: &str,
    advertisement_bytes: &[u8],
    limits: RemoteLimits,
) -> Result<PendingRemoteQuery, Diagnostic> {
    if matches!(
        &authority.control_policy,
        QueryAuthorityControlPolicy::QueryOnly(_)
    ) {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_remote_query_only_authority_local_only",
            "a query-only authority is bound to local database identity and cannot prepare a remote request",
        ));
    }
    let (plan, validated) = authority.validate(plan_bytes)?;
    let invocation = parse_invocation(&plan, invocation_json)?;
    let advertisement = RemoteCapabilities::decode(advertisement_bytes)?;
    check_advertised_capabilities(&validated, &invocation, &advertisement)?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let monotonic_prepared = Instant::now();
    let prepared_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_clock_invalid",
                "system clock cannot establish an absolute remote request time",
            )
        })?
        .as_millis()
        .try_into()
        .map_err(|_| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_clock_limit",
                "system clock exceeds the supported remote timestamp range",
            )
        })?;
    let lifetime = limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS);
    let reply_deadline = monotonic_prepared
        .checked_add(Duration::from_millis(lifetime))
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_deadline_limit",
                "remote deadline exceeds the maximum supported duration",
            )
        })?;
    let request = encode_remote_request_at(
        &validated,
        &invocation,
        &advertisement,
        limits,
        &nonce,
        prepared_at_unix_ms,
    )?;
    let request_fingerprint = RemoteRequestFingerprint::compute(&request)?;
    let advertisement_fingerprint = advertisement.fingerprint()?;
    let trusted_key = advertisement.reply_key();
    Ok(PendingRemoteQuery {
        state: Arc::new(PendingRemoteQueryState {
            advertisement_fingerprint,
            consumed: AtomicBool::new(false),
            limits,
            nonce,
            operation: invocation.operation(),
            request,
            request_fingerprint,
            reply_deadline,
            trusted_key,
            validated,
        }),
    })
}

/// One additive V2 request and its one-shot authenticated reply decoder.
pub struct PendingRemoteQueryV2 {
    state: Arc<PendingRemoteQueryStateV2>,
}

struct PendingRemoteQueryStateV2 {
    advertisement_fingerprint: RemoteCapabilitiesFingerprint,
    consumed: AtomicBool,
    limits: RemoteReplyDecodeLimitsV2,
    request: Vec<u8>,
    request_envelope: RemoteQueryRequestV2,
    request_fingerprint: RemoteRequestFingerprintV2,
    reply_deadline: Instant,
    trusted_key: RemoteSigningPublicKey,
    validated: ValidatedQuery,
}

/// One non-clone capability reserving the sole V2 reply accepted for a request.
pub struct ClaimedRemoteReplyV2 {
    state: Arc<PendingRemoteQueryStateV2>,
}

impl fmt::Debug for PendingRemoteQueryStateV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingRemoteQueryStateV2")
            .field("consumed", &self.consumed.load(Ordering::Acquire))
            .field("request_len", &self.request.len())
            .field("sensitive_request_state", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PendingRemoteQueryV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingRemoteQueryV2")
            .field("state", &self.state)
            .finish()
    }
}

impl fmt::Debug for ClaimedRemoteReplyV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClaimedRemoteReplyV2")
            .field("state", &self.state)
            .finish()
    }
}

impl PendingRemoteQueryV2 {
    /// Borrow the exact canonical V2 request bytes for one caller-owned exchange.
    #[must_use]
    pub fn request_bytes(&self) -> &[u8] {
        &self.state.request
    }

    /// Atomically reserve the only reply slot before response bytes are copied.
    pub fn claim_reply(&self) -> Result<ClaimedRemoteReplyV2, Diagnostic> {
        self.state
            .consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_v2_reply_replayed",
                    "pending V2 remote request already consumed a reply",
                )
            })?;
        ensure_remote_reply_live_v2(self.state.reply_deadline)?;
        Ok(ClaimedRemoteReplyV2 {
            state: Arc::clone(&self.state),
        })
    }

    /// Reserve and decode one reply through the V2 one-shot path.
    pub fn decode_reply(&self, response_bytes: &[u8]) -> Result<String, Diagnostic> {
        self.claim_reply()?.decode(response_bytes)
    }
}

impl ClaimedRemoteReplyV2 {
    /// Maximum bounded immutable response snapshot needed by the decoder.
    #[must_use]
    pub fn response_snapshot_limit(&self) -> usize {
        MAX_REMOTE_ENVELOPE_BYTES.saturating_add(1)
    }

    /// Authenticate, correlate, validate, and decode the reserved V2 reply.
    pub fn decode(self, response_bytes: &[u8]) -> Result<String, Diagnostic> {
        let outcome = self.decode_outcome(response_bytes)?;
        let outcome = serde_json::to_string(&outcome).map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_outcome_unserializable",
                "validated V2 remote outcome could not be serialized",
            )
        })?;
        Ok(outcome)
    }

    /// Authenticate, correlate, and validate one reply without serializing its
    /// typed outcome through a host-language representation.
    ///
    /// Model-oriented binding facades use this path so the authenticated
    /// hydration graph becomes an ordinary native validated match-result proof
    /// before any Python or JavaScript model constructor can run.
    pub fn decode_outcome(
        self,
        response_bytes: &[u8],
    ) -> Result<type_bridge_contract::query_remote_v2::RemoteOutcomeV2, Diagnostic> {
        ensure_remote_reply_live_v2(self.state.reply_deadline)?;
        let outcome = decode_remote_outcome_v2(
            response_bytes,
            &self.state.request_envelope,
            &self.state.validated,
            &self.state.request_fingerprint,
            &self.state.advertisement_fingerprint,
            self.state.trusted_key,
            self.state.limits,
        )?;
        ensure_remote_reply_live_v2(self.state.reply_deadline)?;
        Ok(outcome)
    }
}

/// Prepare one additive V2 invocation and its request-bound one-shot decoder.
pub fn prepare_remote_query_v2(
    authority: &QueryAuthority,
    plan_bytes: &[u8],
    invocation_json: &str,
    advertisement_bytes: &[u8],
    limits: RemoteLimitsV2,
) -> Result<PendingRemoteQueryV2, Diagnostic> {
    let (plan, validated) = authority.validate(plan_bytes)?;
    let invocation = parse_invocation(&plan, invocation_json)?;
    prepare_validated_remote_query_v2(
        authority,
        validated,
        invocation,
        advertisement_bytes,
        limits,
    )
}

/// Prepare one already schema-validated V2 invocation.
///
/// This crate-private seam exists for the production V1 model-query adapter.
/// It retains the same authority, capability, nonce, clock, limit, and
/// one-shot decoder boundary as [`prepare_remote_query_v2`] without encoding
/// and reparsing the adapted plan.
pub(crate) fn prepare_validated_remote_query_v2(
    authority: &QueryAuthority,
    validated: ValidatedQuery,
    invocation: QueryInvocation,
    advertisement_bytes: &[u8],
    limits: RemoteLimitsV2,
) -> Result<PendingRemoteQueryV2, Diagnostic> {
    if matches!(
        &authority.control_policy,
        QueryAuthorityControlPolicy::QueryOnly(_)
    ) {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_query_only_authority_local_only",
            "query-only authority is bound to local database identity and cannot prepare a V2 remote request",
        ));
    }
    if validated.source_state() != &authority.managed {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_authority_mismatch",
            "validated V2 query does not belong to the supplied schema authority",
        ));
    }
    let advertisement = RemoteCapabilities::decode(advertisement_bytes)?;
    check_advertised_capabilities_v2(&validated, &invocation, &advertisement)?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let monotonic_prepared = Instant::now();
    let prepared_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_clock_invalid",
                "system clock cannot establish an absolute V2 request time",
            )
        })?
        .as_millis()
        .try_into()
        .map_err(|_| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v2_clock_limit",
                "system clock exceeds the supported V2 timestamp range",
            )
        })?;
    let lifetime = limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS);
    let reply_deadline = monotonic_prepared
        .checked_add(Duration::from_millis(lifetime))
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v2_deadline_limit",
                "V2 remote deadline exceeds the maximum supported duration",
            )
        })?;
    let request = encode_remote_request_v2_at(
        &validated,
        &invocation,
        &advertisement,
        limits,
        &nonce,
        prepared_at_unix_ms,
    )?;
    let request_envelope = RemoteQueryRequestV2::decode(&request)?;
    let request_fingerprint = RemoteRequestFingerprintV2::compute(&request)?;
    let advertisement_fingerprint = advertisement.fingerprint()?;
    let trusted_key = advertisement.reply_key();
    Ok(PendingRemoteQueryV2 {
        state: Arc::new(PendingRemoteQueryStateV2 {
            advertisement_fingerprint,
            consumed: AtomicBool::new(false),
            limits: RemoteReplyDecodeLimitsV2 {
                max_bytes: limits.max_bytes,
                max_items: limits.max_items,
                max_collection_members: limits.max_collection_members,
                max_graph_nodes: limits.max_graph_nodes,
                max_attribute_values: limits.max_attribute_values,
                max_role_players: limits.max_role_players,
            },
            request,
            request_envelope,
            request_fingerprint,
            reply_deadline,
            trusted_key,
            validated,
        }),
    })
}

fn ensure_remote_reply_live(deadline: Instant) -> Result<(), Diagnostic> {
    if Instant::now() < deadline {
        return Ok(());
    }
    Err(failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_reply_expired",
        "the pending remote reply deadline has elapsed",
    ))
}

fn ensure_remote_reply_live_v2(deadline: Instant) -> Result<(), Diagnostic> {
    if Instant::now() < deadline {
        return Ok(());
    }
    Err(failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_reply_expired",
        "pending V2 remote reply deadline has elapsed",
    ))
}

/// Decode one capability advertisement into its sorted capability ids.
///
/// This is the typed client-side view of the `/v2/capabilities` bytes;
/// bindings surface it so callers can inspect exactly what an executor
/// advertises before preparing requests against it.
pub fn decode_remote_capabilities(advertisement_bytes: &[u8]) -> Result<Vec<String>, Diagnostic> {
    let advertisement = RemoteCapabilities::decode(advertisement_bytes)?;
    Ok(advertisement
        .capabilities()
        .iter()
        .map(|capability| capability.as_str().to_owned())
        .collect())
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
    use type_bridge_contract::id::{TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::{
        InputColumn, InputColumnId, QueryOutput, QueryPattern, ReadStage,
    };
    use type_bridge_contract::schema::{
        DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
        encode_declared_schema,
    };
    use type_bridge_schema_compat::MANAGED_FENCE_SCHEMA_TYPEQL;

    use super::*;
    use crate::error::OrmError;
    use crate::session::backend::{
        BoxFuture, DriverBackend, QueryResult, SchemaFencedReadTransaction, TransactionOps, TxType,
    };
    use crate::session::database::DatabaseConnectionAuthority;

    #[derive(Default)]
    struct Metrics {
        closes: AtomicUsize,
        opens: AtomicUsize,
        queries: AtomicUsize,
        schema_exports: AtomicUsize,
    }

    struct Backend {
        fail_close: bool,
        metrics: Arc<Metrics>,
        response: Mutex<Option<Result<QueryResult, OrmError>>>,
        schema_exports: Mutex<VecDeque<Result<String, OrmError>>>,
        stall_schema_export: bool,
    }

    impl DriverBackend for Backend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.metrics.opens.fetch_add(1, Ordering::SeqCst);
            let response = self
                .response
                .lock()
                .expect("prepared response lock")
                .take()
                .expect("one prepared transaction response");
            let transaction = MockTransaction {
                fail_close: self.fail_close,
                metrics: Arc::clone(&self.metrics),
                response: Some(response),
            };
            Box::pin(async move { Ok(Box::new(transaction) as Box<dyn TransactionOps>) })
        }

        fn open_schema_fenced_read_transaction(
            &self,
            database: &str,
            _timeout: Duration,
        ) -> BoxFuture<'_, Result<SchemaFencedReadTransaction, OrmError>> {
            let database = database.to_owned();
            Box::pin(async move {
                let transaction = self.open_transaction(&database, TxType::Write).await?;
                let schema_text = self.schema_text(&database).await?;
                Ok(SchemaFencedReadTransaction::new(transaction, schema_text))
            })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
            Some(type_bridge_core_lib::version::Version::new(3, 12, 1))
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            self.metrics.schema_exports.fetch_add(1, Ordering::SeqCst);
            if self.stall_schema_export {
                return Box::pin(std::future::pending());
            }
            let export = self
                .schema_exports
                .lock()
                .expect("prepared schema export lock")
                .pop_front()
                .expect("one prepared schema export");
            Box::pin(async move { export })
        }
    }

    struct MockTransaction {
        fail_close: bool,
        metrics: Arc<Metrics>,
        response: Option<Result<QueryResult, OrmError>>,
    }

    impl TransactionOps for MockTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.metrics.queries.fetch_add(1, Ordering::SeqCst);
            let response = self.response.take().expect("one prepared query");
            Box::pin(async move { response })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.metrics.closes.fetch_add(1, Ordering::SeqCst);
            let fail_close = self.fail_close;
            Box::pin(async move {
                if fail_close {
                    Err(OrmError::Transaction("masked close fixture".into()))
                } else {
                    Ok(())
                }
            })
        }
    }

    #[derive(Default)]
    struct ControlMetrics {
        closes: AtomicUsize,
        control_queries: AtomicUsize,
        fenced_opens: AtomicUsize,
        schema_exports: AtomicUsize,
        user_queries: AtomicUsize,
        query_texts: Mutex<Vec<String>>,
    }

    struct ControlBackend {
        advisory_schema: String,
        fenced_schema: String,
        metrics: Arc<ControlMetrics>,
        responses: Mutex<Option<VecDeque<Result<QueryResult, OrmError>>>>,
    }

    impl DriverBackend for ControlBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async {
                Err(OrmError::Transaction(
                    "managed prepared tests must use schema-fenced admission".into(),
                ))
            })
        }

        fn open_schema_fenced_read_transaction(
            &self,
            _database: &str,
            _timeout: Duration,
        ) -> BoxFuture<'_, Result<SchemaFencedReadTransaction, OrmError>> {
            self.metrics.fenced_opens.fetch_add(1, Ordering::SeqCst);
            let responses = self
                .responses
                .lock()
                .expect("control response lock")
                .take()
                .expect("one schema-fenced transaction script");
            let transaction = ControlTransaction {
                metrics: Arc::clone(&self.metrics),
                responses,
            };
            let schema_text = self.fenced_schema.clone();
            Box::pin(async move {
                Ok(SchemaFencedReadTransaction::new(
                    Box::new(transaction),
                    schema_text,
                ))
            })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
            Some(type_bridge_core_lib::version::Version::new(3, 12, 1))
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            self.metrics.schema_exports.fetch_add(1, Ordering::SeqCst);
            let schema = self.advisory_schema.clone();
            Box::pin(async move { Ok(schema) })
        }
    }

    struct ControlTransaction {
        metrics: Arc<ControlMetrics>,
        responses: VecDeque<Result<QueryResult, OrmError>>,
    }

    impl TransactionOps for ControlTransaction {
        fn query(&mut self, typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.metrics
                .query_texts
                .lock()
                .expect("control query text lock")
                .push(typeql.to_owned());
            if typeql.contains("typebridge-internal-v2-migration-control") {
                self.metrics.control_queries.fetch_add(1, Ordering::SeqCst);
            } else {
                self.metrics.user_queries.fetch_add(1, Ordering::SeqCst);
            }
            let response = self
                .responses
                .pop_front()
                .expect("one scripted response per prepared query");
            Box::pin(async move { response })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.metrics.closes.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct UnsupportedFenceBackend {
        metrics: Arc<Metrics>,
        schema: String,
        server_version: type_bridge_core_lib::version::Version,
    }

    impl DriverBackend for UnsupportedFenceBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.metrics.opens.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Err(OrmError::Transaction(
                    "ordinary transaction admission must not be used".into(),
                ))
            })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<type_bridge_core_lib::version::Version> {
            Some(self.server_version)
        }

        fn schema_text(&self, _database: &str) -> BoxFuture<'_, Result<String, OrmError>> {
            self.metrics.schema_exports.fetch_add(1, Ordering::SeqCst);
            let schema = self.schema.clone();
            Box::pin(async move { Ok(schema) })
        }
    }

    fn authority_and_plan() -> (QueryAuthority, Vec<u8>) {
        authority_and_plan_for_profile("typedb-3.12.1/v1")
    }

    fn authority_and_plan_for_profile(profile: &str) -> (QueryAuthority, Vec<u8>) {
        let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
        let declared = DeclaredSchema::from_facts(
            FormatVersion::V1,
            CapabilitySet::new(),
            [SourcedSchemaFact::new(
                SchemaFact::Type(TypeFact::new(person.clone()).expect("type fact")),
                SourceSpan::new(
                    DocumentId::new("prepared-close-fixture").expect("document"),
                    0,
                    1,
                    1,
                    1,
                    1,
                    2,
                )
                .expect("source span"),
            )],
        )
        .expect("declared schema");
        let authority = QueryAuthority::from_declared_bytes(
            &encode_declared_schema(&declared).expect("declared bytes"),
            "prepared-close-scope",
            profile,
        )
        .expect("query authority");
        let binding = BindingId::new(0).expect("binding");
        let plan = QueryPlan::new(
            vec![AssertionBinding::new(
                binding,
                QueryVariable::new("person").expect("variable"),
            )],
            Vec::new(),
            vec![ReadStage::Match {
                patterns: vec![QueryPattern::Isa {
                    binding,
                    include_subtypes: false,
                    type_id: person,
                }],
            }],
            QueryOutput::Rows {
                columns: vec![binding],
            },
            authority.managed.managed_semantic_schema().clone(),
        )
        .expect("query plan");
        (authority, plan.canonical_bytes().expect("plan bytes"))
    }

    fn authority_and_input_plan(
        value_type: type_bridge_contract::value::ValueTypeTag,
        optional: bool,
    ) -> (QueryAuthority, Vec<u8>) {
        let (authority, bytes) = authority_and_plan();
        let source = decode_query_plan(&bytes).expect("source plan");
        let plan = QueryPlan::new(
            source.bindings().to_vec(),
            vec![InputColumn::new(
                InputColumnId::new(0),
                QueryVariable::new("supplied").expect("input name"),
                value_type,
                optional,
            )],
            source.pipeline().to_vec(),
            source.output().clone(),
            source.managed_semantics().clone(),
        )
        .expect("input plan");
        (authority, plan.canonical_bytes().expect("plan bytes"))
    }

    fn database(
        response: Result<QueryResult, OrmError>,
        fail_close: bool,
    ) -> (Database, Arc<Metrics>) {
        database_with_exports(
            response,
            fail_close,
            [
                Ok("define entity person;".to_owned()),
                Ok("define entity person;".to_owned()),
            ],
        )
    }

    fn database_with_exports(
        response: Result<QueryResult, OrmError>,
        fail_close: bool,
        schema_exports: impl IntoIterator<Item = Result<String, OrmError>>,
    ) -> (Database, Arc<Metrics>) {
        let metrics = Arc::new(Metrics::default());
        let backend = Backend {
            fail_close,
            metrics: Arc::clone(&metrics),
            response: Mutex::new(Some(response)),
            schema_exports: Mutex::new(schema_exports.into_iter().collect()),
            stall_schema_export: false,
        };
        (
            Database::with_backend(Box::new(backend), "prepared-close"),
            metrics,
        )
    }

    fn query_only_authority(authority: &QueryAuthority, database: &Database) -> QueryAuthority {
        QueryAuthority::from_declared_bytes_query_only(
            &encode_declared_schema(&authority.declared).expect("declared bytes"),
            authority.delta_context.scope_id().as_str(),
            authority.delta_context.semantic_profile().as_str(),
            database,
        )
        .expect("query-only authority")
    }

    fn managed_schema_export() -> String {
        format!("{MANAGED_FENCE_SCHEMA_TYPEQL}\nentity person;")
    }

    fn managed_control_row(scope: &str, fence: &str, state: &str) -> serde_json::Value {
        serde_json::json!({
            "scope": scope,
            "fence": fence,
            "state": state,
        })
    }

    fn managed_database(
        responses: impl IntoIterator<Item = QueryResult>,
    ) -> (Database, Arc<ControlMetrics>) {
        let metrics = Arc::new(ControlMetrics::default());
        let schema = managed_schema_export();
        let backend = ControlBackend {
            advisory_schema: schema.clone(),
            fenced_schema: schema,
            metrics: Arc::clone(&metrics),
            responses: Mutex::new(Some(responses.into_iter().map(Ok).collect())),
        };
        (
            Database::with_backend(Box::new(backend), "prepared-managed"),
            metrics,
        )
    }

    fn database_with_execution_identity(
        name: &str,
        authority: DatabaseConnectionAuthority,
    ) -> (Database, Arc<Metrics>) {
        let metrics = Arc::new(Metrics::default());
        let backend = Backend {
            fail_close: false,
            metrics: Arc::clone(&metrics),
            response: Mutex::new(Some(Ok(QueryResult::Rows(Vec::new())))),
            schema_exports: Mutex::new(VecDeque::from([
                Ok("define entity person;".to_owned()),
                Ok("define entity person;".to_owned()),
            ])),
            stall_schema_export: false,
        };
        (
            Database::with_backend_authority(Box::new(backend), name, authority),
            metrics,
        )
    }

    #[test]
    fn prepared_invocation_byte_ceiling_precedes_json_allocation() {
        let (_, plan_bytes) = authority_and_plan();
        let plan = decode_query_plan(&plan_bytes).expect("prepared plan");
        let payload = r#"{"operation":"rows","rows":[]}"#;
        let mut at_limit = payload.to_owned();
        at_limit.push_str(&" ".repeat(StructuralLimits::CANONICAL.input_bytes - payload.len()));
        assert_eq!(at_limit.len(), StructuralLimits::CANONICAL.input_bytes);
        parse_invocation(&plan, &at_limit).expect("the exact byte ceiling remains admissible");

        let mut over_limit = at_limit;
        over_limit.push(' ');
        let oversized = parse_invocation(&plan, &over_limit)
            .expect_err("one byte over the ceiling must fail before parsing");
        assert_eq!(
            oversized.code().as_str(),
            "query_invocation_input_byte_limit"
        );
        assert_eq!(oversized.category(), DiagnosticCategory::ResourceLimit);
        assert_eq!(
            oversized.message(),
            "invocation input rows exceed the structural byte ceiling"
        );

        let malformed = parse_invocation(&plan, "{")
            .expect_err("bounded malformed JSON retains its canonical diagnostic");
        assert_eq!(
            malformed.code().as_str(),
            "query_prepared_invocation_malformed"
        );
        assert_eq!(malformed.category(), DiagnosticCategory::InvalidContract);
        assert_eq!(
            malformed.message(),
            "invocation payloads carry an operation and rectangular rows"
        );
    }

    #[test]
    fn prepared_parser_accepts_authored_canonical_invocations_without_changing_legacy_shape() {
        let (_, plan_bytes) = authority_and_plan();
        let plan = decode_query_plan(&plan_bytes).expect("prepared plan");
        let legacy = parse_invocation(&plan, r#"{"operation":"rows","rows":[]}"#)
            .expect("released invocation payload");
        let authored = QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new())
            .expect("authored invocation");
        let canonical = to_canonical_json(&authored).expect("canonical invocation");
        let parsed = parse_invocation(
            &plan,
            std::str::from_utf8(&canonical).expect("canonical UTF-8"),
        )
        .expect("authored canonical invocation");

        assert_eq!(parsed, authored);
        assert_eq!(parsed, legacy);

        let marker_in_legacy_value =
            r#"{"operation":"rows","rows":[],"note":"{\"inputs\":[],\"plan_fingerprint\":{}}"}"#;
        assert_eq!(
            parse_invocation(&plan, marker_in_legacy_value)
                .expect_err("authored markers inside a legacy value do not change dispatch")
                .code()
                .as_str(),
            "query_prepared_invocation_malformed",
        );

        let mut unknown =
            serde_json::from_slice::<serde_json::Value>(&canonical).expect("invocation value");
        unknown["extra"] = serde_json::json!(true);
        let unknown = to_canonical_json(&unknown).expect("canonical unknown field");
        assert_eq!(
            parse_invocation(
                &plan,
                std::str::from_utf8(&unknown).expect("unknown-field UTF-8"),
            )
            .expect_err("prepared boundary keeps unknown top-level fields generic")
            .code()
            .as_str(),
            "query_prepared_invocation_malformed",
        );

        let mut forged =
            serde_json::from_slice::<serde_json::Value>(&canonical).expect("invocation value");
        forged["plan_fingerprint"]["digest"] = serde_json::json!("00".repeat(32));
        let forged = to_canonical_json(&forged).expect("canonical fingerprint forgery");
        assert_eq!(
            parse_invocation(&plan, std::str::from_utf8(&forged).expect("forgery UTF-8"),)
                .expect_err("exact authored shape reaches plan binding verification")
                .code()
                .as_str(),
            "query_invocation_plan_fingerprint_mismatch",
        );
    }

    #[test]
    fn authored_invocation_wire_ceiling_is_admitted_and_one_byte_tight() {
        use type_bridge_contract::limits::{MAX_INPUT_BYTES, MAX_QUERY_INVOCATION_BYTES};
        use type_bridge_contract::value::CanonicalString;

        let (_, plan_bytes) =
            authority_and_input_plan(type_bridge_contract::value::ValueTypeTag::String, false);
        let plan = decode_query_plan(&plan_bytes).expect("prepared plan");
        let base_chunk = "x".repeat((MAX_INPUT_BYTES / 5).saturating_sub(128));
        let mut chunks = vec![base_chunk; 5];
        let build_rows = |chunks: &[String]| {
            chunks
                .iter()
                .map(|chunk| {
                    InputRow::new(vec![Some(CanonicalValue::String(
                        CanonicalString::new(chunk.clone()).expect("bounded string"),
                    ))])
                })
                .collect::<Vec<_>>()
        };
        let initial_size = serde_json::to_vec(&build_rows(&chunks))
            .expect("input rows")
            .len();
        assert!(initial_size < MAX_INPUT_BYTES);
        chunks
            .last_mut()
            .expect("last input chunk")
            .push_str(&"x".repeat(MAX_INPUT_BYTES - initial_size));
        let rows = build_rows(&chunks);
        assert_eq!(
            serde_json::to_vec(&rows).expect("exact input rows").len(),
            MAX_INPUT_BYTES,
        );

        let invocation = QueryInvocation::new(&plan, QueryOperation::Exists, rows)
            .expect("maximum-size invocation");
        let canonical = to_canonical_json(&invocation).expect("canonical invocation");
        assert_eq!(canonical.len(), MAX_QUERY_INVOCATION_BYTES);
        assert_eq!(
            parse_invocation(
                &plan,
                std::str::from_utf8(&canonical).expect("maximum invocation UTF-8"),
            )
            .expect("maximum authored invocation"),
            invocation,
        );

        let mut oversized = String::from_utf8(canonical).expect("canonical invocation is UTF-8");
        oversized.push(' ');
        let error = parse_invocation(&plan, &oversized)
            .expect_err("one byte over the complete wire ceiling");
        assert_eq!(error.code().as_str(), "query_invocation_input_byte_limit");
        assert_eq!(error.category(), DiagnosticCategory::ResourceLimit);
    }

    #[test]
    fn remote_reply_snapshot_is_one_byte_beyond_the_wire_ceiling_for_every_success_budget() {
        let (authority, plan) = authority_and_plan();
        let advertisement = RemoteCapabilities::new(
            type_bridge_contract::query_plan_capability_vocabulary(),
            type_bridge_contract::query_remote::RemoteExecutorBinding::new(
                "prepared-snapshot-executor",
                "prepared-snapshot-epoch-000001",
            )
            .expect("executor binding"),
            RemoteSigningPublicKey::from_bytes([0x17; 32]),
        )
        .encode()
        .expect("advertisement bytes");
        let wire_ceiling = u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).expect("wire ceiling");

        for caller_budget in [0, 7, wire_ceiling, wire_ceiling + 4_096] {
            let pending = prepare_remote_query(
                &authority,
                &plan,
                r#"{"operation":"rows","rows":[]}"#,
                &advertisement,
                RemoteLimits {
                    deadline_ms: None,
                    max_bytes: caller_budget,
                    max_items: 1,
                    max_collection_members: 1,
                },
            )
            .expect("remote request");
            let claimed = pending.claim_reply().expect("one reply claim");
            assert_eq!(
                claimed.response_snapshot_limit(),
                MAX_REMOTE_ENVELOPE_BYTES + 1
            );
        }
    }

    #[tokio::test]
    async fn prepared_invocation_byte_ceiling_is_identical_locally_and_remotely() {
        let (managed_authority, plan) = authority_and_plan();
        let oversized = " ".repeat(StructuralLimits::CANONICAL.input_bytes + 1);
        let (database, metrics) = database(Ok(QueryResult::Rows(Vec::new())), false);
        let local_authority = query_only_authority(&managed_authority, &database);

        let local = execute_prepared_local(
            &database,
            &local_authority,
            &plan,
            &oversized,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("local preparation rejects oversized invocation bytes");
        let remote = prepare_remote_query(
            &managed_authority,
            &plan,
            &oversized,
            b"advertisement decoding must not run",
            RemoteLimits {
                deadline_ms: None,
                max_bytes: 1,
                max_items: 1,
                max_collection_members: 1,
            },
        )
        .expect_err("remote preparation rejects oversized invocation bytes");

        assert_eq!(local.code().as_str(), "query_invocation_input_byte_limit");
        assert_eq!(remote.code(), local.code());
        assert_eq!(remote.category(), local.category());
        assert_eq!(remote.message(), local.message());
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn temporal_batches_pass_provider_independent_preflight() {
        use type_bridge_contract::value::ValueTypeTag;

        for (value_type, value) in [
            (
                ValueTypeTag::Date,
                serde_json::json!({"kind": "date", "value": "2026-07-13"}),
            ),
            (
                ValueTypeTag::DateTime,
                serde_json::json!({"kind": "datetime", "value": "2026-07-13T10:30:00"}),
            ),
            (
                ValueTypeTag::DateTimeTz,
                serde_json::json!({
                    "kind": "datetime_tz",
                    "value": {
                        "local": "2026-07-13T10:30:00",
                        "zone": {"kind": "offset_seconds", "seconds": 32400},
                        "effective_offset_seconds": 32400
                    }
                }),
            ),
            (
                ValueTypeTag::Duration,
                serde_json::json!({"kind": "duration", "value": "P1DT2S"}),
            ),
        ] {
            let (_, plan_bytes) = authority_and_input_plan(value_type, false);
            let plan = decode_query_plan(&plan_bytes).expect("plan");
            let invocation = serde_json::json!({
                "operation": "rows",
                "rows": [[value.clone()], [value]],
            })
            .to_string();
            let parsed = parse_invocation(&plan, &invocation)
                .unwrap_or_else(|error| panic!("{value_type:?} preflight failed: {error}"));
            assert!(
                parsed
                    .transport_capabilities()
                    .contains(&type_bridge_contract::query_given_rows_capability())
            );
        }
    }

    #[tokio::test]
    async fn single_datetime_tz_requires_given_before_local_or_remote_provider_work() {
        use type_bridge_contract::query_remote::RemoteExecutorBinding;
        use type_bridge_contract::value::ValueTypeTag;

        let (managed_authority, plan) = authority_and_input_plan(ValueTypeTag::DateTimeTz, false);
        let invocation = serde_json::json!({
            "operation": "rows",
            "rows": [[{
                "kind": "datetime_tz",
                "value": {
                    "local": "1900-01-01T12:00:00",
                    "zone": {"kind": "offset_seconds", "seconds": 1172},
                    "effective_offset_seconds": 1172
                }
            }]],
        })
        .to_string();
        let decoded_plan = decode_query_plan(&plan).expect("plan");
        assert!(
            parse_invocation(&decoded_plan, &invocation)
                .expect("datetime-tz invocation")
                .transport_capabilities()
                .contains(&type_bridge_contract::query_given_rows_capability())
        );

        let (database, metrics) = database(Ok(QueryResult::Rows(Vec::new())), false);
        let local_authority = query_only_authority(&managed_authority, &database);
        let local = execute_prepared_local(
            &database,
            &local_authority,
            &plan,
            &invocation,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("local no-given provider rejects before authority or transaction work");
        assert_eq!(
            local.code().as_str(),
            "query_v2_given_transport_unsupported"
        );
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);

        let advertisement = RemoteCapabilities::new(
            type_bridge_contract::query_plan_capability_vocabulary(),
            RemoteExecutorBinding::new(
                "prepared-no-given-executor",
                "prepared-no-given-epoch-000001",
            )
            .expect("executor binding"),
            RemoteSigningPublicKey::from_bytes([0x31; 32]),
        )
        .encode()
        .expect("no-given advertisement");
        let remote = prepare_remote_query(
            &managed_authority,
            &plan,
            &invocation,
            &advertisement,
            RemoteLimits {
                deadline_ms: None,
                max_bytes: 1,
                max_items: 1,
                max_collection_members: 1,
            },
        )
        .expect_err("remote no-given advertisement rejects before request construction");
        assert_eq!(
            remote.code().as_str(),
            "query_remote_capability_unsupported"
        );
    }

    #[tokio::test]
    async fn provider_invalid_duration_fails_locally_and_remotely_before_provider_work() {
        use type_bridge_contract::value::ValueTypeTag;

        let (managed_authority, plan) = authority_and_input_plan(ValueTypeTag::Duration, false);
        let invocation = serde_json::json!({
            "operation": "rows",
            "rows": [
                [{"kind": "duration", "value": "-P1D"}],
                [{"kind": "duration", "value": "P2DT3S"}]
            ],
        })
        .to_string();
        let (database, metrics) = database(Ok(QueryResult::Rows(Vec::new())), false);
        let local_authority = query_only_authority(&managed_authority, &database);

        let local = execute_prepared_local(
            &database,
            &local_authority,
            &plan,
            &invocation,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("provider-invalid duration must fail before local provider work");
        let remote = prepare_remote_query(
            &managed_authority,
            &plan,
            &invocation,
            b"advertisement decoding must not run",
            RemoteLimits {
                deadline_ms: None,
                max_bytes: 1,
                max_items: 1,
                max_collection_members: 1,
            },
        )
        .expect_err("provider-invalid duration must fail before request construction");

        assert_eq!(local.code().as_str(), "provider_duration_out_of_range");
        assert_eq!(remote.code(), local.code());
        assert_eq!(remote.category(), local.category());
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prepared_success_and_error_close_once_without_close_failure_replacement() {
        let invocation = r#"{"operation":"rows","rows":[]}"#;

        let (authority, plan) = authority_and_plan();
        let (success, success_metrics) = database(
            Ok(QueryResult::Rows(vec![serde_json::json!({
                "person": {"category": "entity", "label": "person", "iid": "0x01"}
            })])),
            true,
        );
        let authority = query_only_authority(&authority, &success);
        let outcome = execute_prepared_local(
            &success,
            &authority,
            &plan,
            invocation,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect("close failure must not replace prepared success");
        assert!(outcome.contains("0x01"), "{outcome}");
        assert_eq!(success_metrics.opens.load(Ordering::SeqCst), 1);
        assert_eq!(success_metrics.queries.load(Ordering::SeqCst), 1);
        assert_eq!(success_metrics.closes.load(Ordering::SeqCst), 1);
        assert_eq!(success_metrics.schema_exports.load(Ordering::SeqCst), 2);

        let (authority, plan) = authority_and_plan();
        let (failure, failure_metrics) = database(
            Err(OrmError::QueryExecution("masked provider fixture".into())),
            true,
        );
        let authority = query_only_authority(&authority, &failure);
        let diagnostic = execute_prepared_local(
            &failure,
            &authority,
            &plan,
            invocation,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("provider failure remains the primary diagnostic");
        assert_eq!(diagnostic.code().as_str(), "query_prepared_provider_failed");
        assert_eq!(failure_metrics.opens.load(Ordering::SeqCst), 1);
        assert_eq!(failure_metrics.queries.load(Ordering::SeqCst), 1);
        assert_eq!(failure_metrics.closes.load(Ordering::SeqCst), 1);
        assert_eq!(failure_metrics.schema_exports.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn prepared_local_rejects_stale_authority_before_opening_transaction() {
        let (authority, plan) = authority_and_plan();
        let (database, metrics) = database_with_exports(
            Ok(QueryResult::Rows(Vec::new())),
            false,
            [Ok("define entity animal;".to_owned())],
        );
        let authority = query_only_authority(&authority, &database);
        let diagnostic = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("foreign live schema must fail before transaction admission");
        assert_eq!(diagnostic.code().as_str(), "query_prepared_stale_schema");
        assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.closes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prepared_local_rechecks_authority_after_open_and_closes_on_drift() {
        let (authority, plan) = authority_and_plan();
        let (database, metrics) = database_with_exports(
            Ok(QueryResult::Rows(Vec::new())),
            false,
            [
                Ok("define entity person;".to_owned()),
                Ok("define entity animal;".to_owned()),
            ],
        );
        let authority = query_only_authority(&authority, &database);
        let diagnostic = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("schema drift after transaction open must stop execution");
        assert_eq!(diagnostic.code().as_str(), "query_prepared_stale_schema");
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 2);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.closes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prepared_local_masks_untrusted_export_detail_before_open() {
        let (authority, plan) = authority_and_plan();
        let (database, metrics) = database_with_exports(
            Ok(QueryResult::Rows(Vec::new())),
            false,
            [Ok("this is not TypeQL".to_owned())],
        );
        let authority = query_only_authority(&authority, &database);
        let diagnostic = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("malformed live export must fail closed");
        assert_eq!(
            diagnostic.code().as_str(),
            "query_prepared_live_schema_invalid"
        );
        assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prepared_local_deadline_bounds_live_authority_preparation() {
        let (authority, plan) = authority_and_plan();
        let metrics = Arc::new(Metrics::default());
        let database = Database::with_backend(
            Box::new(Backend {
                fail_close: false,
                metrics: Arc::clone(&metrics),
                response: Mutex::new(Some(Ok(QueryResult::Rows(Vec::new())))),
                schema_exports: Mutex::new(VecDeque::new()),
                stall_schema_export: true,
            }),
            "prepared-live-authority-timeout",
        );
        let authority = query_only_authority(&authority, &database);
        let diagnostic = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits {
                answer: crate::session::backend::BoundedAnswerLimits {
                    deadline: Some(std::time::Instant::now() + Duration::from_millis(10)),
                    ..crate::session::backend::BoundedAnswerLimits::default()
                },
                ..QueryV2AnswerLimits::default()
            },
        )
        .await
        .expect_err("live authority export must obey the execution deadline");
        assert_eq!(diagnostic.code().as_str(), "transaction_deadline_exceeded");
        // The same absolute deadline covers admission to the process-wide
        // rebuild limiter. Under parallel load it may expire before the
        // backend export starts; either valid boundary must precede opening a
        // provider transaction.
        assert!(metrics.schema_exports.load(Ordering::SeqCst) <= 1);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn managed_control_rejections_are_bounded_and_precede_user_query() {
        let exact = managed_control_row("prepared-close-scope", "1", "free");
        let cases = vec![
            (
                "missing",
                vec![QueryResult::Documents(Vec::new())],
                "query_prepared_control_invalid",
                1,
            ),
            (
                "duplicate",
                vec![QueryResult::Documents(vec![exact.clone(), exact.clone()])],
                "query_prepared_control_invalid",
                1,
            ),
            (
                "foreign scope",
                vec![QueryResult::Documents(vec![managed_control_row(
                    "foreign-scope",
                    "1",
                    "free",
                )])],
                "query_prepared_managed_scope_mismatch",
                1,
            ),
            (
                "zero fence",
                vec![QueryResult::Documents(vec![managed_control_row(
                    "prepared-close-scope",
                    "0",
                    "free",
                )])],
                "query_prepared_control_invalid",
                1,
            ),
            (
                "held",
                vec![QueryResult::Documents(vec![managed_control_row(
                    "prepared-close-scope",
                    "1",
                    "held",
                )])],
                "query_prepared_migration_in_progress",
                1,
            ),
            (
                "free with holder",
                vec![
                    QueryResult::Documents(vec![exact]),
                    QueryResult::Documents(vec![serde_json::json!({"holder": "runner"})]),
                ],
                "query_prepared_migration_in_progress",
                2,
            ),
            (
                "non-document control result",
                vec![QueryResult::Rows(Vec::new())],
                "query_prepared_control_invalid",
                1,
            ),
        ];

        for (name, responses, expected_code, expected_control_queries) in cases {
            let (authority, plan) = authority_and_plan();
            let (database, metrics) = managed_database(responses);
            let diagnostic = execute_prepared_local(
                &database,
                &authority,
                &plan,
                r#"{"operation":"rows","rows":[]}"#,
                QueryV2AnswerLimits::default(),
            )
            .await
            .expect_err(name);

            assert_eq!(diagnostic.code().as_str(), expected_code, "{name}");
            assert_eq!(
                diagnostic.category(),
                DiagnosticCategory::Integrity,
                "{name}"
            );
            assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 1, "{name}");
            assert_eq!(metrics.fenced_opens.load(Ordering::SeqCst), 1, "{name}");
            assert_eq!(
                metrics.control_queries.load(Ordering::SeqCst),
                expected_control_queries,
                "{name}",
            );
            assert_eq!(metrics.user_queries.load(Ordering::SeqCst), 0, "{name}");
            assert_eq!(metrics.closes.load(Ordering::SeqCst), 1, "{name}");

            let queries = metrics.query_texts.lock().expect("control query text lock");
            assert!(queries[0].contains("limit 2;"), "{name}: {}", queries[0]);
            if expected_control_queries == 2 {
                assert!(queries[1].contains("limit 1;"), "{name}: {}", queries[1]);
            }
        }
    }

    #[tokio::test]
    async fn exact_free_managed_control_executes_after_two_bounded_document_reads() {
        let (authority, plan) = authority_and_plan();
        let (database, metrics) = managed_database([
            QueryResult::Documents(vec![managed_control_row(
                "prepared-close-scope",
                "42",
                "free",
            )]),
            QueryResult::Documents(Vec::new()),
            QueryResult::Rows(vec![serde_json::json!({
                "person": {"category": "entity", "label": "person", "iid": "0x2a"}
            })]),
        ]);

        let outcome = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect("an exact free control row admits the user query");

        assert!(outcome.contains("0x2a"), "{outcome}");
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.fenced_opens.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.control_queries.load(Ordering::SeqCst), 2);
        assert_eq!(metrics.user_queries.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.closes.load(Ordering::SeqCst), 1);
        let queries = metrics.query_texts.lock().expect("control query text lock");
        assert!(queries[0].contains("limit 2;"), "{}", queries[0]);
        assert!(queries[1].contains("limit 1;"), "{}", queries[1]);
        assert!(!queries[2].contains("typebridge-internal-v2-migration-control"));
    }

    #[tokio::test]
    async fn prepared_local_rejects_exact_semantic_profile_mismatch_before_provider_work() {
        let (authority, plan) = authority_and_plan();
        let metrics = Arc::new(Metrics::default());
        let database = Database::with_backend(
            Box::new(UnsupportedFenceBackend {
                metrics: Arc::clone(&metrics),
                schema: "define entity person;".to_owned(),
                server_version: type_bridge_core_lib::version::Version::new(3, 12, 0),
            }),
            "prepared-profile-mismatch",
        );

        let diagnostic = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("3.12.0 authority must not execute against a 3.12.1 provider");

        assert_eq!(
            diagnostic.code().as_str(),
            "query_prepared_semantic_profile_mismatch"
        );
        assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn query_only_authority_rejects_a_different_database_on_the_same_provider() {
        let (authority, plan) = authority_and_plan();
        let shared_provider = DatabaseConnectionAuthority::isolated();
        let (source, _) =
            database_with_execution_identity("prepared-source", shared_provider.clone());
        let (target, metrics) =
            database_with_execution_identity("prepared-target", shared_provider);
        let authority = query_only_authority(&authority, &source);

        let diagnostic = execute_prepared_local(
            &target,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("query-only authority must bind the exact database name");

        assert_eq!(
            diagnostic.code().as_str(),
            "query_prepared_database_identity_mismatch"
        );
        assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn default_backend_rejects_schema_fenced_execution_without_fallback() {
        let (authority, plan) = authority_and_plan();
        let metrics = Arc::new(Metrics::default());
        let database = Database::with_backend(
            Box::new(UnsupportedFenceBackend {
                metrics: Arc::clone(&metrics),
                schema: "define entity person;".to_owned(),
                server_version: type_bridge_core_lib::version::Version::new(3, 12, 1),
            }),
            "prepared-unsupported-fence",
        );
        let authority = query_only_authority(&authority, &database);

        let diagnostic = execute_prepared_local(
            &database,
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            QueryV2AnswerLimits::default(),
        )
        .await
        .expect_err("the default backend must fail closed at fenced admission");

        assert_eq!(
            diagnostic.code().as_str(),
            "query_prepared_transaction_failed"
        );
        assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 1);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.closes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn query_only_authority_cannot_prepare_a_remote_request() {
        let (authority, plan) = authority_and_plan();
        let (database, metrics) = database(Ok(QueryResult::Rows(Vec::new())), false);
        let authority = query_only_authority(&authority, &database);

        let diagnostic = prepare_remote_query(
            &authority,
            &plan,
            r#"{"operation":"rows","rows":[]}"#,
            b"advertisement decoding must not run",
            RemoteLimits {
                deadline_ms: None,
                max_bytes: 1 << 20,
                max_items: 100,
                max_collection_members: 1 << 16,
            },
        )
        .expect_err("query-only authority must remain local-only");

        assert_eq!(
            diagnostic.code().as_str(),
            "query_remote_query_only_authority_local_only"
        );
        assert_eq!(diagnostic.category(), DiagnosticCategory::Integrity);
        assert_eq!(metrics.schema_exports.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.opens.load(Ordering::SeqCst), 0);
        assert_eq!(metrics.queries.load(Ordering::SeqCst), 0);
    }
}
