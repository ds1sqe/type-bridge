//! Additive V2 remote-query envelopes and hydrated model evidence.
//!
//! This module deliberately does not widen any V1 DTO. V2 requests, replies,
//! failures, fingerprints, and hydration graphs have distinct Rust types and
//! exact format discriminators. The authenticated outer reply and executor
//! advertisement remain the existing open V1 containers.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::{
    from_canonical_json_with_limits, to_canonical_json, to_canonical_json_with_limits,
};
use crate::diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticCode, DiagnosticDetailValue, DiagnosticPath,
    DiagnosticPathSegment,
};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDigest, FingerprintDomain,
};
use crate::id::{AttributeId, RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use crate::limits::{
    MAX_CANONICAL_COLLECTION_LEN, MAX_DIAGNOSTIC_BYTES, MAX_REMOTE_ENVELOPE_BYTES,
    MAX_SELECTED_SLOTS, REMOTE_ENVELOPE_CODEC_LIMITS, REMOTE_REQUEST_CODEC_LIMITS,
};
use crate::migration_assertion::BindingId;
use crate::query_plan::{
    CompatibilityValueV2, InputRow, QueryInvocation, QueryOperation, QueryOutput, QueryPlan,
    QueryPlanFingerprint, decode_query_plan,
};
use crate::query_remote::{
    DEFAULT_REMOTE_DEADLINE_MS, MAX_REMOTE_CLOCK_SKEW_MS, MAX_REMOTE_DEADLINE_MS,
    QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN, RemoteCapabilities, RemoteCapabilitiesFingerprint,
    RemoteFieldValue, RemoteReplySigner, RemoteReplyVerifier, RemoteSigningPublicKey, RemoteValue,
};
use crate::value::CanonicalValue;

/// The exact wire discriminator for additive V2 remote requests.
pub const QUERY_REMOTE_REQUEST_FORMAT_V2: &str = "typebridge.query-remote-request/v2";
/// The exact wire discriminator for additive V2 remote responses.
pub const QUERY_REMOTE_RESPONSE_FORMAT_V2: &str = "typebridge.query-remote-response/v2";
/// The exact wire discriminator for additive V2 remote failures.
pub const QUERY_REMOTE_FAILURE_FORMAT_V2: &str = "typebridge.query-remote-failure/v2";
/// The distinct canonicalization identifier for V2 request fingerprints.
pub const QUERY_REMOTE_REQUEST_CANONICALIZATION_V2: &str = "typebridge.query-remote-request/v2";

/// V2-plan admission capability.
pub const CAP_QUERY_PLAN_V2: &str = "query.plan.v2";
/// V2 remote-envelope admission capability.
pub const CAP_QUERY_REMOTE_ENVELOPE_V2: &str = "query.remote.envelope-v2";
/// Complete structured remote-diagnostic admission capability.
pub const CAP_QUERY_REMOTE_STRUCTURED_DIAGNOSTIC: &str = "query.remote.structured-diagnostic";
/// Hydrated model-output admission capability.
pub const CAP_QUERY_OUTPUT_HYDRATED: &str = "query.output.hydrated";
/// Same-snapshot hydration admission capability.
pub const CAP_QUERY_SAME_SNAPSHOT_HYDRATION: &str = "query.execution.same-snapshot-hydration";

const NONCE_MIN_BYTES: usize = 16;
const NONCE_MAX_BYTES: usize = 128;
const FAILURE_FRAMING_ALLOWANCE_BYTES: usize = 1_024;

/// Return the exact base capabilities required by every V2 remote envelope.
#[must_use]
pub fn query_remote_v2_required_capabilities(carries_hydration_graph: bool) -> CapabilitySet {
    let mut capabilities = [
        CAP_QUERY_PLAN_V2,
        CAP_QUERY_REMOTE_ENVELOPE_V2,
        CAP_QUERY_REMOTE_STRUCTURED_DIAGNOSTIC,
    ]
    .into_iter()
    .map(static_capability)
    .collect::<CapabilitySet>();
    if carries_hydration_graph {
        capabilities.insert(static_capability(CAP_QUERY_OUTPUT_HYDRATED));
        capabilities.insert(static_capability(CAP_QUERY_SAME_SNAPSHOT_HYDRATION));
    }
    capabilities
}

fn static_capability(value: &'static str) -> CapabilityId {
    CapabilityId::new(value).expect("static V2 remote capability is canonical")
}

/// The response family bound into a V2 request fingerprint.
///
/// The complete model output, hydration projection, ordering, cardinality,
/// page, and root contract lives in the embedded V2 plan. This discriminator
/// prevents a valid plan/request from being replayed under another terminal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteResultKindV2 {
    /// Low-level positional rows.
    Rows,
    /// Low-level fetched documents.
    Documents,
    /// Low-level scalar count.
    Count,
    /// Low-level scalar existence.
    Exists,
    /// Model-oriented hydrated rows, including the exactly-one contract.
    HydratedRows,
    /// Model-oriented distinct-root page.
    HydratedPage,
    /// Model-oriented distinct-root count.
    DistinctCount,
    /// Model-oriented distinct-root existence.
    DistinctExists,
}

impl RemoteResultKindV2 {
    const fn operation(self) -> QueryOperation {
        match self {
            Self::Rows | Self::Documents | Self::HydratedRows | Self::HydratedPage => {
                QueryOperation::Rows
            }
            Self::Count | Self::DistinctCount => QueryOperation::Count,
            Self::Exists | Self::DistinctExists => QueryOperation::Exists,
        }
    }

    const fn compatibility_terminal(self) -> bool {
        matches!(
            self,
            Self::HydratedRows | Self::HydratedPage | Self::DistinctCount | Self::DistinctExists
        )
    }

    const fn carries_hydration_graph(self) -> bool {
        matches!(self, Self::HydratedRows | Self::HydratedPage)
    }
}

/// Caller-owned V2 transport and hydration budgets bound into one request.
///
/// These limits are additive rather than widening [`crate::query_remote::RemoteLimits`];
/// V1 request bytes and APIs remain exact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteLimitsV2 {
    /// Optional execution deadline in milliseconds.
    pub deadline_ms: Option<u64>,
    /// Maximum authenticated successful reply bytes.
    pub max_bytes: u64,
    /// Maximum rows, page entries, or scalar evidence.
    pub max_items: u64,
    /// Maximum aggregate output collection members.
    pub max_collection_members: u64,
    /// Maximum hydration graph nodes.
    pub max_graph_nodes: u64,
    /// Maximum aggregate hydration attribute values.
    pub max_attribute_values: u64,
    /// Maximum aggregate hydration role-player references.
    pub max_role_players: u64,
}

/// One complete V2 remote invocation of a reusable V2 plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryRequestV2 {
    advertisement: String,
    expires_at_unix_ms: u64,
    format: String,
    limits: RemoteLimitsV2,
    nonce: String,
    plan: serde_json::Value,
    prepared_at_unix_ms: u64,
    result: RemoteResultKindV2,
    rows: Vec<Vec<Option<CanonicalValue>>>,
}

impl RemoteQueryRequestV2 {
    /// Bind one V2 plan invocation, result contract, advertisement, and budget.
    ///
    /// `result` is checked against both the invocation operation and the
    /// embedded plan. Model-oriented result kinds additionally require the
    /// model-query contract carried by the V2 plan.
    pub fn new(
        plan: &QueryPlan,
        invocation: &QueryInvocation,
        result: RemoteResultKindV2,
        advertisement: &RemoteCapabilities,
        limits: RemoteLimitsV2,
        nonce: impl Into<String>,
        prepared_at_unix_ms: u64,
    ) -> Result<Self, Diagnostic> {
        validate_v2_plan_and_result(plan, result)?;
        if !invocation.binds(plan)? || invocation.operation() != result.operation() {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_invocation_plan_mismatch",
                "V2 invocation, result contract, and plan do not bind one another",
            ));
        }
        let mut required = plan.required_capabilities().clone();
        for capability in invocation.transport_capabilities() {
            required.insert(capability);
        }
        for capability in query_remote_v2_required_capabilities(result.carries_hydration_graph()) {
            required.insert(capability);
        }
        required.ensure_supported_by(advertisement.capabilities())?;

        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        validate_remote_limits_v2(limits)?;
        let expires_at_unix_ms = prepared_at_unix_ms
            .checked_add(limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS))
            .ok_or_else(remote_time_invalid_v2)?;
        validate_remote_time_shape_v2(prepared_at_unix_ms, expires_at_unix_ms, limits)?;
        let plan = serde_json::from_slice::<serde_json::Value>(&plan.canonical_bytes()?).map_err(
            |_| {
                remote_v2_failure(
                    DiagnosticCategory::Integrity,
                    "query_remote_v2_plan_unencodable",
                    "the V2 plan cannot be embedded as canonical JSON",
                )
            },
        )?;
        Ok(Self {
            advertisement: advertisement.fingerprint()?.digest_hex(),
            expires_at_unix_ms,
            format: QUERY_REMOTE_REQUEST_FORMAT_V2.to_owned(),
            limits,
            nonce,
            plan,
            prepared_at_unix_ms,
            result,
            rows: invocation
                .inputs()
                .iter()
                .map(|row| row.values().to_vec())
                .collect(),
        })
    }

    /// Encode exact canonical V2 request bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_REQUEST_CODEC_LIMITS)
    }

    /// Decode one canonical V2 request, rejecting V1 and unknown versions
    /// before reconstructing the embedded plan.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        require_format(bytes, QUERY_REMOTE_REQUEST_FORMAT_V2, "remote request")?;
        let request = from_canonical_json_with_limits::<Self>(bytes, REMOTE_REQUEST_CODEC_LIMITS)?;
        check_nonce_v2(&request.nonce)?;
        validate_remote_limits_v2(request.limits)?;
        FingerprintDigest::from_hex(&request.advertisement).map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_advertisement_invalid",
                "V2 request carries a malformed advertisement fingerprint",
            )
        })?;
        validate_remote_time_shape_v2(
            request.prepared_at_unix_ms,
            request.expires_at_unix_ms,
            request.limits,
        )?;
        let plan = request.plan()?;
        validate_v2_plan_and_result(&plan, request.result)?;
        request.invocation(&plan)?;
        if request.encode()? != bytes {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_request_wire_mismatch",
                "V2 request bytes normalize after trusted reconstruction",
            ));
        }
        Ok(request)
    }

    /// Rebuild the trusted V2 plan from the embedded canonical document.
    pub fn plan(&self) -> Result<QueryPlan, Diagnostic> {
        let plan = decode_query_plan(&to_canonical_json(&self.plan)?)?;
        if !is_query_plan_v2(&plan) {
            return Err(remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_plan_format_mismatch",
                "a V2 remote request must embed a V2 query plan",
            ));
        }
        Ok(plan)
    }

    /// Rebuild the validated invocation against the carried plan.
    pub fn invocation(&self, plan: &QueryPlan) -> Result<QueryInvocation, Diagnostic> {
        QueryInvocation::new(
            plan,
            self.result.operation(),
            self.rows
                .iter()
                .map(|row| InputRow::new(row.clone()))
                .collect(),
        )
    }

    /// Compute the V2 request fingerprint over exact canonical bytes.
    pub fn fingerprint(&self) -> Result<RemoteRequestFingerprintV2, Diagnostic> {
        RemoteRequestFingerprintV2::compute(&self.encode()?)
    }

    /// Return the caller nonce echoed by a correlated reply.
    #[must_use]
    pub fn nonce(&self) -> &str {
        &self.nonce
    }

    /// Return the response family bound into this request.
    #[must_use]
    pub const fn result_kind(&self) -> RemoteResultKindV2 {
        self.result
    }

    /// Return the caller execution budgets.
    #[must_use]
    pub const fn limits(&self) -> RemoteLimitsV2 {
        self.limits
    }

    /// Return whether this request binds the executor's exact advertisement.
    pub fn binds_advertisement(
        &self,
        advertisement: &RemoteCapabilities,
    ) -> Result<bool, Diagnostic> {
        let expected = advertisement.fingerprint()?;
        let actual = FingerprintDigest::from_hex(&self.advertisement).map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_advertisement_invalid",
                "V2 request carries a malformed advertisement fingerprint",
            )
        })?;
        Ok(actual == expected.as_fingerprint().digest())
    }

    /// Recheck exact advertisement identity and every plan, invocation, and
    /// envelope capability before an executor constructs provider resources.
    pub fn validate_advertisement(
        &self,
        advertisement: &RemoteCapabilities,
    ) -> Result<(), Diagnostic> {
        if !self.binds_advertisement(advertisement)? {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_advertisement_mismatch",
                "V2 request does not bind this executor advertisement",
            ));
        }
        let plan = self.plan()?;
        let invocation = self.invocation(&plan)?;
        let mut required = plan.required_capabilities().clone();
        for capability in invocation.transport_capabilities() {
            required.insert(capability);
        }
        for capability in
            query_remote_v2_required_capabilities(self.result.carries_hydration_graph())
        {
            required.insert(capability);
        }
        required.ensure_supported_by(advertisement.capabilities())
    }

    /// Validate the absolute request lifetime at one executor clock sample.
    pub fn remaining_lifetime_ms(&self, now_unix_ms: u64) -> Result<u64, Diagnostic> {
        validate_remote_time_shape_v2(
            self.prepared_at_unix_ms,
            self.expires_at_unix_ms,
            self.limits,
        )?;
        if self.prepared_at_unix_ms > now_unix_ms.saturating_add(MAX_REMOTE_CLOCK_SKEW_MS) {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_time_future",
                "V2 request preparation time exceeds the allowed clock skew",
            ));
        }
        self.expires_at_unix_ms
            .checked_sub(now_unix_ms)
            .filter(|remaining| *remaining > 0)
            .ok_or_else(|| {
                remote_v2_failure(
                    DiagnosticCategory::ResourceLimit,
                    "query_remote_v2_request_expired",
                    "V2 request absolute expiry has elapsed",
                )
            })
    }

    /// Return remaining execution time without granting positive clock skew.
    pub fn remaining_execution_ms(&self, now_unix_ms: u64) -> Result<u64, Diagnostic> {
        let absolute_remaining = self.remaining_lifetime_ms(now_unix_ms)?;
        let declared_lifetime = self
            .expires_at_unix_ms
            .checked_sub(self.prepared_at_unix_ms)
            .ok_or_else(remote_time_invalid_v2)?;
        Ok(absolute_remaining.min(declared_lifetime))
    }

    /// Return the exclusive absolute expiry timestamp.
    #[must_use]
    pub const fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

