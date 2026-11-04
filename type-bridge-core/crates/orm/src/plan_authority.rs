//! Schema authority and identity shared by generated queries and staged plans.

use crate::OwnedTransaction as Transaction;
use crate::plan_lowering::ast;
use crate::plan_lowering::failure;
use crate::session::backend::QueryResult;
use crate::session::database::{Database, DatabaseExecutionIdentity};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, LazyLock, OnceLock};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::{DeclaredSchema, decode_declared_schema};
use type_bridge_contract::schema_delta::ManagedSchemaState;
use type_bridge_core_lib::__internal as tql;
use type_bridge_query::MigrationAssertionValidationContext;
use type_bridge_schema::{ManagedDeltaContext, ResolvedSchema};

/// Opaque identity of one exact [`QueryAuthority`].
///
/// Equality is allocation identity, not schema-value equality: separately
/// constructed authorities remain distinct even when their canonical schema
/// bytes are equal.
#[derive(Clone)]
pub struct QueryAuthorityIdentity(Arc<()>);

impl QueryAuthorityIdentity {
    pub(crate) fn fresh() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for QueryAuthorityIdentity {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for QueryAuthorityIdentity {}

impl fmt::Debug for QueryAuthorityIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QueryAuthorityIdentity([OPAQUE])")
    }
}

/// One owned schema authority prepared plans validate against.
pub struct QueryAuthority {
    pub(crate) declared: DeclaredSchema,
    pub(crate) delta_context: ManagedDeltaContext,
    identity: QueryAuthorityIdentity,
    limits: QueryLimits,
    pub(crate) managed: ManagedSchemaState,
    resolved: ResolvedSchema,
    pub(crate) control_policy: QueryAuthorityControlPolicy,
    adapter_authority: OnceLock<Arc<crate::_registry::InstalledSchemaAuthority>>,
}

#[derive(Clone)]
pub(crate) enum QueryAuthorityControlPolicy {
    Managed,
    QueryOnly(DatabaseExecutionIdentity),
}

impl QueryAuthority {
    pub(crate) fn adapter_authority(&self) -> Arc<crate::_registry::InstalledSchemaAuthority> {
        Arc::clone(self.adapter_authority.get_or_init(|| {
            Arc::new(crate::_registry::InstalledSchemaAuthority::new(
                self.declared.clone(),
                self.resolved.clone(),
                self.managed.clone(),
            ))
        }))
    }

    pub(crate) fn scope_id(&self) -> &ManagedScopeId {
        self.delta_context.scope_id()
    }

    /// The declared schema plans are decoded and validated against.
    #[must_use]
    pub fn declared(&self) -> &DeclaredSchema {
        &self.declared
    }
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

    /// Test whether this authority resolves to one exact semantic schema
    /// fingerprint.
    #[doc(hidden)]
    pub fn matches_semantic_fingerprint(
        &self,
        fingerprint: &type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint,
    ) -> bool {
        self.resolved.semantic_fingerprint() == fingerprint
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
            limits: QueryLimits::CANONICAL,
            managed,
            resolved,
            control_policy,
            adapter_authority: OnceLock::new(),
        })
    }

    /// Author, decode, and validate every plan of this authority under
    /// `limits` instead of the canonical defaults.
    ///
    /// The caller admits untrusted values with [`QueryLimits::validate`]
    /// first.
    #[must_use]
    pub fn with_limits(mut self, limits: QueryLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Derive a distinct authority over the same schema that holds its plans
    /// to `limits`.
    ///
    /// The result has a fresh identity: handles and plans authored under one
    /// limits policy cannot be replayed under another.
    #[must_use]
    pub fn rebound_with_limits(&self, limits: QueryLimits) -> Self {
        Self {
            declared: self.declared.clone(),
            delta_context: self.delta_context.clone(),
            identity: QueryAuthorityIdentity::fresh(),
            limits,
            managed: self.managed.clone(),
            resolved: self.resolved.clone(),
            control_policy: self.control_policy.clone(),
            adapter_authority: self.adapter_authority.clone(),
        }
    }

    /// Return the limits plans of this authority are held to.
    #[must_use]
    pub const fn limits(&self) -> QueryLimits {
        self.limits
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
}

fn schema_rejected() -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_prepared_schema_rejected",
        "the declared schema does not resolve under this profile",
    )
}

const LIVE_AUTHORITY_REBUILD_CONCURRENCY: usize = 4;

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
#[allow(clippy::result_unit_err)]
pub async fn acquire_live_authority_rebuild_permit() -> Result<tokio::sync::OwnedSemaphorePermit, ()>
{
    Arc::clone(&LIVE_AUTHORITY_REBUILD_SLOTS)
        .acquire_owned()
        .await
        .map_err(|_| ())
}

fn cancelled() -> Diagnostic {
    failure(
        DiagnosticCategory::Cancelled,
        "provider_cancelled",
        "provider answer processing was cancelled",
    )
}

pub(crate) fn check_authority_cancellation(
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<(), Diagnostic> {
    if cancellation.is_cancelled() {
        Err(cancelled())
    } else {
        Ok(())
    }
}

pub(crate) async fn await_authority_stage<F>(
    future: F,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<F::Output, Diagnostic>
where
    F: Future,
{
    check_authority_cancellation(cancellation)?;
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
    check_authority_cancellation(cancellation)?;
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err(deadline_exceeded());
    }
    Ok(output)
}

/// Verify the one global managed control row inside the same schema-fenced
/// transaction that will execute a plan.
#[doc(hidden)]
pub async fn verify_managed_query_control_scope(
    transaction: &mut Transaction,
    expected_scope: &ManagedScopeId,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<(), Diagnostic> {
    let control_query = managed_control_query()?;
    let documents =
        managed_control_documents(transaction, &control_query, deadline, cancellation).await?;
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
    let result = await_authority_stage(query, deadline, cancellation)
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

pub(crate) fn deadline_exceeded() -> Diagnostic {
    failure(
        DiagnosticCategory::ResourceLimit,
        "transaction_deadline_exceeded",
        "provider transaction deadline expired",
    )
}

fn managed_control_query() -> Result<String, Diagnostic> {
    let mut constraints = vec![ast(tql::isa(
        "typebridge-internal-v2-migration-control",
        false,
    ))?];
    for (attribute, variable) in [
        ("typebridge-internal-v2-control-scope", "scope"),
        ("typebridge-internal-v2-lease-fence", "fence"),
        ("typebridge-internal-v2-lease-state", "state"),
    ] {
        constraints.push(ast(tql::has(attribute, ast(tql::var(variable))?))?);
    }
    let fields = ["scope", "fence", "state"]
        .into_iter()
        .map(|name| Ok((name, tql::fetch_expr(ast(tql::var(name))?))))
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    ast(tql::render(&tql::pipeline(vec![
        tql::match_stage(vec![ast(tql::thing("control", constraints))?]),
        tql::limit(2),
        ast(tql::fetch(fields))?,
    ])))
}

#[test]
fn managed_control_query_preserves_the_released_read() {
    let old = "match $control isa typebridge-internal-v2-migration-control, has typebridge-internal-v2-control-scope $scope, has typebridge-internal-v2-lease-fence $fence, has typebridge-internal-v2-lease-state $state; limit 2; fetch { \"scope\": $scope, \"fence\": $fence, \"state\": $state };";
    let canonical = |source: &str| typeql::parse_query(source).unwrap().to_string();
    assert_eq!(
        typeql::parse_query(&canonical(old)).unwrap(),
        typeql::parse_query(&canonical(&managed_control_query().unwrap())).unwrap()
    );
}
