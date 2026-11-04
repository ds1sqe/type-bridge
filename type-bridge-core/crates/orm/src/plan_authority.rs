//! Schema authority and identity shared by generated queries and staged plans.

use crate::plan_lowering::failure;
use crate::session::database::{Database, DatabaseExecutionIdentity};
use std::fmt;
use std::sync::{Arc, OnceLock};
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