/// The canonical fingerprint of one complete V2 request envelope.
///
/// The domain is shared with V1 while the canonicalization identifier is
/// distinct, so identical-looking versioned documents cannot share identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteRequestFingerprintV2(Fingerprint);

impl RemoteRequestFingerprintV2 {
    /// Compute a V2 request fingerprint from exact request bytes.
    pub fn compute(request_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_REMOTE_REQUEST_CANONICALIZATION_V2)?,
            None,
            request_bytes,
        )))
    }

    /// Return the generic fingerprint.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    fn digest_hex(&self) -> String {
        self.0.digest().to_hex()
    }
}

/// A strict V2 representation of one complete structured diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteDiagnosticV2 {
    category: DiagnosticCategory,
    code: DiagnosticCode,
    details: BTreeMap<String, RemoteDiagnosticDetailV2>,
    message: String,
    path: Vec<RemoteDiagnosticPathSegmentV2>,
}

impl RemoteDiagnosticV2 {
    /// Preserve all diagnostic fields in their typed representation.
    pub fn new(diagnostic: &Diagnostic) -> Result<Self, Diagnostic> {
        let details = diagnostic
            .details()
            .iter()
            .map(|(key, value)| {
                validate_detail_key(key)?;
                Ok((key.clone(), RemoteDiagnosticDetailV2::from(value)))
            })
            .collect::<Result<BTreeMap<_, _>, Diagnostic>>()?;
        let value = Self {
            category: diagnostic.category(),
            code: diagnostic.code().clone(),
            details,
            message: diagnostic.message().to_owned(),
            path: diagnostic
                .path()
                .segments()
                .iter()
                .map(RemoteDiagnosticPathSegmentV2::from)
                .collect(),
        };
        value.validate_size()?;
        Ok(value)
    }

    /// Reconstruct the exact structured diagnostic.
    pub fn diagnostic(&self) -> Result<Diagnostic, Diagnostic> {
        self.validate_size()?;
        let path = DiagnosticPath::from_segments(
            self.path
                .iter()
                .map(DiagnosticPathSegment::from)
                .collect::<Vec<_>>(),
        );
        let mut diagnostic =
            Diagnostic::new(self.category, self.code.clone(), self.message.clone()).with_path(path);
        for (key, value) in &self.details {
            validate_detail_key(key)?;
            diagnostic = diagnostic.with_detail(key.clone(), detail_value_from_wire(value)?);
        }
        Ok(diagnostic)
    }

    fn validate_size(&self) -> Result<(), Diagnostic> {
        validate_remote_diagnostic_wire(self)?;
        let bytes = to_canonical_json(self)?;
        if bytes.len() > MAX_DIAGNOSTIC_BYTES {
            return Err(remote_v2_failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v2_diagnostic_too_large",
                "V2 remote diagnostic exceeds the canonical byte ceiling",
            ));
        }
        Ok(())
    }
}

/// One strict typed V2 diagnostic path segment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RemoteDiagnosticPathSegmentV2 {
    /// An object field.
    Field(String),
    /// A zero-based collection index.
    Index(u64),
    /// A typed semantic identifier.
    Identifier(String),
}

impl From<&DiagnosticPathSegment> for RemoteDiagnosticPathSegmentV2 {
    fn from(value: &DiagnosticPathSegment) -> Self {
        match value {
            DiagnosticPathSegment::Field(value) => Self::Field(value.clone()),
            DiagnosticPathSegment::Index(value) => Self::Index(*value),
            DiagnosticPathSegment::Identifier(value) => Self::Identifier(value.clone()),
        }
    }
}

impl From<&RemoteDiagnosticPathSegmentV2> for DiagnosticPathSegment {
    fn from(value: &RemoteDiagnosticPathSegmentV2) -> Self {
        match value {
            RemoteDiagnosticPathSegmentV2::Field(value) => Self::Field(value.clone()),
            RemoteDiagnosticPathSegmentV2::Index(value) => Self::Index(*value),
            RemoteDiagnosticPathSegmentV2::Identifier(value) => Self::Identifier(value.clone()),
        }
    }
}

/// One strict typed V2 diagnostic detail value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RemoteDiagnosticDetailV2 {
    /// Text context.
    Text(String),
    /// Signed long, encoded canonically as decimal text for binding safety.
    Long(String),
    /// Boolean context.
    Boolean(bool),
    /// Ordered text values.
    TextList(Vec<String>),
}

impl From<&DiagnosticDetailValue> for RemoteDiagnosticDetailV2 {
    fn from(value: &DiagnosticDetailValue) -> Self {
        match value {
            DiagnosticDetailValue::Text(value) => Self::Text(value.clone()),
            DiagnosticDetailValue::Long(value) => Self::Long(value.to_string()),
            DiagnosticDetailValue::Boolean(value) => Self::Boolean(*value),
            DiagnosticDetailValue::TextList(value) => Self::TextList(value.clone()),
        }
    }
}

/// Dense zero-based node identity in one hydration graph.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HydrationNodeIdV2(u32);

impl HydrationNodeIdV2 {
    /// Construct one node identity.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the dense ordinal.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One reference to a canonical graph node under a declared model type.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationReferenceV2 {
    declared: TypeId,
    node: HydrationNodeIdV2,
}

impl HydrationReferenceV2 {
    /// Construct one declared occurrence of a graph node.
    #[must_use]
    pub const fn new(declared: TypeId, node: HydrationNodeIdV2) -> Self {
        Self { declared, node }
    }

    /// Return the declared type under which the occurrence was selected.
    #[must_use]
    pub const fn declared(&self) -> &TypeId {
        &self.declared
    }

    /// Return the referenced dense node.
    #[must_use]
    pub const fn node(&self) -> HydrationNodeIdV2 {
        self.node
    }
}

/// Complete values for one provider attribute in a hydrated concrete model.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationAttributeEvidenceV2 {
    attribute: AttributeId,
    values: Vec<CompatibilityValueV2>,
}

impl HydrationAttributeEvidenceV2 {
    /// Construct one descriptor-qualified attribute evidence entry.
    #[must_use]
    pub const fn new(attribute: AttributeId, values: Vec<CompatibilityValueV2>) -> Self {
        Self { attribute, values }
    }

    /// Return the provider attribute descriptor.
    #[must_use]
    pub const fn attribute(&self) -> &AttributeId {
        &self.attribute
    }

    /// Return canonical or released-only compatibility values in field order.
    #[must_use]
    pub fn values(&self) -> &[CompatibilityValueV2] {
        &self.values
    }
}

/// Complete players for one descriptor-qualified hydrated relation role.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationRoleEvidenceV2 {
    players: Vec<HydrationReferenceV2>,
    role: RoleId,
}

impl HydrationRoleEvidenceV2 {
    /// Construct one ordered role-player evidence entry.
    #[must_use]
    pub const fn new(role: RoleId, players: Vec<HydrationReferenceV2>) -> Self {
        Self { players, role }
    }

    /// Return the exact descriptor-qualified role.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }

    /// Return player occurrences in provider/materializer order.
    #[must_use]
    pub fn players(&self) -> &[HydrationReferenceV2] {
        &self.players
    }
}

/// Materializable concept kind of one hydration node.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HydrationNodeKindV2 {
    /// Entity model.
    Entity,
    /// Relation model.
    Relation,
}

/// One canonical provider identity and its complete concrete model evidence.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationNodeV2 {
    attributes: Vec<HydrationAttributeEvidenceV2>,
    concrete: TypeId,
    id: HydrationNodeIdV2,
    iid: String,
    kind: HydrationNodeKindV2,
    roles: Vec<HydrationRoleEvidenceV2>,
}

impl HydrationNodeV2 {
    /// Construct one node. Whole-graph ordering, uniqueness, and references
    /// are checked by [`HydrationGraphV2::new`].
    #[must_use]
    pub const fn new(
        id: HydrationNodeIdV2,
        iid: String,
        concrete: TypeId,
        kind: HydrationNodeKindV2,
        attributes: Vec<HydrationAttributeEvidenceV2>,
        roles: Vec<HydrationRoleEvidenceV2>,
    ) -> Self {
        Self {
            attributes,
            concrete,
            id,
            iid,
            kind,
            roles,
        }
    }

    /// Return this node's dense identity.
    #[must_use]
    pub const fn id(&self) -> HydrationNodeIdV2 {
        self.id
    }

    /// Return the exact provider IID.
    #[must_use]
    pub fn iid(&self) -> &str {
        &self.iid
    }

    /// Return the exact concrete descriptor.
    #[must_use]
    pub const fn concrete(&self) -> &TypeId {
        &self.concrete
    }

    /// Return entity or relation materialization kind.
    #[must_use]
    pub const fn kind(&self) -> HydrationNodeKindV2 {
        self.kind
    }

    /// Return complete ordered attribute evidence.
    #[must_use]
    pub fn attributes(&self) -> &[HydrationAttributeEvidenceV2] {
        &self.attributes
    }

    /// Return complete ordered relation-role evidence.
    #[must_use]
    pub fn roles(&self) -> &[HydrationRoleEvidenceV2] {
        &self.roles
    }
}

/// One dense, provider-identity-deduplicated hydration graph.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationGraphV2 {
    nodes: Vec<HydrationNodeV2>,
}

impl HydrationGraphV2 {
    /// Validate and construct one complete hydration graph.
    pub fn new(nodes: Vec<HydrationNodeV2>) -> Result<Self, Diagnostic> {
        let graph = Self { nodes };
        graph.validate()?;
        Ok(graph)
    }

    /// Return dense nodes in canonical identity order.
    #[must_use]
    pub fn nodes(&self) -> &[HydrationNodeV2] {
        &self.nodes
    }

