//! Schema authority and identity shared by generated queries and staged plans.

use crate::OwnedTransaction as Transaction;
use crate::plan_lowering::failure;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, LazyLock, OnceLock};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::limits::QueryLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::schema::{DeclaredSchema, decode_declared_schema};
use type_bridge_contract::schema_delta::ManagedSchemaState;
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
    adapter_authority: OnceLock<Arc<crate::_registry::InstalledSchemaAuthority>>,
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
    /// Test whether this authority resolves to one exact semantic schema
    /// fingerprint.
    #[doc(hidden)]
    pub fn matches_semantic_fingerprint(
        &self,
        fingerprint: &type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint,
    ) -> bool {
        self.resolved.semantic_fingerprint() == fingerprint
    }

    /// Build an authority from canonical declared-schema bytes.
    pub fn from_declared_bytes(
        bytes: &[u8],
        scope: &str,
        profile: &str,
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
    let plans = managed_control_plans()?;
    let answer = managed_control_answer(transaction, &plans[0], deadline, cancellation).await?;
    let documents = managed_control_documents(&answer, &plans[0])?;
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
    let holder = managed_control_answer(transaction, &plans[1], deadline, cancellation).await?;
    let crate::session::staged::PlanAnswer::Exists(held) = holder else {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor managed holder authority returned an invalid answer",
        ));
    };
    if held {
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

async fn managed_control_answer(
    transaction: &mut Transaction,
    plan: &type_bridge_contract::query_plan::QueryPlan,
    deadline: Option<std::time::Instant>,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> Result<crate::session::staged::PlanAnswer, Diagnostic> {
    let policy = crate::QueryExecutionResourceLimits::default();
    let plan_deadline = deadline.map_or_else(
        || crate::QueryExecutionDeadline::from_timeout_milliseconds(0),
        crate::QueryExecutionDeadline::from_instant,
    );
    let query = transaction.execute_plans(
        std::slice::from_ref(plan),
        &policy,
        cancellation,
        plan_deadline,
    );
    let result = await_authority_stage(query, deadline, cancellation)
        .await?
        .map_err(|error| {
            if matches!(&error, crate::OrmError::Match(error) if matches!(error.category(), crate::match_request::MatchErrorCategory::ResultDecode)) {
                return failure(DiagnosticCategory::Integrity, "query_prepared_control_invalid", "the executor managed control row is malformed");
            }
            failure(
                DiagnosticCategory::Integrity,
                "query_prepared_control_unavailable",
                "the executor could not read managed control authority",
            )
        })?;
    result.into_iter().next().ok_or_else(|| {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor managed control returned no answer",
        )
    })
}

fn managed_control_documents(
    answer: &crate::session::staged::PlanAnswer,
    plan: &type_bridge_contract::query_plan::QueryPlan,
) -> Result<Vec<serde_json::Value>, Diagnostic> {
    let invalid = || {
        failure(
            DiagnosticCategory::Integrity,
            "query_prepared_control_invalid",
            "the executor managed control row is malformed",
        )
    };
    answer
        .scalar_rows(plan)
        .map_err(|_| invalid())?
        .into_iter()
        .map(|row| {
            let fields = plan.bindings()[1..]
                .iter()
                .zip(row)
                .map(|(binding, value)| {
                    let type_bridge_contract::value::CanonicalValue::String(value) = value else {
                        return Err(invalid());
                    };
                    Ok((
                        binding.variable().as_str().to_owned(),
                        serde_json::Value::String(value.as_str().to_owned()),
                    ))
                })
                .collect::<Result<serde_json::Map<_, _>, Diagnostic>>()?;
            Ok(serde_json::Value::Object(fields))
        })
        .collect()
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

fn managed_control_plans()
-> Result<&'static [type_bridge_contract::query_plan::QueryPlan; 2], Diagnostic> {
    use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
    use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
    use type_bridge_contract::query_plan::{Operation, PlanSpec, QueryPlan, Stage};
    use type_bridge_contract::query_plan::{OrderDirection, OrderTerm, QueryOutput, QueryPattern};
    use type_bridge_contract::reserved::{
        MANAGED_CONTROL_ENTITY, MANAGED_CONTROL_LEASE_FENCE, MANAGED_CONTROL_LEASE_HOLDER,
        MANAGED_CONTROL_LEASE_STATE, MANAGED_CONTROL_SCOPE,
    };
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
    use type_bridge_schema_compat::MANAGED_FENCE_SCHEMA_TYPEQL;

    static PLANS: LazyLock<Result<[QueryPlan; 2], Diagnostic>> = LazyLock::new(|| {
        let schema = type_bridge_schema_compat::typeql_to_declared(
            DocumentId::new("managed-control-plans.typeql")?,
            MANAGED_FENCE_SCHEMA_TYPEQL,
        )
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "query_prepared_control_invalid",
                "the frozen managed control schema is invalid",
            )
        })?;
        let semantics = ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1")?,
            MANAGED_FENCE_SCHEMA_TYPEQL.as_bytes(),
        )?;
        let read = |columns: &[(&str, &str)], limit: Option<u64>| {
            let root = BindingId::new(0)?;
            let mut bindings = vec![AssertionBinding::new(root, QueryVariable::new("control")?)];
            let mut patterns = vec![
                QueryPattern::Isa {
                    binding: root,
                    type_id: TypeId::new(TypeKind::Entity, MANAGED_CONTROL_ENTITY)?,
                    include_subtypes: true,
                }
                .into(),
            ];
            let mut output = Vec::new();
            for (index, (attribute, alias)) in columns.iter().enumerate() {
                let binding = BindingId::new((index + 1) as u16)?;
                bindings.push(AssertionBinding::new(binding, QueryVariable::new(*alias)?));
                patterns.push(
                    QueryPattern::Has {
                        owner: root,
                        attribute: binding,
                        attribute_id: AttributeId::new(*attribute)?,
                    }
                    .into(),
                );
                output.push(binding);
            }
            let mut pipeline = vec![Stage::Match { patterns }];
            if let Some(rows) = limit {
                // Native truncation requires ordering. Sorting scope preserves the
                // duplicate-row check; a valid control table has only one row.
                pipeline.push(Stage::Sort {
                    terms: vec![OrderTerm::new(
                        BindingId::new(1)?,
                        OrderDirection::Ascending,
                    )],
                });
                pipeline.push(Stage::Limit { rows });
            }
            QueryPlan::new(
                PlanSpec {
                    bindings,
                    inputs: vec![],
                    given_rows: None,
                    operation: if limit.is_some() {
                        Operation::Rows
                    } else {
                        Operation::Exists
                    },
                    pipeline,
                    output: QueryOutput::Rows { columns: output },
                    managed_semantics: semantics.clone(),
                    functions: vec![],
                },
                &schema,
            )
        };
        Ok([
            read(
                &[
                    (MANAGED_CONTROL_SCOPE, "scope"),
                    (MANAGED_CONTROL_LEASE_FENCE, "fence"),
                    (MANAGED_CONTROL_LEASE_STATE, "state"),
                ],
                Some(2),
            )?,
            read(&[(MANAGED_CONTROL_LEASE_HOLDER, "holder")], None)?,
        ])
    });
    PLANS.as_ref().map_err(Clone::clone)
}

#[cfg(test)]
mod tests;