    fn validate(&self) -> Result<(), Diagnostic> {
        if self.nodes.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(remote_graph_limit_v2());
        }
        let mut iids = BTreeSet::new();
        let mut previous_iid = None;
        for (index, node) in self.nodes.iter().enumerate() {
            if usize::try_from(node.id.get()).ok() != Some(index) {
                return Err(remote_evidence_mismatch_v2(
                    "hydration graph node IDs must be dense zero-based ordinals",
                ));
            }
            if !is_canonical_thing_iid(&node.iid) || !iids.insert(node.iid.as_str()) {
                return Err(remote_evidence_mismatch_v2(
                    "hydration graph provider IIDs must be canonical and unique",
                ));
            }
            if previous_iid.is_some_and(|previous| previous >= node.iid.as_str()) {
                return Err(remote_evidence_mismatch_v2(
                    "hydration graph nodes must be strictly ordered by provider IID",
                ));
            }
            previous_iid = Some(node.iid.as_str());
            let expected_kind = match node.kind {
                HydrationNodeKindV2::Entity => TypeKind::Entity,
                HydrationNodeKindV2::Relation => TypeKind::Relation,
            };
            if node.concrete.kind() != expected_kind {
                return Err(remote_evidence_mismatch_v2(
                    "hydration node kind contradicts its concrete descriptor",
                ));
            }
            if node.kind == HydrationNodeKindV2::Entity && !node.roles.is_empty() {
                return Err(remote_evidence_mismatch_v2(
                    "hydrated entities must not carry relation roles",
                ));
            }
            if node.attributes.windows(2).any(|pair| {
                pair[0].attribute().label().as_str() >= pair[1].attribute().label().as_str()
            }) {
                return Err(remote_evidence_mismatch_v2(
                    "hydration attributes must be strictly descriptor-ordered",
                ));
            }
            if node
                .roles
                .windows(2)
                .any(|pair| pair[0].role >= pair[1].role)
            {
                return Err(remote_evidence_mismatch_v2(
                    "hydration roles must be strictly descriptor-ordered",
                ));
            }
        }
        for node in &self.nodes {
            for role in &node.roles {
                for player in &role.players {
                    let player_index = usize::try_from(player.node.get()).map_err(|_| {
                        remote_evidence_mismatch_v2(
                            "hydration role player references an unknown graph node",
                        )
                    })?;
                    let Some(player_node) = self.nodes.get(player_index) else {
                        return Err(remote_evidence_mismatch_v2(
                            "hydration role player references an unknown graph node",
                        ));
                    };
                    if player.declared.kind() != player_node.concrete.kind() {
                        return Err(remote_evidence_mismatch_v2(
                            "hydration role-player declared and concrete kinds disagree",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One hydrated model-result slot.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HydrationSlotV2 {
    /// One singular model occurrence.
    Singular {
        /// Exact graph reference.
        value: HydrationReferenceV2,
    },
    /// One ordered model collection preserving multiplicity.
    Collection {
        /// Ordered graph references.
        values: Vec<HydrationReferenceV2>,
    },
}

impl HydrationSlotV2 {
    fn references(&self) -> &[HydrationReferenceV2] {
        match self {
            Self::Singular { value } => std::slice::from_ref(value),
            Self::Collection { values } => values,
        }
    }
}

/// One ordered hydrated output row.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedRowV2 {
    slots: Vec<HydrationSlotV2>,
}

impl HydratedRowV2 {
    /// Construct one ordered row.
    #[must_use]
    pub const fn new(slots: Vec<HydrationSlotV2>) -> Self {
        Self { slots }
    }

    /// Return output slots in public order.
    #[must_use]
    pub fn slots(&self) -> &[HydrationSlotV2] {
        &self.slots
    }
}

/// The typed terminal outcome of one V2 remote invocation.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteOutcomeV2 {
    /// Evidence-validated low-level positional rows.
    Rows {
        /// Positional row values.
        rows: Vec<Vec<RemoteValue>>,
    },
    /// Evidence-validated low-level fetched documents.
    Documents {
        /// Positional document fields.
        documents: Vec<Vec<RemoteFieldValue>>,
    },
    /// Low-level answer count.
    Count {
        /// Exact unsigned count.
        value: u64,
    },
    /// Low-level existence verdict.
    Exists {
        /// Exact existence verdict.
        value: bool,
    },
    /// Model-oriented rows sharing one validated hydration graph.
    HydratedRows {
        /// Dense graph used by every row.
        graph: HydrationGraphV2,
        /// Ordered result rows.
        rows: Vec<HydratedRowV2>,
    },
    /// One distinct-root model page and optional same-snapshot total.
    HydratedPage {
        /// Ordered page entries.
        entries: Vec<HydratedRowV2>,
        /// Dense graph used by every entry.
        graph: HydrationGraphV2,
        /// Exact requested page limit.
        limit: u64,
        /// Exact requested page offset.
        offset: u64,
        /// Exact distinct root binding.
        #[serde(deserialize_with = "deserialize_binding_id_v2")]
        root: BindingId,
        /// Optional same-snapshot total.
        total: Option<u64>,
    },
    /// Lossless distinct-root count.
    DistinctCount {
        /// Exact counted root binding.
        #[serde(deserialize_with = "deserialize_binding_id_v2")]
        root: BindingId,
        /// Exact unsigned count.
        value: u64,
    },
    /// Distinct-root existence verdict.
    DistinctExists {
        /// Exact tested root binding.
        #[serde(deserialize_with = "deserialize_binding_id_v2")]
        root: BindingId,
        /// Exact existence verdict.
        value: bool,
    },
}

/// Caller budgets checked before typed V2 reply evidence is allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteReplyDecodeLimitsV2 {
    /// Maximum authenticated successful outer-reply bytes.
    pub max_bytes: u64,
    /// Maximum rows, page entries, count value, or positive existence item.
    pub max_items: u64,
    /// Maximum aggregate output collection members.
    pub max_collection_members: u64,
    /// Maximum hydration graph nodes.
    pub max_graph_nodes: u64,
    /// Maximum canonical values across hydration attribute evidence.
    pub max_attribute_values: u64,
    /// Maximum references across hydration role-player evidence.
    pub max_role_players: u64,
}

impl RemoteReplyDecodeLimitsV2 {
    fn hard_ceiling() -> Self {
        let collection = u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX);
        Self {
            max_bytes: u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX),
            max_items: u64::MAX,
            max_collection_members: collection,
            max_graph_nodes: collection,
            max_attribute_values: collection,
            max_role_players: collection,
        }
    }
}

/// One successful V2 remote execution bound to its request and plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryResponseV2 {
    format: String,
    nonce: String,
    outcome: RemoteOutcomeV2,
    plan: String,
    request: String,
}

impl RemoteQueryResponseV2 {
    /// Bind one V2 outcome to the nonce, V2 plan, whole V2 request, and
    /// request-selected result family.
    pub fn new(
        nonce: impl Into<String>,
        plan: &QueryPlan,
        request: &RemoteRequestFingerprintV2,
        expected_result: RemoteResultKindV2,
        outcome: RemoteOutcomeV2,
    ) -> Result<Self, Diagnostic> {
        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        validate_v2_plan_and_result(plan, expected_result)?;
        validate_outcome_kind_v2(&outcome, expected_result)?;
        validate_outcome_evidence_v2(
            &outcome,
            expected_result,
            RemoteReplyDecodeLimitsV2::hard_ceiling(),
            Some(plan),
        )?;
        Ok(Self {
            format: QUERY_REMOTE_RESPONSE_FORMAT_V2.to_owned(),
            nonce,
            outcome,
            plan: plan.fingerprint()?.as_fingerprint().digest().to_hex(),
            request: request.digest_hex(),
        })
    }

    /// Encode one authenticated outer reply using the unchanged signed-reply
    /// and capability-advertisement formats.
    pub fn encode_signed(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        crate::query_remote::encode_signed_reply_payload(
            &self.encode_payload()?,
            advertisement,
            signer,
        )
    }

    /// Return the exact signed outer-envelope length without signing.
    pub fn signed_encoded_len(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        key: RemoteSigningPublicKey,
    ) -> Result<usize, Diagnostic> {
        Ok(crate::query_remote::signed_reply_payload_encoded_len(
            &self.encode_payload()?,
            advertisement,
            key,
        ))
    }

    /// Return a signed-envelope framing lower bound for one result family.
    ///
    /// Model outcomes may require provider-owned graph evidence before a fully
    /// valid response can be constructed. This method uses the shortest
    /// syntactic member of the selected family, while still validating the
    /// nonce, plan terminal, and correlation fingerprints. Every valid signed
    /// response for the same bindings is at least this large, so executors can
    /// reject an impossible caller byte budget before constructing host state.
    pub fn signed_framing_floor(
        nonce: impl Into<String>,
        plan: &QueryPlan,
        request: &RemoteRequestFingerprintV2,
        expected_result: RemoteResultKindV2,
        advertisement: &RemoteCapabilitiesFingerprint,
        key: RemoteSigningPublicKey,
        max_items: u64,
    ) -> Result<usize, Diagnostic> {
        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        validate_v2_plan_and_result(plan, expected_result)?;
        let root = BindingId::new(0)?;
        let outcome = match expected_result {
            RemoteResultKindV2::Rows => RemoteOutcomeV2::Rows { rows: Vec::new() },
            RemoteResultKindV2::Documents => RemoteOutcomeV2::Documents {
                documents: Vec::new(),
            },
            RemoteResultKindV2::Count => RemoteOutcomeV2::Count { value: 0 },
            RemoteResultKindV2::Exists if max_items == 0 => {
                RemoteOutcomeV2::Exists { value: false }
            }
            RemoteResultKindV2::Exists => RemoteOutcomeV2::Exists { value: true },
            RemoteResultKindV2::HydratedRows => RemoteOutcomeV2::HydratedRows {
                graph: HydrationGraphV2 { nodes: Vec::new() },
                rows: Vec::new(),
            },
            RemoteResultKindV2::HydratedPage => RemoteOutcomeV2::HydratedPage {
                entries: Vec::new(),
                graph: HydrationGraphV2 { nodes: Vec::new() },
                limit: 0,
                offset: 0,
                root,
                total: Some(0),
            },
            RemoteResultKindV2::DistinctCount => RemoteOutcomeV2::DistinctCount { root, value: 0 },
            RemoteResultKindV2::DistinctExists if max_items == 0 => {
                RemoteOutcomeV2::DistinctExists { root, value: false }
            }
            RemoteResultKindV2::DistinctExists => {
                RemoteOutcomeV2::DistinctExists { root, value: true }
            }
        };
        let response = Self {
            format: QUERY_REMOTE_RESPONSE_FORMAT_V2.to_owned(),
            nonce,
            outcome,
            plan: plan.fingerprint()?.as_fingerprint().digest().to_hex(),
            request: request.digest_hex(),
        };
        response.signed_encoded_len(advertisement, key)
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    fn decode_bound(
        bytes: &[u8],
        expected_result: RemoteResultKindV2,
        limits: RemoteReplyDecodeLimitsV2,
        plan: Option<&QueryPlan>,
    ) -> Result<Self, Diagnostic> {
        require_format(bytes, QUERY_REMOTE_RESPONSE_FORMAT_V2, "remote response")?;
        let output_width = match expected_result {
            RemoteResultKindV2::Rows | RemoteResultKindV2::Documents => {
                plan.and_then(low_level_output_width)
            }
            RemoteResultKindV2::HydratedRows | RemoteResultKindV2::HydratedPage => {
                model_output_width(plan, expected_result)
            }
            RemoteResultKindV2::Count
            | RemoteResultKindV2::Exists
            | RemoteResultKindV2::DistinctCount
            | RemoteResultKindV2::DistinctExists => None,
        };
        preflight_response_evidence_v2(bytes, expected_result, output_width, limits)?;
        let response =
            from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        validate_outcome_kind_v2(&response.outcome, expected_result)?;
        validate_outcome_evidence_v2(&response.outcome, expected_result, limits, plan)?;
        if response.encode_payload()? != bytes {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_response_wire_mismatch",
                "V2 response bytes normalize after trusted reconstruction",
            ));
        }
        Ok(response)
    }

    /// Return the typed outcome.
    #[must_use]
    pub const fn outcome(&self) -> &RemoteOutcomeV2 {
        &self.outcome
    }

    /// Consume the response and return its typed outcome.
    #[must_use]
    pub fn into_outcome(self) -> RemoteOutcomeV2 {
        self.outcome
    }
}

fn encode_minimal_signed_failure_v2(
    original: &RemoteQueryFailureV2,
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"category\":\"integrity\",\"code\":\"query_remote_v2_internal_failure\",\"details\":{},\"format\":\"typebridge.query-remote-failure/v2\",\"message\":\"executor could not encode the original V2 remote failure\",\"nonce\":";
    let nonce = original
        .nonce
        .as_deref()
        .filter(|nonce| check_nonce_v2(nonce).is_ok());
    let request = original.request.as_deref().filter(|request| {
        request.len() == 64
            && request
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    });
    let mut payload = Vec::with_capacity(PREFIX.len() + 256);
    payload.extend_from_slice(PREFIX);
    append_optional_safe_ascii_v2(&mut payload, nonce);
    payload.extend_from_slice(b",\"path\":[],\"request\":");
    append_optional_safe_ascii_v2(&mut payload, request);
    payload.push(b'}');
    crate::query_remote::encode_signed_reply_payload_unchecked(&payload, advertisement, signer)
}

fn append_optional_safe_ascii_v2(encoded: &mut Vec<u8>, value: Option<&str>) {
    if let Some(value) = value {
        encoded.push(b'"');
        encoded.extend_from_slice(value.as_bytes());
        encoded.push(b'"');
    } else {
        encoded.extend_from_slice(b"null");
    }
}

/// One complete structured V2 remote failure bound to its request when one
/// decoded far enough to establish correlation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteQueryFailureV2 {
    category: DiagnosticCategory,
    code: DiagnosticCode,
    details: BTreeMap<String, RemoteDiagnosticDetailV2>,
    format: String,
    message: String,
    nonce: Option<String>,
    path: Vec<RemoteDiagnosticPathSegmentV2>,
    request: Option<String>,
}

impl RemoteQueryFailureV2 {
    /// Construct an uncorrelated V2 failure for a pre-request transport error.
    pub fn new(nonce: Option<String>, diagnostic: &Diagnostic) -> Result<Self, Diagnostic> {
        let diagnostic = RemoteDiagnosticV2::new(diagnostic)?;
        if let Some(nonce) = nonce.as_deref() {
            check_nonce_v2(nonce)?;
        }
        Ok(Self {
            category: diagnostic.category,
            code: diagnostic.code,
            details: diagnostic.details,
            format: QUERY_REMOTE_FAILURE_FORMAT_V2.to_owned(),
            message: diagnostic.message,
            nonce,
            path: diagnostic.path,
            request: None,
        })
    }

    /// Bind all five diagnostic fields to the exact V2 request.
    pub fn bound(
        nonce: impl Into<String>,
        request: &RemoteRequestFingerprintV2,
        diagnostic: &Diagnostic,
    ) -> Result<Self, Diagnostic> {
        let nonce = nonce.into();
        check_nonce_v2(&nonce)?;
        let mut failure = Self::new(Some(nonce), diagnostic)?;
        failure.request = Some(request.digest_hex());
        Ok(failure)
    }

    /// Verify the mandatory request correlation of a request-bound failure.
    pub fn verify_binding(
        &self,
        expected_nonce: &str,
        expected_request: &RemoteRequestFingerprintV2,
    ) -> Result<(), Diagnostic> {
        verify_failure_binding_v2(
            self.nonce.as_deref(),
            self.request.as_deref(),
            expected_nonce,
            expected_request,
        )
    }

    /// Encode one authenticated V2 failure in the unchanged signed outer
    /// envelope.
    pub fn encode_signed(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        crate::query_remote::encode_signed_reply_payload(
            &self.encode_payload()?,
            advertisement,
            signer,
        )
    }

    /// Encode an authenticated failure, replacing an unencodable diagnostic
    /// with a fixed bounded V2 internal failure while retaining safe request
    /// bindings.
    #[must_use]
    pub fn encode_signed_or_fallback(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Vec<u8> {
        match self.encode_signed(advertisement, signer) {
            Ok(encoded) => encoded,
            Err(_) => encode_minimal_signed_failure_v2(self, advertisement, signer),
        }
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        self.remote_diagnostic()?.validate_size()?;
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    /// Decode an already authenticated V2 failure payload.
    pub fn decode_payload(bytes: &[u8]) -> Result<Self, Diagnostic> {
        require_format(bytes, QUERY_REMOTE_FAILURE_FORMAT_V2, "remote failure")?;
        let maximum = MAX_DIAGNOSTIC_BYTES.saturating_add(FAILURE_FRAMING_ALLOWANCE_BYTES);
        if bytes.len() > maximum {
            return Err(remote_v2_failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v2_diagnostic_too_large",
                "V2 remote diagnostic exceeds the canonical byte ceiling",
            ));
        }
        let failure = from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        if let Some(nonce) = failure.nonce.as_deref() {
            check_nonce_v2(nonce)?;
        }
        failure.remote_diagnostic()?.validate_size()?;
        if failure.encode_payload()? != bytes {
            return Err(remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_failure_wire_mismatch",
                "V2 failure bytes normalize after trusted reconstruction",
            ));
        }
        Ok(failure)
    }

    /// Rebuild the exact structured diagnostic.
    pub fn diagnostic(&self) -> Result<Diagnostic, Diagnostic> {
        self.remote_diagnostic()?.diagnostic()
    }

    /// Return the echoed nonce, when request decoding reached it.
    #[must_use]
    pub fn nonce(&self) -> Option<&str> {
        self.nonce.as_deref()
    }

    fn remote_diagnostic(&self) -> Result<RemoteDiagnosticV2, Diagnostic> {
        let diagnostic = RemoteDiagnosticV2 {
            category: self.category,
            code: self.code.clone(),
            details: self.details.clone(),
            message: self.message.clone(),
            path: self.path.clone(),
        };
        validate_remote_diagnostic_wire(&diagnostic)?;
        Ok(diagnostic)
    }
}

/// One decoded and correlated V2 remote reply.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteReplyV2 {
    /// A successful V2 response.
    Response(RemoteQueryResponseV2),
    /// A fully structured V2 failure.
    Failure(RemoteQueryFailureV2),
}

#[derive(Deserialize)]
struct RemoteReplyBindingPeekV2<'wire> {
    #[serde(borrow)]
    format: &'wire str,
    #[serde(borrow)]
    nonce: Option<&'wire str>,
    #[serde(borrow)]
    plan: Option<&'wire str>,
    #[serde(borrow)]
    request: Option<&'wire str>,
}

/// Authenticate, correlate, preflight, and decode one V2 remote reply.
///
/// Signature and request correlation checks precede allocation of the typed
/// outcome or reconstruction of a payload-selected diagnostic.
#[expect(
    clippy::too_many_arguments,
    reason = "the trust-boundary API keeps every expected binding and verifier explicit"
)]
pub fn decode_remote_reply_v2(
    bytes: &[u8],
    expected_request_envelope: &RemoteQueryRequestV2,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprintV2,
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    limits: RemoteReplyDecodeLimitsV2,
    verifier: &impl RemoteReplyVerifier,
) -> Result<RemoteReplyV2, Diagnostic> {
    validate_reply_limits_v2(expected_request_envelope.limits(), limits)?;
    crate::query_remote::preflight_signed_reply_size(bytes, u64::MAX)?;
    let payload = crate::query_remote::verify_signed_reply_payload(
        bytes,
        expected_advertisement,
        trusted_key,
        verifier,
    )?;
    let plan = validate_expected_request_binding_v2(
        expected_request_envelope,
        expected_plan,
        expected_request,
        expected_advertisement,
    )?;
    let peek = peek_remote_reply_binding_v2(payload)?;
    if peek.format == QUERY_REMOTE_RESPONSE_FORMAT_V2 {
        verify_response_binding_v2(
            peek.nonce,
            peek.plan,
            peek.request,
            expected_request_envelope.nonce(),
            expected_plan,
            expected_request,
        )?;
        crate::query_remote::preflight_signed_reply_size(bytes, limits.max_bytes)?;
        let response = RemoteQueryResponseV2::decode_bound(
            payload,
            expected_request_envelope.result_kind(),
            limits,
            Some(&plan),
        )?;
        return Ok(RemoteReplyV2::Response(response));
    }
    if peek.format == QUERY_REMOTE_FAILURE_FORMAT_V2 {
        verify_failure_binding_v2(
            peek.nonce,
            peek.request,
            expected_request_envelope.nonce(),
            expected_request,
        )?;
        let failure = RemoteQueryFailureV2::decode_payload(payload)?;
        failure.verify_binding(expected_request_envelope.nonce(), expected_request)?;
        return Ok(RemoteReplyV2::Failure(failure));
    }
    Err(remote_v2_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_v2_format_unsupported",
        "signed payload is not a V2 remote response or failure",
    ))
}

fn validate_reply_limits_v2(
    request: RemoteLimitsV2,
    reply: RemoteReplyDecodeLimitsV2,
) -> Result<(), Diagnostic> {
    let widened = reply.max_bytes > request.max_bytes
        || reply.max_items > request.max_items
        || reply.max_collection_members > request.max_collection_members
        || reply.max_graph_nodes > request.max_graph_nodes
        || reply.max_attribute_values > request.max_attribute_values
        || reply.max_role_players > request.max_role_players;
    if widened {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_limits_widened",
            "reply decode budgets must monotonically tighten the request-bound limits",
        ));
    }
    Ok(())
}

fn validate_expected_request_binding_v2(
    request: &RemoteQueryRequestV2,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprintV2,
    expected_advertisement: &RemoteCapabilitiesFingerprint,
) -> Result<QueryPlan, Diagnostic> {
    let plan = request.plan()?;
    let actual_advertisement =
        FingerprintDigest::from_hex(&request.advertisement).map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_advertisement_invalid",
                "V2 request carries a malformed advertisement fingerprint",
            )
        })?;
    let coherent = plan.fingerprint()? == *expected_plan
        && request.fingerprint()? == *expected_request
        && actual_advertisement == expected_advertisement.as_fingerprint().digest();
    if !coherent {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_expected_binding_mismatch",
            "trusted V2 reply bindings do not describe the expected request envelope",
        ));
    }
    Ok(plan)
}

/// Authenticate and decode an uncorrelated V2 pre-request failure.
pub fn decode_signed_remote_failure_v2(
    bytes: &[u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    max_bytes: u64,
    verifier: &impl RemoteReplyVerifier,
) -> Result<RemoteQueryFailureV2, Diagnostic> {
    crate::query_remote::preflight_signed_reply_size(bytes, max_bytes)?;
    let payload = crate::query_remote::verify_signed_reply_payload(
        bytes,
        expected_advertisement,
        trusted_key,
        verifier,
    )?;
    let failure = RemoteQueryFailureV2::decode_payload(payload)?;
    if failure.request.is_some() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_failure_unexpected_binding",
            "a pre-request V2 failure must not carry a request fingerprint",
        ));
    }
    Ok(failure)
}

#[derive(Deserialize)]
struct FormatPeek<'wire> {
    #[serde(borrow)]
    format: &'wire str,
}

fn require_format(bytes: &[u8], expected: &str, subject: &str) -> Result<(), Diagnostic> {
    if bytes.len() > MAX_REMOTE_ENVELOPE_BYTES {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_envelope_too_large",
            "V2 remote envelope exceeds the hard byte ceiling",
        ));
    }
    let peek = serde_json::from_slice::<FormatPeek<'_>>(bytes).map_err(|_| {
        remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_format_missing",
            "remote envelope has no readable format discriminator",
        )
    })?;
    if peek.format != expected {
        return Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_format_unsupported",
            match subject {
                "remote request" => "remote request is not the V2 wire format",
                "remote response" => "remote response is not the V2 wire format",
                "remote failure" => "remote failure is not the V2 wire format",
                _ => "remote envelope is not the expected V2 wire format",
            },
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(transparent)]
struct BindingIdV2Wire(u16);

fn deserialize_binding_id_v2<'de, D>(deserializer: D) -> Result<BindingId, D::Error>
where
    D: Deserializer<'de>,
{
    let wire = BindingIdV2Wire::deserialize(deserializer)?;
    BindingId::new(wire.0).map_err(serde::de::Error::custom)
}

fn is_query_plan_v2(plan: &QueryPlan) -> bool {
    plan.format() == "typebridge.query-plan/v2"
}

fn validate_v2_plan_and_result(
    plan: &QueryPlan,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    if !is_query_plan_v2(plan) {
        return Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_plan_format_mismatch",
            "V2 remote envelopes accept only typebridge.query-plan/v2",
        ));
    }
    let has_model_contract = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
        .is_some();
    if has_model_contract != result.compatibility_terminal() {
        return Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_result_contract_mismatch",
            "V2 result family does not match the plan's model-query contract",
        ));
    }
    if let Some(model) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    {
        let matches = matches!(
            (result, model),
            (
                RemoteResultKindV2::HydratedRows,
                crate::query_plan::ModelQueryV2::Rows { .. }
            ) | (
                RemoteResultKindV2::HydratedPage,
                crate::query_plan::ModelQueryV2::Page { .. }
            ) | (
                RemoteResultKindV2::DistinctCount,
                crate::query_plan::ModelQueryV2::DistinctCount { .. }
            ) | (
                RemoteResultKindV2::DistinctExists,
                crate::query_plan::ModelQueryV2::DistinctExists { .. }
            )
        );
        if !matches {
            return Err(remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_result_contract_mismatch",
                "V2 result family contradicts the plan's exact model-query terminal",
            ));
        }
    } else {
        match (result, plan.output()) {
            (RemoteResultKindV2::Rows, QueryOutput::Rows { .. })
            | (RemoteResultKindV2::Documents, QueryOutput::Documents { .. })
            | (RemoteResultKindV2::Count | RemoteResultKindV2::Exists, _) => {}
            _ => {
                return Err(remote_v2_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_v2_result_contract_mismatch",
                    "low-level result family contradicts the plan output",
                ));
            }
        }
    }
    Ok(())
}

fn check_nonce_v2(nonce: &str) -> Result<(), Diagnostic> {
    let valid = (NONCE_MIN_BYTES..=NONCE_MAX_BYTES).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_nonce_invalid",
            "V2 request nonces are 16-128 ASCII alphanumeric or dash bytes",
        ))
    }
}

fn validate_remote_limits_v2(limits: RemoteLimitsV2) -> Result<(), Diagnostic> {
    if limits
        .deadline_ms
        .is_some_and(|value| value > MAX_REMOTE_DEADLINE_MS)
    {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_deadline_limit",
            "V2 remote deadline exceeds the maximum supported duration",
        ));
    }
    Ok(())
}

fn validate_remote_time_shape_v2(
    prepared_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    limits: RemoteLimitsV2,
) -> Result<(), Diagnostic> {
    let lifetime = expires_at_unix_ms
        .checked_sub(prepared_at_unix_ms)
        .ok_or_else(remote_time_invalid_v2)?;
    if lifetime != limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS) {
        return Err(remote_time_invalid_v2());
    }
    Ok(())
}

fn remote_time_invalid_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_v2_time_invalid",
        "V2 request timestamps do not form a bounded absolute lifetime",
    )
}

fn validate_detail_key(key: &str) -> Result<(), Diagnostic> {
    DiagnosticCode::new(key.to_owned())
        .map(|_| ())
        .map_err(|_| {
            remote_v2_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_v2_diagnostic_detail_key_invalid",
                "V2 diagnostic detail keys must be canonical snake-case identifiers",
            )
        })
}

fn detail_value_from_wire(
    value: &RemoteDiagnosticDetailV2,
) -> Result<DiagnosticDetailValue, Diagnostic> {
    match value {
        RemoteDiagnosticDetailV2::Text(value) => Ok(DiagnosticDetailValue::Text(value.clone())),
        RemoteDiagnosticDetailV2::Long(value) => {
            let parsed = value.parse::<i64>().map_err(|_| {
                remote_v2_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_v2_diagnostic_long_invalid",
                    "V2 diagnostic long detail is outside the signed 64-bit range",
                )
            })?;
            if parsed.to_string() != *value {
                return Err(remote_v2_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_v2_diagnostic_long_invalid",
                    "V2 diagnostic long detail is not canonical decimal text",
                ));
            }
            Ok(DiagnosticDetailValue::Long(parsed))
        }
        RemoteDiagnosticDetailV2::Boolean(value) => Ok(DiagnosticDetailValue::Boolean(*value)),
        RemoteDiagnosticDetailV2::TextList(value) => {
            Ok(DiagnosticDetailValue::TextList(value.clone()))
        }
    }
}

fn validate_remote_diagnostic_wire(value: &RemoteDiagnosticV2) -> Result<(), Diagnostic> {
    for key in value.details.keys() {
        validate_detail_key(key)?;
    }
    for detail in value.details.values() {
        detail_value_from_wire(detail)?;
    }
    Ok(())
}

fn peek_remote_reply_binding_v2(bytes: &[u8]) -> Result<RemoteReplyBindingPeekV2<'_>, Diagnostic> {
    if bytes.len() > MAX_REMOTE_ENVELOPE_BYTES {
        return Err(remote_v2_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_v2_envelope_too_large",
            "V2 remote reply exceeds the envelope byte ceiling",
        ));
    }
    serde_json::from_slice(bytes).map_err(|_| {
        remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_reply_malformed",
            "V2 remote reply has no readable correlation fields",
        )
    })
}

fn verify_response_binding_v2(
    nonce: Option<&str>,
    plan: Option<&str>,
    request: Option<&str>,
    expected_nonce: &str,
    expected_plan: &QueryPlanFingerprint,
    expected_request: &RemoteRequestFingerprintV2,
) -> Result<(), Diagnostic> {
    if nonce != Some(expected_nonce) {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_nonce_mismatch",
            "V2 response does not echo the request nonce",
        ));
    }
    let Some(plan) = plan else {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_plan_mismatch",
            "V2 response does not bind the invoked plan",
        ));
    };
    let echoed_plan = FingerprintDigest::from_hex(plan)?;
    if echoed_plan != expected_plan.as_fingerprint().digest() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_plan_mismatch",
            "V2 response does not bind the invoked plan",
        ));
    }
    let Some(request) = request else {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_request_mismatch",
            "V2 response does not bind the exact request",
        ));
    };
    let echoed_request = FingerprintDigest::from_hex(request)?;
    if echoed_request != expected_request.as_fingerprint().digest() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_request_mismatch",
            "V2 response does not bind the exact request",
        ));
    }
    Ok(())
}

fn verify_failure_binding_v2(
    nonce: Option<&str>,
    request: Option<&str>,
    expected_nonce: &str,
    expected_request: &RemoteRequestFingerprintV2,
) -> Result<(), Diagnostic> {
    let nonce = nonce.ok_or_else(|| {
        remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_failure_unbound",
            "V2 failure is not bound to the request nonce",
        )
    })?;
    if nonce != expected_nonce {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_nonce_mismatch",
            "V2 failure does not echo the request nonce",
        ));
    }
    let request = request.ok_or_else(|| {
        remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_failure_unbound",
            "V2 failure is not bound to the request fingerprint",
        )
    })?;
    let echoed_request = FingerprintDigest::from_hex(request)?;
    if echoed_request != expected_request.as_fingerprint().digest() {
        return Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_request_mismatch",
            "V2 failure does not bind the exact request",
        ));
    }
    Ok(())
}

fn validate_outcome_kind_v2(
    outcome: &RemoteOutcomeV2,
    expected: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let actual = match outcome {
        RemoteOutcomeV2::Rows { .. } => RemoteResultKindV2::Rows,
        RemoteOutcomeV2::Documents { .. } => RemoteResultKindV2::Documents,
        RemoteOutcomeV2::Count { .. } => RemoteResultKindV2::Count,
        RemoteOutcomeV2::Exists { .. } => RemoteResultKindV2::Exists,
        RemoteOutcomeV2::HydratedRows { .. } => RemoteResultKindV2::HydratedRows,
        RemoteOutcomeV2::HydratedPage { .. } => RemoteResultKindV2::HydratedPage,
        RemoteOutcomeV2::DistinctCount { .. } => RemoteResultKindV2::DistinctCount,
        RemoteOutcomeV2::DistinctExists { .. } => RemoteResultKindV2::DistinctExists,
    };
    if actual == expected {
        Ok(())
    } else {
        Err(remote_v2_failure(
            DiagnosticCategory::Integrity,
            "query_remote_v2_outcome_mismatch",
            "V2 response outcome kind does not match the request-bound terminal",
        ))
    }
}

/// Validate one locally constructed V2 outcome against the exact plan,
/// terminal, and request-tightened evidence budgets.
///
/// This is the contract-owned gate for compatibility executors before they
/// hand evidence to the response serializer. It checks terminal identity,
/// low-level widths, model projections, hydration graph authority, and every
/// item/collection/graph budget. The authenticated outer-envelope byte budget
/// remains a property of the final signed response and is checked by
/// [`RemoteQueryResponseV2::encode_signed`] consumers.
pub fn validate_remote_outcome_v2(
    outcome: &RemoteOutcomeV2,
    expected: RemoteResultKindV2,
    limits: RemoteReplyDecodeLimitsV2,
    plan: &QueryPlan,
) -> Result<(), Diagnostic> {
    validate_outcome_evidence_v2(outcome, expected, limits, Some(plan))
}

fn validate_outcome_evidence_v2(
    outcome: &RemoteOutcomeV2,
    expected: RemoteResultKindV2,
    limits: RemoteReplyDecodeLimitsV2,
    plan: Option<&QueryPlan>,
) -> Result<(), Diagnostic> {
    validate_outcome_kind_v2(outcome, expected)?;
    match outcome {
        RemoteOutcomeV2::Rows { rows } => {
            check_sequence_item_budget(rows.len(), limits.max_items)?;
            let expected_width = plan.and_then(low_level_output_width);
            validate_low_level_rows(rows, expected_width)?;
        }
        RemoteOutcomeV2::Documents { documents } => {
            check_sequence_item_budget(documents.len(), limits.max_items)?;
            let expected_width = plan.and_then(low_level_output_width);
            let mut members = 0_u64;
            let width = expected_width.or_else(|| documents.first().map(Vec::len));
            for document in documents {
                if width.is_some_and(|width| document.len() != width)
                    || document.is_empty()
                    || document.len() > MAX_SELECTED_SLOTS
                {
                    return Err(remote_evidence_mismatch_v2(
                        "V2 document width contradicts the validated output",
                    ));
                }
                for field in document {
                    if let RemoteFieldValue::List { values } = field {
                        add_budgeted(
                            &mut members,
                            values.len(),
                            limits.max_collection_members,
                            remote_collection_limit_v2,
                        )?;
                    }
                }
            }
        }
        RemoteOutcomeV2::Count { value } => {
            if *value > limits.max_items {
                return Err(remote_item_limit_v2());
            }
        }
        RemoteOutcomeV2::Exists { value } => {
            if *value && limits.max_items == 0 {
                return Err(remote_item_limit_v2());
            }
        }
        RemoteOutcomeV2::HydratedRows { graph, rows } => {
            check_sequence_item_budget(rows.len(), limits.max_items)?;
            validate_hydration_graph_budget(graph, limits)?;
            validate_hydrated_rows(graph, rows, limits, plan, expected)?;
        }
        RemoteOutcomeV2::HydratedPage {
            entries,
            graph,
            limit,
            offset,
            root,
            total,
        } => {
            check_sequence_item_budget(entries.len(), limits.max_items)?;
            if u64::try_from(entries.len()).unwrap_or(u64::MAX) > *limit {
                return Err(remote_evidence_mismatch_v2(
                    "V2 page contains more entries than its exact limit",
                ));
            }
            if let Some(total) = total {
                let remaining = total.saturating_sub(*offset);
                let exact_len = remaining.min(*limit);
                if u64::try_from(entries.len()).unwrap_or(u64::MAX) != exact_len {
                    return Err(remote_evidence_mismatch_v2(
                        "V2 page entries contradict its same-snapshot total and window",
                    ));
                }
            }
            validate_hydration_graph_budget(graph, limits)?;
            validate_hydrated_rows(graph, entries, limits, plan, expected)?;
            validate_page_root_distinct(entries, *root, plan)?;
            validate_page_contract(plan, *root, *offset, *limit, total.is_some())?;
        }
        RemoteOutcomeV2::DistinctCount { root, value } => {
            validate_scalar_root(plan, expected, *root)?;
            if *value > limits.max_items {
                return Err(remote_item_limit_v2());
            }
        }
        RemoteOutcomeV2::DistinctExists { root, value } => {
            validate_scalar_root(plan, expected, *root)?;
            if *value && limits.max_items == 0 {
                return Err(remote_item_limit_v2());
            }
        }
    }
    Ok(())
}

fn low_level_output_width(plan: &QueryPlan) -> Option<usize> {
    match plan.output() {
        QueryOutput::Rows { columns } => Some(columns.len()),
        QueryOutput::Documents { fields } => Some(fields.len()),
    }
}

fn validate_low_level_rows(
    rows: &[Vec<RemoteValue>],
    expected_width: Option<usize>,
) -> Result<(), Diagnostic> {
    let width = expected_width.or_else(|| rows.first().map(Vec::len));
    for row in rows {
        if width.is_some_and(|width| row.len() != width)
            || row.is_empty()
            || row.len() > MAX_SELECTED_SLOTS
        {
            return Err(remote_evidence_mismatch_v2(
                "V2 row width contradicts the validated output",
            ));
        }
        for value in row {
            match value {
                RemoteValue::Thing { iid, type_id } => {
                    if !is_canonical_thing_iid(iid)
                        || !matches!(type_id.kind(), TypeKind::Entity | TypeKind::Relation)
                    {
                        return Err(remote_evidence_mismatch_v2(
                            "V2 thing evidence has an invalid IID or type kind",
                        ));
                    }
                }
                RemoteValue::Attribute { type_id, .. } => {
                    if type_id.kind() != TypeKind::Attribute {
                        return Err(remote_evidence_mismatch_v2(
                            "V2 attribute evidence has a non-attribute type",
                        ));
                    }
                }
                RemoteValue::Value { .. } | RemoteValue::Absent => {}
            }
        }
    }
    Ok(())
}

fn validate_hydration_graph_budget(
    graph: &HydrationGraphV2,
    limits: RemoteReplyDecodeLimitsV2,
) -> Result<(), Diagnostic> {
    graph.validate()?;
    if u64::try_from(graph.nodes.len()).unwrap_or(u64::MAX) > limits.max_graph_nodes {
        return Err(remote_graph_limit_v2());
    }
    let mut attribute_values = 0_u64;
    let mut role_players = 0_u64;
    for node in &graph.nodes {
        for attribute in &node.attributes {
            add_budgeted(
                &mut attribute_values,
                attribute.values.len(),
                limits.max_attribute_values,
                remote_attribute_limit_v2,
            )?;
        }
        for role in &node.roles {
            add_budgeted(
                &mut role_players,
                role.players.len(),
                limits.max_role_players,
                remote_role_player_limit_v2,
            )?;
        }
    }
    Ok(())
}

fn validate_hydrated_rows(
    graph: &HydrationGraphV2,
    rows: &[HydratedRowV2],
    limits: RemoteReplyDecodeLimitsV2,
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let expected_width = model_output_width(plan, result);
    let width = expected_width.or_else(|| rows.first().map(|row| row.slots.len()));
    let mut collection_members = 0_u64;
    let mut seen_rows = BTreeSet::new();
    for row in rows {
        if width.is_some_and(|width| row.slots.len() != width)
            || row.slots.is_empty()
            || row.slots.len() > MAX_SELECTED_SLOTS
        {
            return Err(remote_evidence_mismatch_v2(
                "hydrated row width contradicts the model output contract",
            ));
        }
        if !seen_rows.insert(row.clone()) {
            return Err(remote_evidence_mismatch_v2(
                "hydrated rows must be distinct selected-identity tuples",
            ));
        }
        for slot in &row.slots {
            if let HydrationSlotV2::Collection { values } = slot {
                add_budgeted(
                    &mut collection_members,
                    values.len(),
                    limits.max_collection_members,
                    remote_collection_limit_v2,
                )?;
            }
            for reference in slot.references() {
                validate_graph_reference(graph, reference)?;
            }
        }
    }
    validate_model_rows_against_plan(rows, plan, result)?;
    validate_graph_against_model_plan(graph, rows, plan, result)?;
    validate_model_order_against_plan(graph, rows, plan, result)
}

fn validate_graph_reference(
    graph: &HydrationGraphV2,
    reference: &HydrationReferenceV2,
) -> Result<(), Diagnostic> {
    let index = usize::try_from(reference.node.get())
        .map_err(|_| remote_evidence_mismatch_v2("hydration slot references an unknown node"))?;
    let Some(node) = graph.nodes.get(index) else {
        return Err(remote_evidence_mismatch_v2(
            "hydration slot references an unknown node",
        ));
    };
    if reference.declared.kind() != node.concrete.kind() {
        return Err(remote_evidence_mismatch_v2(
            "hydration slot declared and concrete kinds disagree",
        ));
    }
    Ok(())
}

fn check_item_budget(actual: usize, maximum: u64) -> Result<(), Diagnostic> {
    if u64::try_from(actual).unwrap_or(u64::MAX) > maximum {
        Err(remote_item_limit_v2())
    } else {
        Ok(())
    }
}

fn check_sequence_item_budget(actual: usize, maximum: u64) -> Result<(), Diagnostic> {
    check_item_budget(actual, sequence_item_limit(maximum))
}

fn sequence_item_limit(maximum: u64) -> u64 {
    maximum.min(u64::try_from(MAX_CANONICAL_COLLECTION_LEN).unwrap_or(u64::MAX))
}

fn add_budgeted(
    total: &mut u64,
    increment: usize,
    maximum: u64,
    failure: fn() -> Diagnostic,
) -> Result<(), Diagnostic> {
    *total = total.saturating_add(u64::try_from(increment).unwrap_or(u64::MAX));
    if *total > maximum {
        Err(failure())
    } else {
        Ok(())
    }
}

#[derive(Deserialize)]
struct ResponseOutcomeRaw<'wire> {
    #[serde(borrow)]
    outcome: &'wire serde_json::value::RawValue,
}

fn preflight_response_evidence_v2(
    payload: &[u8],
    expected: RemoteResultKindV2,
    output_width: Option<usize>,
    limits: RemoteReplyDecodeLimitsV2,
) -> Result<(), Diagnostic> {
    let response = serde_json::from_slice::<ResponseOutcomeRaw<'_>>(payload).map_err(|_| {
        remote_v2_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_v2_reply_malformed",
            "V2 response has no readable outcome",
        )
    })?;
    let mut state = StreamingPreflightState::default();
    let mut deserializer = serde_json::Deserializer::from_str(response.outcome.get());
    let result = OutcomeScanSeed {
        expected,
        limits,
        output_width,
        state: &mut state,
    }
    .deserialize(&mut deserializer)
    .and_then(|()| deserializer.end());
    result.map_err(|_| {
        state.failure.map_or_else(
            || {
                remote_v2_failure(
                    DiagnosticCategory::InvalidContract,
                    "query_remote_v2_reply_malformed",
                    "V2 response outcome is malformed",
                )
            },
            StreamingPreflightFailure::diagnostic,
        )
    })
}

#[derive(Clone, Copy)]
enum StreamingPreflightFailure {
    AttributeValues,
    CollectionMembers,
    GraphNodes,
    Items,
    Outcome,
    RolePlayers,
}

impl StreamingPreflightFailure {
    fn diagnostic(self) -> Diagnostic {
        match self {
            Self::AttributeValues => remote_attribute_limit_v2(),
            Self::CollectionMembers => remote_collection_limit_v2(),
            Self::GraphNodes => remote_graph_limit_v2(),
            Self::Items => remote_item_limit_v2(),
            Self::Outcome => remote_v2_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v2_outcome_mismatch",
                "V2 response outcome kind does not match the request-bound terminal",
            ),
            Self::RolePlayers => remote_role_player_limit_v2(),
        }
    }
}

#[derive(Default)]
struct StreamingPreflightState {
    attribute_values: u64,
    collection_members: u64,
    failure: Option<StreamingPreflightFailure>,
    graph_nodes: u64,
    items: u64,
    role_players: u64,
}

impl StreamingPreflightState {
    fn increment(
        &mut self,
        counter: StreamingCounter,
        maximum: u64,
        failure: StreamingPreflightFailure,
    ) -> Result<(), &'static str> {
        let value = match counter {
            StreamingCounter::AttributeValues => &mut self.attribute_values,
            StreamingCounter::CollectionMembers => &mut self.collection_members,
            StreamingCounter::GraphNodes => &mut self.graph_nodes,
            StreamingCounter::Items => &mut self.items,
            StreamingCounter::RolePlayers => &mut self.role_players,
        };
        *value = value.saturating_add(1);
        if *value > maximum {
            self.failure = Some(failure);
            Err("V2 response budget exceeded")
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum StreamingCounter {
    AttributeValues,
    CollectionMembers,
    GraphNodes,
    Items,
    RolePlayers,
}

struct OutcomeScanSeed<'scan> {
    expected: RemoteResultKindV2,
    limits: RemoteReplyDecodeLimitsV2,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for OutcomeScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(OutcomeScanVisitor {
            expected: self.expected,
            limits: self.limits,
            output_width: self.output_width,
            state: self.state,
        })
    }
}

struct OutcomeScanVisitor<'scan> {
    expected: RemoteResultKindV2,
    limits: RemoteReplyDecodeLimitsV2,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> Visitor<'de> for OutcomeScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a V2 response outcome")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut kind = None;
        while let Some(field) = map.next_key::<String>()? {
            match (field.as_str(), self.expected) {
                ("kind", _) => kind = Some(map.next_value::<String>()?),
                ("rows", RemoteResultKindV2::Rows) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::Row,
                        counter: Some((
                            StreamingCounter::Items,
                            sequence_item_limit(self.limits.max_items),
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("documents", RemoteResultKindV2::Documents) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::Document,
                        counter: Some((
                            StreamingCounter::Items,
                            sequence_item_limit(self.limits.max_items),
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("rows", RemoteResultKindV2::HydratedRows)
                | ("entries", RemoteResultKindV2::HydratedPage) => {
                    map.next_value_seed(SequenceScanSeed {
                        child: ScanChild::HydratedRow,
                        counter: Some((
                            StreamingCounter::Items,
                            sequence_item_limit(self.limits.max_items),
                            StreamingPreflightFailure::Items,
                        )),
                        limits: self.limits,
                        local_maximum: None,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("graph", RemoteResultKindV2::HydratedRows | RemoteResultKindV2::HydratedPage) => {
                    map.next_value_seed(ObjectScanSeed {
                        kind: ScanObject::Graph,
                        limits: self.limits,
                        output_width: self.output_width,
                        state: self.state,
                    })?;
                }
                ("value", RemoteResultKindV2::Count | RemoteResultKindV2::DistinctCount) => {
                    if map.next_value::<u64>()? > self.limits.max_items {
                        self.state.failure = Some(StreamingPreflightFailure::Items);
                        return Err(serde::de::Error::custom("V2 item budget exceeded"));
                    }
                }
                ("value", RemoteResultKindV2::Exists | RemoteResultKindV2::DistinctExists) => {
                    if map.next_value::<bool>()? && self.limits.max_items == 0 {
                        self.state.failure = Some(StreamingPreflightFailure::Items);
                        return Err(serde::de::Error::custom("V2 item budget exceeded"));
                    }
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let expected = match self.expected {
            RemoteResultKindV2::Rows => "rows",
            RemoteResultKindV2::Documents => "documents",
            RemoteResultKindV2::Count => "count",
            RemoteResultKindV2::Exists => "exists",
            RemoteResultKindV2::HydratedRows => "hydrated_rows",
            RemoteResultKindV2::HydratedPage => "hydrated_page",
            RemoteResultKindV2::DistinctCount => "distinct_count",
            RemoteResultKindV2::DistinctExists => "distinct_exists",
        };
        if kind.as_deref() != Some(expected) {
            self.state.failure = Some(StreamingPreflightFailure::Outcome);
            return Err(serde::de::Error::custom("V2 outcome kind mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ScanChild {
    Attribute,
    Document,
    DocumentField,
    GraphNode,
    HydratedRow,
    HydratedSlot,
    Ignored,
    Row,
    Role,
}

struct SequenceScanSeed<'scan> {
    child: ScanChild,
    counter: Option<(StreamingCounter, u64, StreamingPreflightFailure)>,
    limits: RemoteReplyDecodeLimitsV2,
    local_maximum: Option<usize>,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for SequenceScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SequenceScanVisitor {
            child: self.child,
            counter: self.counter,
            limits: self.limits,
            local_maximum: self.local_maximum,
            output_width: self.output_width,
            state: self.state,
        })
    }
}

struct SequenceScanVisitor<'scan> {
    child: ScanChild,
    counter: Option<(StreamingCounter, u64, StreamingPreflightFailure)>,
    limits: RemoteReplyDecodeLimitsV2,
    local_maximum: Option<usize>,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> Visitor<'de> for SequenceScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded V2 response sequence")
    }

    fn visit_seq<S>(self, mut sequence: S) -> Result<Self::Value, S::Error>
    where
        S: SeqAccess<'de>,
    {
        let mut local_count = 0_usize;
        loop {
            let present = match self.child {
                ScanChild::Ignored => sequence.next_element::<IgnoredAny>()?.is_some(),
                ScanChild::Document => sequence
                    .next_element_seed(SequenceScanSeed {
                        child: ScanChild::DocumentField,
                        counter: None,
                        limits: self.limits,
                        local_maximum: Some(self.output_width.unwrap_or(MAX_SELECTED_SLOTS)),
                        output_width: self.output_width,
                        state: self.state,
                    })?
                    .is_some(),
                ScanChild::Row => sequence
                    .next_element_seed(SequenceScanSeed {
                        child: ScanChild::Ignored,
                        counter: None,
                        limits: self.limits,
                        local_maximum: Some(self.output_width.unwrap_or(MAX_SELECTED_SLOTS)),
                        output_width: self.output_width,
                        state: self.state,
                    })?
                    .is_some(),
                ScanChild::Attribute
                | ScanChild::DocumentField
                | ScanChild::GraphNode
                | ScanChild::HydratedRow
                | ScanChild::HydratedSlot
                | ScanChild::Role => {
                    let kind = match self.child {
                        ScanChild::Attribute => ScanObject::Attribute,
                        ScanChild::DocumentField => ScanObject::DocumentField,
                        ScanChild::GraphNode => ScanObject::GraphNode,
                        ScanChild::HydratedRow => ScanObject::HydratedRow,
                        ScanChild::HydratedSlot => ScanObject::HydratedSlot,
                        ScanChild::Role => ScanObject::Role,
                        ScanChild::Document | ScanChild::Ignored | ScanChild::Row => {
                            return Err(serde::de::Error::custom(
                                "invalid V2 response preflight state",
                            ));
                        }
                    };
                    sequence
                        .next_element_seed(ObjectScanSeed {
                            kind,
                            limits: self.limits,
                            output_width: self.output_width,
                            state: self.state,
                        })?
                        .is_some()
                }
            };
            if !present {
                break;
            }
            local_count = local_count.saturating_add(1);
            if self
                .local_maximum
                .is_some_and(|maximum| local_count > maximum)
            {
                return Err(serde::de::Error::custom(
                    "V2 response sequence exceeds its output width",
                ));
            }
            if let Some((counter, maximum, failure)) = self.counter {
                self.state
                    .increment(counter, maximum, failure)
                    .map_err(serde::de::Error::custom)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ScanObject {
    Attribute,
    DocumentField,
    Graph,
    GraphNode,
    HydratedRow,
    HydratedSlot,
    Role,
}

struct ObjectScanSeed<'scan> {
    kind: ScanObject,
    limits: RemoteReplyDecodeLimitsV2,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> DeserializeSeed<'de> for ObjectScanSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ObjectScanVisitor {
            kind: self.kind,
            limits: self.limits,
            output_width: self.output_width,
            state: self.state,
        })
    }
}

struct ObjectScanVisitor<'scan> {
    kind: ScanObject,
    limits: RemoteReplyDecodeLimitsV2,
    output_width: Option<usize>,
    state: &'scan mut StreamingPreflightState,
}

impl<'de> Visitor<'de> for ObjectScanVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a V2 response evidence object")
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        while let Some(field) = map.next_key::<String>()? {
            let scan = match (self.kind, field.as_str()) {
                (ScanObject::DocumentField | ScanObject::HydratedSlot, "values") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::CollectionMembers,
                        self.limits.max_collection_members,
                        StreamingPreflightFailure::CollectionMembers,
                    )),
                )),
                (ScanObject::Graph, "nodes") => Some((
                    ScanChild::GraphNode,
                    Some((
                        StreamingCounter::GraphNodes,
                        self.limits.max_graph_nodes,
                        StreamingPreflightFailure::GraphNodes,
                    )),
                )),
                (ScanObject::GraphNode, "attributes") => Some((ScanChild::Attribute, None)),
                (ScanObject::GraphNode, "roles") => Some((ScanChild::Role, None)),
                (ScanObject::Attribute, "values") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::AttributeValues,
                        self.limits.max_attribute_values,
                        StreamingPreflightFailure::AttributeValues,
                    )),
                )),
                (ScanObject::Role, "players") => Some((
                    ScanChild::Ignored,
                    Some((
                        StreamingCounter::RolePlayers,
                        self.limits.max_role_players,
                        StreamingPreflightFailure::RolePlayers,
                    )),
                )),
                (ScanObject::HydratedRow, "slots") => Some((ScanChild::HydratedSlot, None)),
                _ => None,
            };
            if let Some((child, counter)) = scan {
                map.next_value_seed(SequenceScanSeed {
                    child,
                    counter,
                    limits: self.limits,
                    local_maximum: if self.kind == ScanObject::HydratedRow && field == "slots" {
                        Some(self.output_width.unwrap_or(MAX_SELECTED_SLOTS))
                    } else {
                        None
                    },
                    output_width: self.output_width,
                    state: self.state,
                })?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

fn model_output_width(plan: Option<&QueryPlan>, result: RemoteResultKindV2) -> Option<usize> {
    let compatibility = plan?.v2_compatibility()?;
    let model = compatibility.model_query()?;
    match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows { output, .. },
        )
        | (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page { output, .. },
        ) => Some(output.slots().len()),
        _ => None,
    }
}

fn validate_model_rows_against_plan(
    rows: &[HydratedRowV2],
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(model) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch_v2(
            "hydrated result has no model-query contract",
        ));
    };
    match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows {
                cardinality,
                output,
                window,
                ..
            },
        ) => {
            if cardinality.is_exactly_one() && rows.len() != 1 {
                return Err(remote_evidence_mismatch_v2(
                    "exactly-one terminal must carry exactly one hydrated row",
                ));
            }
            if u64::try_from(rows.len()).unwrap_or(u64::MAX) > window.limit() {
                return Err(remote_evidence_mismatch_v2(
                    "hydrated rows exceed the request-bound row window",
                ));
            }
            validate_slot_shapes(rows, output)
        }
        (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page { output, .. },
        ) => validate_slot_shapes(rows, output),
        _ => Err(remote_evidence_mismatch_v2(
            "hydrated outcome contradicts the model-query terminal",
        )),
    }
}

fn validate_graph_against_model_plan(
    graph: &HydrationGraphV2,
    rows: &[HydratedRowV2],
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(model) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch_v2(
            "hydration graph has no request-bound projection",
        ));
    };
    let (output, hydration) = match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows {
                output, hydration, ..
            },
        )
        | (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page {
                output, hydration, ..
            },
        ) => (output, hydration),
        _ => {
            return Err(remote_evidence_mismatch_v2(
                "hydration graph contradicts the model-query terminal",
            ));
        }
    };

    let descriptors = hydration
        .descriptors()
        .iter()
        .map(|descriptor| (descriptor.descriptor(), descriptor))
        .collect::<BTreeMap<_, _>>();
    let slots = output.slots();
    let binding_projections = hydration
        .bindings()
        .iter()
        .map(|binding| (binding.binding(), binding))
        .collect::<BTreeMap<_, _>>();
    let mut full_nodes = BTreeSet::new();
    let mut frontier = Vec::new();
    for row in rows {
        for (slot, slot_contract) in row.slots.iter().zip(&slots) {
            let Some(binding) = binding_projections.get(&slot_contract.binding()) else {
                return Err(remote_evidence_mismatch_v2(
                    "hydrated output binding is absent from the projection",
                ));
            };
            for reference in slot.references() {
                let node_index = usize::try_from(reference.node.get()).map_err(|_| {
                    remote_evidence_mismatch_v2("hydrated output references an unknown node")
                })?;
                let Some(node) = graph.nodes.get(node_index) else {
                    return Err(remote_evidence_mismatch_v2(
                        "hydrated output references an unknown node",
                    ));
                };
                if reference.declared != *binding.declared_descriptor()
                    || !binding.concrete_descriptors().contains(&node.concrete)
                {
                    return Err(remote_evidence_mismatch_v2(
                        "hydrated output violates declared-to-concrete authority",
                    ));
                }
                full_nodes.insert(reference.node);
                frontier.push(reference.node);
            }
        }
    }
    for node in &graph.nodes {
        let Some(projection) = descriptors.get(&node.concrete) else {
            return Err(remote_evidence_mismatch_v2(
                "hydration node concrete descriptor is outside the projection",
            ));
        };
        validate_node_projection(graph, node, projection, full_nodes.contains(&node.id))?;
    }
    let mut unique_values = BTreeMap::<(TypeId, AttributeId), Vec<&CompatibilityValueV2>>::new();
    for node in &graph.nodes {
        let Some(projection) = descriptors.get(&node.concrete) else {
            return Err(remote_evidence_mismatch_v2(
                "hydration node concrete descriptor is outside the projection",
            ));
        };
        let fields = projection
            .fields()
            .iter()
            .map(|field| (field.attribute(), field))
            .collect::<BTreeMap<_, _>>();
        for evidence in &node.attributes {
            let Some(field) = fields.get(&evidence.attribute) else {
                return Err(remote_evidence_mismatch_v2(
                    "hydration attribute is outside the concrete projection",
                ));
            };
            if field.unique() {
                for owner in field.reference_owners() {
                    let authority = (owner.clone(), evidence.attribute.clone());
                    let seen = unique_values.entry(authority).or_default();
                    for value in &evidence.values {
                        seen.push(value);
                    }
                }
            }
        }
    }
    for values in unique_values.values() {
        if contains_semantic_duplicate(values.iter().copied())? {
            return Err(remote_evidence_mismatch_v2(
                "unique hydration field repeats one value across provider identities",
            ));
        }
    }

    let mut referenced = BTreeSet::new();
    while let Some(node_id) = frontier.pop() {
        if !referenced.insert(node_id) {
            continue;
        }
        let node_index = usize::try_from(node_id.get()).map_err(|_| {
            remote_evidence_mismatch_v2("hydration graph traversal reached an unknown node")
        })?;
        let node = graph.nodes.get(node_index).ok_or_else(|| {
            remote_evidence_mismatch_v2("hydration graph traversal reached an unknown node")
        })?;
        if full_nodes.contains(&node_id) {
            for role in &node.roles {
                for player in &role.players {
                    frontier.push(player.node);
                }
            }
        }
    }
    if referenced.len() != graph.nodes.len() {
        return Err(remote_evidence_mismatch_v2(
            "hydration graph contains unreferenced provider identities",
        ));
    }
    Ok(())
}

fn validate_model_order_against_plan(
    graph: &HydrationGraphV2,
    rows: &[HydratedRowV2],
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
) -> Result<(), Diagnostic> {
    let Some(model) = plan
        .and_then(QueryPlan::v2_compatibility)
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Ok(());
    };
    let (output, row_order) = match (result, model) {
        (
            RemoteResultKindV2::HydratedRows,
            crate::query_plan::ModelQueryV2::Rows { order, output, .. },
        ) => (output, order.as_ref()),
        (
            RemoteResultKindV2::HydratedPage,
            crate::query_plan::ModelQueryV2::Page { order, output, .. },
        ) => (output, Some(order)),
        _ => {
            return Err(remote_evidence_mismatch_v2(
                "hydrated result has no request-bound order contract",
            ));
        }
    };

    validate_collection_orders(graph, rows, output)?;
    if let Some(order) = row_order {
        validate_row_order(graph, rows, output, order)?;
    }
    if result == RemoteResultKindV2::HydratedRows {
        validate_selected_row_identities(rows, output)?;
    }
    Ok(())
}

fn validate_selected_row_identities(
    rows: &[HydratedRowV2],
    output: &crate::query_plan::ModelOutputV2,
) -> Result<(), Diagnostic> {
    let singular_bindings = output
        .slots()
        .into_iter()
        .filter(|slot| !slot.collection())
        .map(crate::query_plan::QueryModelOutputSlotV2::binding)
        .collect::<Vec<_>>();
    let mut identities = BTreeSet::new();
    for row in rows {
        let identity = row_identity(row, output, &singular_bindings)?;
        if !identities.insert(identity) {
            return Err(remote_evidence_mismatch_v2(
                "hydrated rows repeat one selected singular-identity tuple",
            ));
        }
    }
    Ok(())
}

fn validate_row_order(
    graph: &HydrationGraphV2,
    rows: &[HydratedRowV2],
    output: &crate::query_plan::ModelOutputV2,
    order: &crate::query_plan::QueryStableOrderV2,
) -> Result<(), Diagnostic> {
    for pair in rows.windows(2) {
        let ordering = compare_rows_by_order(graph, &pair[0], &pair[1], output, order)?;
        if ordering == Ordering::Greater {
            return Err(remote_evidence_mismatch_v2(
                "hydrated rows are not in the request-bound stable order",
            ));
        }
        if ordering == Ordering::Equal
            && row_identity(&pair[0], output, order.identity_tiebreakers())?
                != row_identity(&pair[1], output, order.identity_tiebreakers())?
        {
            return Err(remote_evidence_mismatch_v2(
                "hydrated row order does not distinguish two selected identities",
            ));
        }
    }
    Ok(())
}

fn compare_rows_by_order(
    graph: &HydrationGraphV2,
    left: &HydratedRowV2,
    right: &HydratedRowV2,
    output: &crate::query_plan::ModelOutputV2,
    order: &crate::query_plan::QueryStableOrderV2,
) -> Result<Ordering, Diagnostic> {
    for term in order.terms() {
        let left = row_binding_reference(left, output, term.field().binding())?;
        let right = row_binding_reference(right, output, term.field().binding())?;
        let ordering = compare_order_values(
            order_value(graph, left, term)?,
            order_value(graph, right, term)?,
            term,
        )?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn validate_collection_orders(
    graph: &HydrationGraphV2,
    rows: &[HydratedRowV2],
    output: &crate::query_plan::ModelOutputV2,
) -> Result<(), Diagnostic> {
    let output_slots = output.slots();
    for row in rows {
        for (slot, contract) in row.slots().iter().zip(&output_slots) {
            let (
                HydrationSlotV2::Collection { values },
                crate::query_plan::QueryModelOutputSlotV2::Collect { order, .. },
            ) = (slot, *contract)
            else {
                continue;
            };
            for pair in values.windows(2) {
                let ordering = compare_references_by_order(graph, &pair[0], &pair[1], order)?;
                if ordering == Ordering::Greater {
                    return Err(remote_evidence_mismatch_v2(
                        "hydrated collection members are not in the request-bound stable order",
                    ));
                }
                if ordering == Ordering::Equal && pair[0].node() != pair[1].node() {
                    return Err(remote_evidence_mismatch_v2(
                        "hydrated collection order does not distinguish two provider identities",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn compare_references_by_order(
    graph: &HydrationGraphV2,
    left: &HydrationReferenceV2,
    right: &HydrationReferenceV2,
    order: &crate::query_plan::QueryStableOrderV2,
) -> Result<Ordering, Diagnostic> {
    for term in order.terms() {
        let ordering = compare_order_values(
            order_value(graph, left, term)?,
            order_value(graph, right, term)?,
            term,
        )?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn row_identity(
    row: &HydratedRowV2,
    output: &crate::query_plan::ModelOutputV2,
    bindings: &[BindingId],
) -> Result<Vec<(BindingId, HydrationNodeIdV2)>, Diagnostic> {
    let mut identity = bindings
        .iter()
        .map(|binding| {
            row_binding_reference(row, output, *binding)
                .map(|reference| (*binding, reference.node()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    identity.sort_unstable();
    Ok(identity)
}

fn row_binding_reference<'row>(
    row: &'row HydratedRowV2,
    output: &crate::query_plan::ModelOutputV2,
    binding: BindingId,
) -> Result<&'row HydrationReferenceV2, Diagnostic> {
    let output_slots = output.slots();
    let Some(index) = output_slots
        .iter()
        .position(|slot| slot.binding() == binding)
    else {
        return Err(remote_evidence_mismatch_v2(
            "stable order references a binding absent from the public output",
        ));
    };
    match row.slots().get(index) {
        Some(HydrationSlotV2::Singular { value }) => Ok(value),
        _ => Err(remote_evidence_mismatch_v2(
            "stable row order requires a singular public binding",
        )),
    }
}

fn order_value<'graph>(
    graph: &'graph HydrationGraphV2,
    reference: &HydrationReferenceV2,
    term: &crate::query_plan::QueryOrderTermV2,
) -> Result<Option<&'graph CompatibilityValueV2>, Diagnostic> {
    let index = usize::try_from(reference.node().get()).map_err(|_| {
        remote_evidence_mismatch_v2("stable order references an unknown hydration node")
    })?;
    let node = graph.nodes.get(index).ok_or_else(|| {
        remote_evidence_mismatch_v2("stable order references an unknown hydration node")
    })?;
    let evidence = node
        .attributes
        .iter()
        .find(|evidence| evidence.attribute() == term.field().attribute())
        .ok_or_else(|| {
            remote_evidence_mismatch_v2("stable order attribute is absent from hydration evidence")
        })?;
    if evidence.values().len() > 1 {
        return Err(remote_evidence_mismatch_v2(
            "stable order attribute carries non-scalar hydration evidence",
        ));
    }
    Ok(evidence.values().first())
}

fn compare_order_values(
    left: Option<&CompatibilityValueV2>,
    right: Option<&CompatibilityValueV2>,
    term: &crate::query_plan::QueryOrderTermV2,
) -> Result<Ordering, Diagnostic> {
    let ordering = match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => match term.missing() {
            crate::query_plan::QueryMissingOrderV2::First => Ordering::Less,
            crate::query_plan::QueryMissingOrderV2::Last => Ordering::Greater,
            crate::query_plan::QueryMissingOrderV2::Reject => {
                return Err(remote_evidence_mismatch_v2(
                    "stable order rejects a missing hydration value",
                ));
            }
        },
        (Some(_), None) => match term.missing() {
            crate::query_plan::QueryMissingOrderV2::First => Ordering::Greater,
            crate::query_plan::QueryMissingOrderV2::Last => Ordering::Less,
            crate::query_plan::QueryMissingOrderV2::Reject => {
                return Err(remote_evidence_mismatch_v2(
                    "stable order rejects a missing hydration value",
                ));
            }
        },
        (Some(left), Some(right)) => {
            let ordering = left.semantic_cmp_same_domain(right).ok_or_else(|| {
                remote_evidence_mismatch_v2("stable order values are not semantically comparable")
            })?;
            if term.direction() == crate::query_plan::QueryOrderDirectionV2::Descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
    };
    Ok(ordering)
}

fn validate_node_projection(
    graph: &HydrationGraphV2,
    node: &HydrationNodeV2,
    projection: &crate::query_plan::HydrationDescriptorV2,
    full_output: bool,
) -> Result<(), Diagnostic> {
    if node.attributes.len() != projection.fields().len()
        || (full_output && node.roles.len() != projection.roles().len())
        || (!full_output && !node.roles.is_empty())
    {
        return Err(remote_evidence_mismatch_v2(
            "hydration node contradicts its full-output or shallow-player projection",
        ));
    }
    let fields = projection
        .fields()
        .iter()
        .map(|field| (field.attribute(), field))
        .collect::<BTreeMap<_, _>>();
    for evidence in &node.attributes {
        let Some(field) = fields.get(&evidence.attribute) else {
            return Err(remote_evidence_mismatch_v2(
                "hydration attribute is outside the concrete projection",
            ));
        };
        if !cardinality_allows(field.cardinality(), evidence.values.len())
            || evidence
                .values
                .iter()
                .any(|value| value.value_type() != field.value_type())
        {
            return Err(remote_evidence_mismatch_v2(
                "hydration attribute violates its value type or cardinality",
            ));
        }
        if !field.ordered() {
            for pair in evidence.values.windows(2) {
                let ordering = pair[0].semantic_cmp_same_domain(&pair[1]).ok_or_else(|| {
                    remote_evidence_mismatch_v2(
                        "hydration attribute values are not semantically comparable",
                    )
                })?;
                if ordering != Ordering::Less {
                    return Err(remote_evidence_mismatch_v2(
                        "unordered hydration attribute values must be semantically sorted and unique",
                    ));
                }
            }
        }
        if field.ordered()
            && field.distinct()
            && contains_semantic_duplicate(evidence.values.iter())?
        {
            return Err(remote_evidence_mismatch_v2(
                "distinct ordered hydration attribute contains duplicate values",
            ));
        }
    }
    if !full_output {
        return Ok(());
    }
    let roles = projection
        .roles()
        .iter()
        .map(|role| (role.role(), role))
        .collect::<BTreeMap<_, _>>();
    for evidence in &node.roles {
        let Some(role) = roles.get(&evidence.role) else {
            return Err(remote_evidence_mismatch_v2(
                "hydration role is outside the concrete projection",
            ));
        };
        if !cardinality_allows(role.cardinality(), evidence.players.len()) {
            return Err(remote_evidence_mismatch_v2(
                "hydration role violates its effective cardinality",
            ));
        }
        if !role.ordered()
            && evidence.players.windows(2).any(|pair| {
                (pair[0].node(), pair[0].declared()) > (pair[1].node(), pair[1].declared())
            })
        {
            return Err(remote_evidence_mismatch_v2(
                "unordered hydration role players must be in canonical order",
            ));
        }
        if role.distinct() {
            let distinct_players = evidence
                .players
                .iter()
                .map(HydrationReferenceV2::node)
                .collect::<BTreeSet<_>>();
            if distinct_players.len() != evidence.players.len() {
                return Err(remote_evidence_mismatch_v2(
                    "distinct hydration role contains a repeated provider identity",
                ));
            }
        }
        for player in &evidence.players {
            let index = usize::try_from(player.node.get()).map_err(|_| {
                remote_evidence_mismatch_v2("hydration role references an unknown player")
            })?;
            let Some(player_node) = graph.nodes.get(index) else {
                return Err(remote_evidence_mismatch_v2(
                    "hydration role references an unknown player",
                ));
            };
            let Some(player_authority) = role
                .players()
                .iter()
                .find(|authority| authority.declared_descriptor() == &player.declared)
            else {
                return Err(remote_evidence_mismatch_v2(
                    "hydration role player uses an undeclared descriptor",
                ));
            };
            if !player_authority
                .concrete_descriptors()
                .contains(&player_node.concrete)
            {
                return Err(remote_evidence_mismatch_v2(
                    "hydration role player violates descriptor compatibility",
                ));
            }
        }
    }
    Ok(())
}

fn contains_semantic_duplicate<'value>(
    values: impl IntoIterator<Item = &'value CompatibilityValueV2>,
) -> Result<bool, Diagnostic> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if let Some(first) = values.first().copied() {
        for value in &values[1..] {
            first.semantic_cmp_same_domain(value).ok_or_else(|| {
                remote_evidence_mismatch_v2(
                    "hydration attribute values are not semantically comparable",
                )
            })?;
        }
    }
    let mut incomparable = false;
    values.sort_by(|left, right| match left.semantic_cmp_same_domain(right) {
        Some(ordering) => ordering.then_with(|| left.cmp(right)),
        None => {
            incomparable = true;
            left.cmp(right)
        }
    });
    if incomparable {
        return Err(remote_evidence_mismatch_v2(
            "hydration attribute values are not semantically comparable",
        ));
    }
    Ok(values
        .windows(2)
        .any(|pair| pair[0].semantic_cmp_same_domain(pair[1]) == Some(Ordering::Equal)))
}

fn cardinality_allows(cardinality: crate::value::Cardinality, count: usize) -> bool {
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    count >= cardinality.min() && cardinality.max().is_none_or(|maximum| count <= maximum)
}

fn validate_page_contract(
    plan: Option<&QueryPlan>,
    root: BindingId,
    offset: u64,
    limit: u64,
    has_total: bool,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(crate::query_plan::ModelQueryV2::Page {
        root: expected_root,
        window,
        include_total,
        ..
    }) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch_v2(
            "hydrated page has no request-bound page contract",
        ));
    };
    if root != *expected_root
        || offset != window.offset()
        || limit != window.limit()
        || has_total != *include_total
    {
        return Err(remote_evidence_mismatch_v2(
            "hydrated page root, window, or total presence contradicts the request",
        ));
    }
    Ok(())
}

fn validate_slot_shapes(
    rows: &[HydratedRowV2],
    output: &crate::query_plan::ModelOutputV2,
) -> Result<(), Diagnostic> {
    for row in rows {
        if row.slots.len() != output.slots().len() {
            return Err(remote_evidence_mismatch_v2(
                "hydrated row arity contradicts the model output",
            ));
        }
        for (slot, expected) in row.slots.iter().zip(output.slots()) {
            let shape_matches = match (slot, expected.collection()) {
                (HydrationSlotV2::Singular { value }, false) => {
                    value.declared() == expected.declared()
                }
                (HydrationSlotV2::Collection { values }, true) => values
                    .iter()
                    .all(|value| value.declared() == expected.declared()),
                _ => false,
            };
            if !shape_matches {
                return Err(remote_evidence_mismatch_v2(
                    "hydrated slot shape or declared descriptor contradicts the model output",
                ));
            }
            if expected.distinct()
                && matches!(slot, HydrationSlotV2::Collection { values } if {
                    let unique = values.iter().collect::<BTreeSet<_>>();
                    unique.len() != values.len()
                })
            {
                return Err(remote_evidence_mismatch_v2(
                    "distinct hydrated collection contains duplicate identities",
                ));
            }
        }
    }
    Ok(())
}

fn validate_page_root_distinct(
    entries: &[HydratedRowV2],
    root: BindingId,
    plan: Option<&QueryPlan>,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let Some(crate::query_plan::ModelQueryV2::Page {
        root: expected_root,
        output,
        ..
    }) = plan
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(remote_evidence_mismatch_v2(
            "hydrated page has no page contract",
        ));
    };
    if root != *expected_root {
        return Err(remote_evidence_mismatch_v2(
            "hydrated page root binding contradicts the request",
        ));
    }
    let Some(root_index) = output
        .slots()
        .iter()
        .position(|slot| slot.binding() == root)
    else {
        return Err(remote_evidence_mismatch_v2(
            "hydrated page root is absent from its output",
        ));
    };
    let mut roots = BTreeSet::new();
    for entry in entries {
        let Some(HydrationSlotV2::Singular { value }) = entry.slots.get(root_index) else {
            return Err(remote_evidence_mismatch_v2(
                "hydrated page root slot must be singular",
            ));
        };
        if !roots.insert(value.node()) {
            return Err(remote_evidence_mismatch_v2(
                "hydrated page root identities must be distinct",
            ));
        }
    }
    Ok(())
}

fn validate_scalar_root(
    plan: Option<&QueryPlan>,
    result: RemoteResultKindV2,
    root: BindingId,
) -> Result<(), Diagnostic> {
    let Some(plan) = plan else {
        return Ok(());
    };
    let expected = match (
        result,
        plan.v2_compatibility()
            .and_then(|compatibility| compatibility.model_query()),
    ) {
        (
            RemoteResultKindV2::DistinctCount,
            Some(crate::query_plan::ModelQueryV2::DistinctCount { root, .. }),
        )
        | (
            RemoteResultKindV2::DistinctExists,
            Some(crate::query_plan::ModelQueryV2::DistinctExists { root, .. }),
        ) => *root,
        _ => {
            return Err(remote_evidence_mismatch_v2(
                "distinct-root scalar contradicts the model-query terminal",
            ));
        }
    };
    if root != expected {
        return Err(remote_evidence_mismatch_v2(
            "distinct-root scalar binds the wrong root",
        ));
    }
    Ok(())
}

fn remote_item_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_item_limit",
        "V2 response exceeds the caller item budget",
    )
}

fn remote_collection_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_collection_member_limit",
        "V2 response exceeds the aggregate collection-member budget",
    )
}

fn remote_graph_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_graph_node_limit",
        "V2 hydration graph exceeds the node budget",
    )
}

fn remote_attribute_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_attribute_value_limit",
        "V2 hydration graph exceeds the attribute-value budget",
    )
}

fn remote_role_player_limit_v2() -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_v2_role_player_limit",
        "V2 hydration graph exceeds the role-player budget",
    )
}

fn remote_evidence_mismatch_v2(message: &'static str) -> Diagnostic {
    remote_v2_failure(
        DiagnosticCategory::Integrity,
        "query_remote_v2_evidence_mismatch",
        message,
    )
}

fn remote_v2_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static V2 remote diagnostic code is canonical"),
        message,
    )
}
