//! Canonical staged V3 transactions. Requests are unsigned; replies retain the
//! existing Ed25519 signed outer envelope. No V1/V2 wire type is widened.

use crate::capability::{CapabilityId, CapabilitySet};
use crate::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::limits::{
    DEFAULT_STAGED_READ_STATEMENTS_V3, DEFAULT_STAGED_WRITE_STATEMENTS_V3,
    MAX_REMOTE_ENVELOPE_BYTES, MAX_STAGED_ITEMS_V3, MAX_STAGED_PLANS_V3, MAX_STAGED_STATEMENTS_V3,
    QueryLimits, STAGED_REMOTE_CODEC_LIMITS_V3,
};
use crate::query_plan::v3::{
    Operation, QueryPlanV3, WriteReturns, decode_query_plan_v3_with_limits,
};
use crate::query_plan::{InputRow, QueryOutput};
use crate::query_remote::{
    DEFAULT_REMOTE_DEADLINE_MS, MAX_REMOTE_CLOCK_SKEW_MS, MAX_REMOTE_DEADLINE_MS,
    QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN, RemoteCapabilities, RemoteFieldValue,
    RemoteReplySigner, RemoteReplyVerifier, RemoteSigningPublicKey, RemoteValue,
};
use crate::schema::DeclaredSchema;
use crate::sdk_diagnostic::SdkExecutionDiagnostic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact request discriminator and fingerprint canonicalization.
pub const QUERY_REMOTE_REQUEST_FORMAT_V3: &str = "typebridge.query-remote-request/v3";
/// Exact staged reply payload discriminator.
pub const QUERY_REMOTE_REPLY_FORMAT_V3: &str = "typebridge.query-remote-response/v3";
/// Admission capability for the staged envelope.
pub const CAP_QUERY_REMOTE_STAGED_V3: &str = "query.remote.envelope-v3";
/// Admission capability for authenticated write staging.
pub const CAP_QUERY_REMOTE_WRITE_V3: &str = "query.remote.write";

fn invalid(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::InvalidContract, code, message)
}
fn limit(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::ResourceLimit, code, message)
}
fn mismatch() -> Diagnostic {
    Diagnostic::stable(
        DiagnosticCategory::Integrity,
        "query_remote_v3_binding_mismatch",
        "staged reply does not bind the expected request",
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>, &'static str> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid lowercase byte encoding");
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let digit = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            Ok((digit(pair[0]) << 4) | digit(pair[1]))
        })
        .collect()
}

/// Explicit transaction kind. Schema transactions do not admit staged plans.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedKindV3 {
    /// A read-only transaction, closed instead of committed by execution.
    Read,
    /// A write transaction, including a caller-selected write snapshot for reads.
    Write,
    /// Reserved schema kind; V3 query staging rejects it.
    Schema,
}

/// Aggregate staged answer and execution budgets, independent of frozen V2.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StagedLimitsV3 {
    /// Optional execution deadline; absence means thirty seconds.
    pub deadline_ms: Option<u64>,
    /// Maximum signed successful reply bytes.
    pub max_bytes: u64,
    /// Maximum aggregate answer items across all plans.
    pub max_items: u64,
    /// Maximum ordered plans, at most 64.
    pub max_staged_plans: u32,
    /// Maximum read provider statements, charged during execution.
    pub max_read_statements: u32,
    /// Independent write provider-statement budget.
    pub max_write_statements: u32,
}
impl Default for StagedLimitsV3 {
    fn default() -> Self {
        Self {
            deadline_ms: None,
            max_bytes: MAX_REMOTE_ENVELOPE_BYTES as u64,
            max_items: MAX_STAGED_ITEMS_V3,
            max_staged_plans: MAX_STAGED_PLANS_V3,
            max_read_statements: DEFAULT_STAGED_READ_STATEMENTS_V3,
            max_write_statements: DEFAULT_STAGED_WRITE_STATEMENTS_V3,
        }
    }
}
impl StagedLimitsV3 {
    /// Refuse unsupported, zero, or above-ceiling execution budgets.
    pub fn validate(self) -> Result<Self, Diagnostic> {
        if self.max_bytes == 0
            || self.max_bytes > MAX_REMOTE_ENVELOPE_BYTES as u64
            || self.max_items == 0
            || self.max_items > MAX_STAGED_ITEMS_V3
            || self.max_staged_plans == 0
            || self.max_staged_plans > MAX_STAGED_PLANS_V3
            || self.max_read_statements == 0
            || self.max_read_statements > MAX_STAGED_STATEMENTS_V3
            || self.max_write_statements == 0
            || self.max_write_statements > MAX_STAGED_STATEMENTS_V3
            || self
                .deadline_ms
                .is_some_and(|v| v == 0 || v > MAX_REMOTE_DEADLINE_MS)
        {
            return Err(limit(
                "query_remote_v3_limits",
                "staged budgets exceed the supported range",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct PlanWire {
    plan_bytes: String,
    rows: Vec<InputRow>,
}
impl<'de> Deserialize<'de> for PlanWire {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            plan_bytes: String,
            rows: Vec<Vec<Option<crate::value::CanonicalValue>>>,
        }
        let raw = Raw::deserialize(d)?;
        Ok(Self {
            plan_bytes: raw.plan_bytes,
            rows: raw.rows.into_iter().map(InputRow::new).collect(),
        })
    }
}
/// One canonical plan and positional invocation rows in an ordered stack.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedPlanV3 {
    wire: PlanWire,
    plan: QueryPlanV3,
}
impl StagedPlanV3 {
    /// Validate one plan under schema authority and bind its invocation rows.
    pub fn new(
        plan: &QueryPlanV3,
        rows: Vec<InputRow>,
        schema: &DeclaredSchema,
    ) -> Result<Self, Diagnostic> {
        Self::rebuild(
            PlanWire {
                plan_bytes: hex(&plan.canonical_bytes()?),
                rows,
            },
            schema,
            plan.limits(),
        )
    }
    fn rebuild(
        wire: PlanWire,
        schema: &DeclaredSchema,
        limits: QueryLimits,
    ) -> Result<Self, Diagnostic> {
        // The outer codec bounds the string before the embedded bytes are allocated.
        if wire.plan_bytes.len() > limits.codec.max_bytes.saturating_mul(2) {
            return Err(limit(
                "query_remote_v3_plan_bytes",
                "embedded plan exceeds its byte ceiling",
            ));
        }
        let bytes = unhex(&wire.plan_bytes).map_err(|_| {
            invalid(
                "query_remote_v3_plan_bytes",
                "embedded plan bytes are not canonical lowercase hexadecimal",
            )
        })?;
        let plan = decode_query_plan_v3_with_limits(&bytes, schema, limits)?;
        if let Some(bound) = plan.given_rows() {
            if bound != wire.rows {
                return Err(invalid(
                    "query_remote_v3_given_mismatch",
                    "bound plan rows differ from staged invocation rows",
                ));
            }
        } else if plan.inputs().is_empty() {
            if !wire.rows.is_empty() {
                return Err(invalid(
                    "query_remote_v3_unexpected_rows",
                    "a plan without columns has no invocation rows",
                ));
            }
        } else {
            plan.with_given_rows(wire.rows.clone(), schema)?;
        }
        Ok(Self { wire, plan })
    }
    /// Original trusted plan, whose operation selects the answer kind.
    pub fn plan(&self) -> &QueryPlanV3 {
        &self.plan
    }
    /// Exact positional invocation rows.
    pub fn rows(&self) -> &[InputRow] {
        &self.wire.rows
    }
    /// Materialize invocation values as a trusted plan for the staged executor.
    pub fn bound_plan(&self, schema: &DeclaredSchema) -> Result<QueryPlanV3, Diagnostic> {
        if self.plan.inputs().is_empty() || self.plan.given_rows().is_some() {
            Ok(self.plan.clone())
        } else {
            self.plan.with_given_rows(self.wire.rows.clone(), schema)
        }
    }
}

/// Untrusted request constructor inputs. Expiry is derived from preparation time.
pub struct StagedRequestSpecV3 {
    /// Caller nonce, 16..128 ASCII letters, digits, hyphens or underscores.
    pub nonce: String,
    /// Optional kind, otherwise inferred from all plan operations.
    pub kind: Option<StagedKindV3>,
    /// Aggregate execution budgets.
    pub limits: StagedLimitsV3,
    /// Ordered transaction stack.
    pub plans: Vec<StagedPlanV3>,
    /// Caller preparation timestamp in Unix milliseconds.
    pub prepared_at_unix_ms: u64,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RequestWire {
    advertisement: String,
    expires_at_unix_ms: u64,
    format: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    kind: Option<StagedKindV3>,
    limits: StagedLimitsV3,
    nonce: String,
    plans: Vec<PlanWire>,
    prepared_at_unix_ms: u64,
}
/// Validated canonical staged request; no execution opens at construction.
#[derive(Clone, Debug, PartialEq)]
pub struct StagedRequestV3 {
    wire: RequestWire,
    plans: Vec<StagedPlanV3>,
}
impl StagedRequestV3 {
    /// Construct a request bound to one exact executor advertisement.
    pub fn new(
        spec: StagedRequestSpecV3,
        schema: &DeclaredSchema,
        advertisement: &RemoteCapabilities,
    ) -> Result<Self, Diagnostic> {
        let limits = spec.limits.validate()?;
        let expires = spec
            .prepared_at_unix_ms
            .checked_add(limits.deadline_ms.unwrap_or(DEFAULT_REMOTE_DEADLINE_MS))
            .ok_or_else(|| invalid("query_remote_v3_time", "staged request lifetime is invalid"))?;
        let wire = RequestWire {
            advertisement: advertisement.fingerprint()?.digest_hex(),
            expires_at_unix_ms: expires,
            format: QUERY_REMOTE_REQUEST_FORMAT_V3.into(),
            kind: spec.kind,
            limits,
            nonce: spec.nonce,
            plans: spec.plans.into_iter().map(|p| p.wire).collect(),
            prepared_at_unix_ms: spec.prepared_at_unix_ms,
        };
        Self::rebuild(wire, schema, advertisement, QueryLimits::CANONICAL)
    }
    fn rebuild(
        wire: RequestWire,
        schema: &DeclaredSchema,
        advertisement: &RemoteCapabilities,
        receiver: QueryLimits,
    ) -> Result<Self, Diagnostic> {
        wire.limits.validate()?;
        if wire.format != QUERY_REMOTE_REQUEST_FORMAT_V3 {
            return Err(invalid(
                "query_remote_v3_format",
                "unsupported staged request format",
            ));
        }
        if wire.advertisement != advertisement.fingerprint()?.digest_hex() {
            return Err(mismatch());
        }
        if !(16..=128).contains(&wire.nonce.len())
            || !wire
                .nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(invalid(
                "query_remote_v3_nonce",
                "staged nonce is outside its canonical grammar",
            ));
        }
        if wire
            .expires_at_unix_ms
            .checked_sub(wire.prepared_at_unix_ms)
            != Some(
                wire.limits
                    .deadline_ms
                    .unwrap_or(DEFAULT_REMOTE_DEADLINE_MS),
            )
        {
            return Err(invalid(
                "query_remote_v3_time",
                "staged request lifetime is invalid",
            ));
        }
        if wire.plans.is_empty() || wire.plans.len() > wire.limits.max_staged_plans as usize {
            return Err(limit(
                "query_remote_v3_plan_limit",
                "staged plan count exceeds its budget",
            ));
        }
        let advertised = advertisement.query_limits();
        let limits = QueryLimits::from_overrides(QueryLimits::FIELDS.iter().map(|name| {
            (
                *name,
                receiver
                    .get(name)
                    .unwrap()
                    .min(advertised.get(name).unwrap()) as u64,
            )
        }))
        .map_err(|_| {
            invalid(
                "query_remote_v3_query_limits",
                "receiver query limits are invalid",
            )
        })?;
        let plans = wire
            .plans
            .iter()
            .cloned()
            .map(|p| StagedPlanV3::rebuild(p, schema, limits))
            .collect::<Result<Vec<_>, _>>()?;
        let writes = plans
            .iter()
            .filter(|p| matches!(p.plan.operation(), Operation::Write { .. }))
            .count();
        if wire.kind == Some(StagedKindV3::Schema)
            || (writes > 0 && wire.kind == Some(StagedKindV3::Read))
        {
            return Err(invalid(
                "query_remote_v3_kind",
                "transaction kind refuses the staged operations",
            ));
        }
        if writes > wire.limits.max_write_statements as usize
            || plans.len() - writes > wire.limits.max_read_statements as usize
        {
            return Err(limit(
                "query_remote_v3_statement_limit",
                "staged plans exceed their minimum statement budget",
            ));
        }
        let mut capabilities = CapabilitySet::new();
        capabilities.insert(CapabilityId::new(CAP_QUERY_REMOTE_STAGED_V3)?);
        if writes > 0 {
            capabilities.insert(CapabilityId::new(CAP_QUERY_REMOTE_WRITE_V3)?);
        }
        for plan in &plans {
            for capability in plan.plan.required_capabilities().iter() {
                capabilities.insert(capability.clone());
            }
        }
        capabilities.ensure_supported_by(advertisement.capabilities())?;
        let request = Self { wire, plans };
        request.encode()?;
        Ok(request)
    }
    /// Decode canonical bytes through schema and advertisement validation.
    pub fn decode(
        bytes: &[u8],
        schema: &DeclaredSchema,
        advertisement: &RemoteCapabilities,
    ) -> Result<Self, Diagnostic> {
        Self::decode_with_limits(bytes, schema, advertisement, QueryLimits::CANONICAL)
    }
    /// Decode with receiver-owned query limits; senders cannot widen them.
    pub fn decode_with_limits(
        bytes: &[u8],
        schema: &DeclaredSchema,
        advertisement: &RemoteCapabilities,
        limits: QueryLimits,
    ) -> Result<Self, Diagnostic> {
        limits.validate().map_err(|_| {
            invalid(
                "query_remote_v3_query_limits",
                "receiver query limits are invalid",
            )
        })?;
        let wire = from_canonical_json_with_limits(
            bytes,
            crate::limits::CodecLimits {
                max_bytes: limits.remote_request_bytes.min(MAX_REMOTE_ENVELOPE_BYTES),
                ..STAGED_REMOTE_CODEC_LIMITS_V3
            },
        )?;
        let request = Self::rebuild(wire, schema, advertisement, limits)?;
        if request.encode()?.as_slice() != bytes {
            return Err(invalid(
                "query_remote_v3_noncanonical",
                "staged request reconstruction changes canonical bytes",
            ));
        }
        Ok(request)
    }
    /// Encode exact canonical request bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(&self.wire, STAGED_REMOTE_CODEC_LIMITS_V3)
    }
    /// Fingerprint the complete request, including key, kind, rows and expiry.
    pub fn fingerprint(&self) -> Result<Fingerprint, Diagnostic> {
        Ok(Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_REMOTE_REQUEST_FORMAT_V3)?,
            None,
            &self.encode()?,
        ))
    }
    /// Validate freshness separately from canonical archival decoding.
    pub fn validate_at(&self, now_unix_ms: u64) -> Result<(), Diagnostic> {
        if now_unix_ms >= self.wire.expires_at_unix_ms
            || self.wire.prepared_at_unix_ms > now_unix_ms.saturating_add(MAX_REMOTE_CLOCK_SKEW_MS)
        {
            return Err(invalid(
                "query_remote_v3_expired",
                "staged request is expired or prepared beyond the clock-skew allowance",
            ));
        }
        Ok(())
    }
    /// Exact advertisement fingerprint binding.
    pub fn advertisement(&self) -> &str {
        &self.wire.advertisement
    }
    /// Exact caller nonce.
    pub fn nonce(&self) -> &str {
        &self.wire.nonce
    }
    /// Absolute expiry in Unix milliseconds.
    pub fn expires_at_unix_ms(&self) -> u64 {
        self.wire.expires_at_unix_ms
    }
    /// Aggregate execution budgets.
    pub fn limits(&self) -> StagedLimitsV3 {
        self.wire.limits
    }
    /// Ordered validated plans and invocation rows.
    pub fn plans(&self) -> &[StagedPlanV3] {
        &self.plans
    }
    /// Selected or inferred transaction kind.
    pub fn kind(&self) -> StagedKindV3 {
        self.wire.kind.unwrap_or_else(|| {
            if self
                .plans
                .iter()
                .any(|p| matches!(p.plan.operation(), Operation::Write { .. }))
            {
                StagedKindV3::Write
            } else {
                StagedKindV3::Read
            }
        })
    }
}

/// Per-plan successful evidence. Array position binds each answer to its plan.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StagedAnswerV3 {
    /// Positional typed rows, including write-returning rows.
    Rows {
        /// Complete rows.
        rows: Vec<Vec<RemoteValue>>,
    },
    /// Named fetched documents.
    Documents {
        /// Complete documents.
        documents: Vec<BTreeMap<String, RemoteFieldValue>>,
    },
    /// Read row count.
    Count {
        /// Exact count.
        value: u64,
    },
    /// Read presence.
    Exists {
        /// Whether any row exists.
        value: bool,
    },
    /// Number of rows matched by a write pipeline.
    Matched {
        /// Exact matched count.
        value: u64,
    },
}
/// Typed server outcome, sharing the existing SDK diagnostic authority.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum StagedOutcomeV3 {
    /// The complete stack succeeded; read transactions were closed successfully.
    Committed,
    /// The stack failed; diagnostic certainty governs retry safety.
    Failed {
        /// Existing SDK diagnostic, including additive phase-B failures.
        diagnostic: SdkExecutionDiagnostic,
    },
}

/// Authenticated wire projection of an SDK diagnostic, not a second failure
/// model or a decoder that weakens implementation-owned static SDK messages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SdkDiagnosticProjectionV3 {
    version: u16,
    category: String,
    sdk_category: String,
    query_category: Option<String>,
    code: String,
    message: String,
    path: Vec<serde_json::Value>,
    details: BTreeMap<String, serde_json::Value>,
}
impl<'de> Deserialize<'de> for SdkDiagnosticProjectionV3 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u16,
            category: String,
            sdk_category: String,
            query_category: Option<String>,
            code: String,
            message: String,
            path: Vec<serde_json::Value>,
            details: BTreeMap<String, serde_json::Value>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let projection = Self {
            version: wire.version,
            category: wire.category,
            sdk_category: wire.sdk_category,
            query_category: wire.query_category,
            code: wire.code,
            message: wire.message,
            path: wire.path,
            details: wire.details,
        };
        projection
            .validate()
            .map_err(|_| serde::de::Error::custom("invalid SDK diagnostic projection"))?;
        Ok(projection)
    }
}
impl SdkDiagnosticProjectionV3 {
    fn from_diagnostic(diagnostic: &SdkExecutionDiagnostic) -> Result<Self, Diagnostic> {
        let value = serde_json::to_value(diagnostic.host_json()).map_err(|_| bad_diagnostic())?;
        let projection: Self = serde_json::from_value(value).map_err(|_| bad_diagnostic())?;
        projection.validate()?;
        Ok(projection)
    }
    /// Retain the exact established host JSON form for the SDK exception mapper.
    pub fn host_json(&self) -> Result<serde_json::Value, Diagnostic> {
        serde_json::to_value(self).map_err(|_| bad_diagnostic())
    }
    /// Stable diagnostic code.
    pub fn code(&self) -> &str {
        &self.code
    }
    /// The typed failure the signed details carry, as the server classified
    /// it ([`SdkExecutionDiagnostic::failure_from_host_json`]).
    #[must_use]
    pub fn failure(&self) -> Option<crate::failure::Failure> {
        SdkExecutionDiagnostic::failure_from_host_json(&self.host_json().ok()?)
    }
    fn validate(&self) -> Result<(), Diagnostic> {
        use crate::sdk_diagnostic::*;
        if self.version != 1
            || !matches!(
                self.sdk_category.as_str(),
                "invalid_input"
                    | "unsupported_capability"
                    | "resource_limit"
                    | "integrity"
                    | "provider"
                    | "transaction"
                    | "cancelled"
                    | "internal"
            )
            || self.query_category.as_deref().is_some_and(|q| {
                !matches!(
                    q,
                    "invalid_plan"
                        | "cardinality"
                        | "unsupported_capability"
                        | "stale_schema"
                        | "resource_limit"
                        | "cancelled"
                        | "provider"
                        | "result_decode"
                )
            })
            || self.category != self.query_category.as_deref().unwrap_or(&self.sdk_category)
            || self.message.is_empty()
            || self.message.len() > MAX_SDK_DIAGNOSTIC_MESSAGE_BYTES
            || self.message.trim() != self.message
            || self
                .message
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | '\\'))
            || self.path.len() > MAX_SDK_DIAGNOSTIC_PATH_SEGMENTS
            || self.details.len() > MAX_SDK_DIAGNOSTIC_DETAILS
        {
            return Err(bad_diagnostic());
        }
        if let Some(query) = self.query_category.as_deref() {
            let expected = match query {
                "invalid_plan" | "cardinality" => "invalid_input",
                "stale_schema" | "result_decode" => "integrity",
                other => other,
            };
            if self.sdk_category != expected {
                return Err(bad_diagnostic());
            }
        }
        SdkDiagnosticCode::new(self.code.clone()).map_err(|_| bad_diagnostic())?;
        for segment in &self.path {
            validate_sdk_segment(segment)?;
        }
        for (key, value) in &self.details {
            SdkDiagnosticName::new(key.clone()).map_err(|_| bad_diagnostic())?;
            validate_sdk_detail(value)?;
        }
        if to_canonical_json_with_limits(self, STAGED_REMOTE_CODEC_LIMITS_V3)?.len()
            > crate::limits::MAX_DIAGNOSTIC_BYTES
        {
            return Err(limit(
                "query_remote_v3_diagnostic_limit",
                "SDK diagnostic exceeds its byte ceiling",
            ));
        }
        Ok(())
    }
}
fn bad_diagnostic() -> Diagnostic {
    invalid(
        "query_remote_v3_diagnostic",
        "staged reply has an invalid SDK diagnostic projection",
    )
}
fn tagged(value: &serde_json::Value) -> Result<(&str, Option<&serde_json::Value>), Diagnostic> {
    let object = value.as_object().ok_or_else(bad_diagnostic)?;
    let kind = object
        .get("kind")
        .and_then(|v| v.as_str())
        .ok_or_else(bad_diagnostic)?;
    if object.len() != 1 + usize::from(object.contains_key("value")) {
        return Err(bad_diagnostic());
    }
    Ok((kind, object.get("value")))
}
fn query_identity(value: &serde_json::Value) -> Result<(), Diagnostic> {
    crate::sdk_diagnostic::SdkQueryDiagnosticIdentity::new(
        value.as_str().ok_or_else(bad_diagnostic)?.to_owned(),
    )
    .map(|_| ())
    .map_err(|_| bad_diagnostic())
}
fn typed_identity<T: serde::de::DeserializeOwned + Serialize>(value: &serde_json::Value) -> bool {
    serde_json::from_value::<T>(value.clone())
        .is_ok_and(|decoded| serde_json::to_value(decoded).is_ok_and(|encoded| encoded == *value))
}
fn ownership_identity(value: &serde_json::Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    if fields.len() != 2 {
        return false;
    }
    let Some(owner) = fields.get("owner") else {
        return false;
    };
    let Some(attribute) = fields.get("attribute").and_then(|value| value.as_str()) else {
        return false;
    };
    if !typed_identity::<crate::id::TypeId>(owner) {
        return false;
    }
    let Ok(owner) = serde_json::from_value::<crate::id::TypeId>(owner.clone()) else {
        return false;
    };
    let Ok(attribute) = crate::id::AttributeId::new(attribute) else {
        return false;
    };
    crate::schema::OwnsFactId::new(owner, attribute).is_ok()
}

fn validate_sdk_segment(value: &serde_json::Value) -> Result<(), Diagnostic> {
    let (kind, payload) = tagged(value)?;
    if matches!(
        kind,
        "request" | "plan" | "operation" | "predicate" | "output" | "provider_evidence" | "result"
    ) {
        return if payload.is_none() {
            Ok(())
        } else {
            Err(bad_diagnostic())
        };
    }
    let value = payload.ok_or_else(bad_diagnostic)?;
    let valid = match kind {
        "argument" => value
            .as_str()
            .is_some_and(|s| crate::sdk_diagnostic::SdkDiagnosticName::new(s.to_owned()).is_ok()),
        "index" => value.as_u64().is_some(),
        "binding" => value.as_u64().is_some_and(|v| v < 256),
        "role_edge" | "output_slot" => value.as_u64().is_some_and(|v| v <= u64::from(u16::MAX)),
        "output_name" | "contract_field" | "contract_identity" => query_identity(value).is_ok(),
        "type" => typed_identity::<crate::id::TypeId>(value),
        "field" => ownership_identity(value) || owned_name(value),
        "role" => typed_identity::<crate::id::RoleId>(value) || owned_name(value),
        _ => false,
    };
    if valid { Ok(()) } else { Err(bad_diagnostic()) }
}
fn owned_name(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|v| {
        v.len() == 2
            && v.get("owner").is_some_and(|x| query_identity(x).is_ok())
            && v.get("name").is_some_and(|x| query_identity(x).is_ok())
    })
}
fn validate_sdk_detail(value: &serde_json::Value) -> Result<(), Diagnostic> {
    let (kind, payload) = tagged(value)?;
    let payload = payload.ok_or_else(bad_diagnostic)?;
    let valid = match kind {
        "boolean" => payload.is_boolean(),
        "count" | "byte_count" => payload
            .as_str()
            .is_some_and(|s| s.parse::<u64>().is_ok_and(|v| v.to_string() == s)),
        "signed" => payload
            .as_str()
            .is_some_and(|s| s.parse::<i64>().is_ok_and(|v| v.to_string() == s)),
        "query_identity" => query_identity(payload).is_ok(),
        "query_identity_list" => payload.as_array().is_some_and(|v| {
            v.len() <= crate::sdk_diagnostic::MAX_SDK_QUERY_DIAGNOSTIC_IDENTITY_LIST
                && v.iter().all(|x| query_identity(x).is_ok())
        }),
        "text" => {
            payload.as_str().is_some_and(|s| {
                CapabilityId::new(s).is_ok()
                    || crate::sdk_diagnostic::SdkDiagnosticName::new(s.to_owned()).is_ok()
            }) || typed_identity::<crate::id::TypeId>(payload)
                || typed_identity::<crate::id::RoleId>(payload)
                || ownership_identity(payload)
                || typed_identity::<Fingerprint>(payload)
        }
        _ => false,
    };
    if valid { Ok(()) } else { Err(bad_diagnostic()) }
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum OutcomeWire {
    Committed,
    Failed {
        diagnostic: SdkDiagnosticProjectionV3,
    },
}
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ReplyWire {
    answers: Vec<StagedAnswerV3>,
    format: String,
    nonce: String,
    outcome: OutcomeWire,
    request: String,
}
/// Server-side reply authoring uses only the typed SDK failure authority.
#[derive(Clone, Debug)]
pub struct StagedReplyV3 {
    wire: ReplyWire,
}
impl StagedReplyV3 {
    /// Bind an all-or-nothing outcome and ordered answers to one request.
    pub fn new(
        request: &StagedRequestV3,
        outcome: StagedOutcomeV3,
        answers: Vec<StagedAnswerV3>,
    ) -> Result<Self, Diagnostic> {
        let outcome = match outcome {
            StagedOutcomeV3::Committed => OutcomeWire::Committed,
            StagedOutcomeV3::Failed { diagnostic } => OutcomeWire::Failed {
                diagnostic: SdkDiagnosticProjectionV3::from_diagnostic(&diagnostic)?,
            },
        };
        let wire = ReplyWire {
            answers,
            format: QUERY_REMOTE_REPLY_FORMAT_V3.into(),
            nonce: request.nonce().into(),
            outcome,
            request: request.fingerprint()?.digest().to_hex(),
        };
        validate_reply(&wire, request)?;
        Ok(Self { wire })
    }
    /// Canonical unsigned payload, used only inside the signed outer envelope.
    pub fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(&self.wire, STAGED_REMOTE_CODEC_LIMITS_V3)
    }
    /// Sign with the unchanged V1 outer envelope, bound to the exact advertisement.
    pub fn encode_signed(
        &self,
        request: &StagedRequestV3,
        advertisement: &RemoteCapabilities,
        signer: &impl RemoteReplySigner,
    ) -> Result<Vec<u8>, Diagnostic> {
        validate_reply(&self.wire, request)?;
        if request.advertisement() != advertisement.fingerprint()?.digest_hex()
            || signer.public_key() != advertisement.reply_key()
        {
            return Err(mismatch());
        }
        let bytes = crate::query_remote::encode_signed_reply_payload(
            &self.encode_payload()?,
            &advertisement.fingerprint()?,
            signer,
        )?;
        if matches!(self.wire.outcome, OutcomeWire::Committed)
            && bytes.len() as u64 > request.limits().max_bytes
        {
            return Err(limit(
                "query_remote_v3_reply_bytes",
                "staged successful reply exceeds its byte budget",
            ));
        }
        Ok(bytes)
    }
}
/// Authenticated decoded evidence. The SDK diagnostic remains its established
/// host-JSON wire projection; it is never converted into a new runtime failure.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedStagedReplyV3 {
    wire: ReplyWire,
}
impl VerifiedStagedReplyV3 {
    /// Authenticate before interpreting any outcome or SDK diagnostic content.
    pub fn decode(
        bytes: &[u8],
        request: &StagedRequestV3,
        advertisement: &RemoteCapabilities,
        trusted_key: RemoteSigningPublicKey,
        verifier: &impl RemoteReplyVerifier,
    ) -> Result<Self, Diagnostic> {
        if request.advertisement() != advertisement.fingerprint()?.digest_hex()
            || trusted_key != advertisement.reply_key()
        {
            return Err(mismatch());
        }
        crate::query_remote::preflight_signed_reply_size(bytes, MAX_REMOTE_ENVELOPE_BYTES as u64)?;
        let payload = crate::query_remote::verify_signed_reply_payload(
            bytes,
            &advertisement.fingerprint()?,
            trusted_key,
            verifier,
        )?;
        preflight_reply_payload(payload, bytes.len(), request)?;
        let wire: ReplyWire =
            from_canonical_json_with_limits(payload, STAGED_REMOTE_CODEC_LIMITS_V3)?;
        validate_reply(&wire, request)?;
        if to_canonical_json_with_limits(&wire, STAGED_REMOTE_CODEC_LIMITS_V3)?.as_slice()
            != payload
        {
            return Err(invalid(
                "query_remote_v3_noncanonical",
                "staged reply reconstruction changes canonical bytes",
            ));
        }
        if matches!(wire.outcome, OutcomeWire::Committed)
            && bytes.len() as u64 > request.limits().max_bytes
        {
            return Err(limit(
                "query_remote_v3_reply_bytes",
                "staged successful reply exceeds its byte budget",
            ));
        }
        Ok(Self { wire })
    }
    /// Whether every plan succeeded and its answers are final.
    pub fn committed(&self) -> bool {
        matches!(self.wire.outcome, OutcomeWire::Committed)
    }
    /// Ordered final answers, empty on failure.
    pub fn answers(&self) -> &[StagedAnswerV3] {
        &self.wire.answers
    }
    /// Established SDK diagnostic projection when the transaction failed.
    pub fn diagnostic(&self) -> Option<&SdkDiagnosticProjectionV3> {
        match &self.wire.outcome {
            OutcomeWire::Failed { diagnostic } => Some(diagnostic),
            OutcomeWire::Committed => None,
        }
    }
    /// Exact canonical authenticated payload for fixture round-trips.
    pub fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(&self.wire, STAGED_REMOTE_CODEC_LIMITS_V3)
    }
}
fn validate_reply(wire: &ReplyWire, request: &StagedRequestV3) -> Result<(), Diagnostic> {
    if wire.format != QUERY_REMOTE_REPLY_FORMAT_V3
        || wire.nonce != request.nonce()
        || wire.request != request.fingerprint()?.digest().to_hex()
    {
        return Err(mismatch());
    }
    if let OutcomeWire::Failed { diagnostic } = &wire.outcome {
        diagnostic.validate()?;
        return if wire.answers.is_empty() {
            Ok(())
        } else {
            Err(invalid(
                "query_remote_v3_partial_answers",
                "failed transactions do not expose partial answers",
            ))
        };
    }
    if wire.answers.len() != request.plans().len() {
        return Err(invalid(
            "query_remote_v3_answer_count",
            "answer count does not match the staged stack",
        ));
    }
    let mut items = 0_u64;
    for (answer, entry) in wire.answers.iter().zip(request.plans()) {
        items = items.saturating_add(validate_answer(answer, entry.plan())?);
        if items > request.limits().max_items {
            return Err(limit(
                "query_remote_v3_answer_items",
                "aggregate staged answers exceed their item budget",
            ));
        }
    }
    Ok(())
}
fn validate_answer(answer: &StagedAnswerV3, plan: &QueryPlanV3) -> Result<u64, Diagnostic> {
    let bad = || {
        invalid(
            "query_remote_v3_answer_kind",
            "staged evidence does not match its plan operation and output",
        )
    };
    match (answer, plan.operation(), plan.output()) {
        (StagedAnswerV3::Count { .. }, Operation::Count, _)
        | (StagedAnswerV3::Exists { .. }, Operation::Exists, _)
        | (
            StagedAnswerV3::Matched { .. },
            Operation::Write {
                returns: WriteReturns::Matched,
            },
            _,
        ) => Ok(1),
        (
            StagedAnswerV3::Rows { rows },
            Operation::Rows
            | Operation::Write {
                returns: WriteReturns::Rows,
            },
            QueryOutput::Rows { columns },
        ) => {
            for row in rows {
                if row.len() != columns.len() {
                    return Err(bad());
                }
                for value in row {
                    match value {
                        RemoteValue::Thing { iid, type_id }
                            if !crate::id::is_canonical_thing_iid(iid)
                                || !matches!(
                                    type_id.kind(),
                                    crate::id::TypeKind::Entity | crate::id::TypeKind::Relation
                                ) =>
                        {
                            return Err(bad());
                        }
                        RemoteValue::Attribute { type_id, .. }
                            if type_id.kind() != crate::id::TypeKind::Attribute =>
                        {
                            return Err(bad());
                        }
                        _ => {}
                    }
                }
            }
            Ok(rows.len() as u64)
        }
        (
            StagedAnswerV3::Documents { documents },
            Operation::Rows
            | Operation::Write {
                returns: WriteReturns::Rows,
            },
            QueryOutput::Documents { fields },
        ) => {
            for document in documents {
                if document.len() != fields.len()
                    || fields
                        .iter()
                        .any(|field| !document.contains_key(field.key().as_str()))
                {
                    return Err(bad());
                }
                for field in fields {
                    if matches!(
                        field.source(),
                        crate::query_plan::DocumentSource::AttributeList { .. }
                    ) && !matches!(
                        document.get(field.key().as_str()),
                        Some(RemoteFieldValue::List { .. })
                    ) {
                        return Err(bad());
                    }
                }
            }
            Ok(documents.len() as u64)
        }
        _ => Err(bad()),
    }
}

// Authentication has already succeeded. Borrow payload fragments so tighter
// success and diagnostic budgets are checked before their owned DTOs allocate.
fn preflight_reply_payload(
    payload: &[u8],
    envelope_bytes: usize,
    request: &StagedRequestV3,
) -> Result<(), Diagnostic> {
    #[derive(Deserialize)]
    struct Peek<'a> {
        #[serde(borrow)]
        format: &'a str,
        #[serde(borrow)]
        nonce: &'a str,
        #[serde(borrow)]
        request: &'a str,
        #[serde(borrow)]
        outcome: &'a serde_json::value::RawValue,
        #[serde(borrow)]
        answers: &'a serde_json::value::RawValue,
    }
    #[derive(Deserialize)]
    struct OutcomePeek<'a> {
        #[serde(borrow)]
        kind: &'a str,
        #[serde(borrow)]
        diagnostic: Option<&'a serde_json::value::RawValue>,
    }
    let peek: Peek<'_> = serde_json::from_slice(payload).map_err(|_| {
        invalid(
            "query_remote_v3_reply_shape",
            "staged reply has an invalid framing shape",
        )
    })?;
    if peek.format != QUERY_REMOTE_REPLY_FORMAT_V3
        || peek.nonce != request.nonce()
        || peek.request != request.fingerprint()?.digest().to_hex()
    {
        return Err(mismatch());
    }
    let outcome: OutcomePeek<'_> = serde_json::from_str(peek.outcome.get()).map_err(|_| {
        invalid(
            "query_remote_v3_reply_shape",
            "staged reply has an invalid outcome framing",
        )
    })?;
    match outcome.kind {
        "committed" if envelope_bytes as u64 > request.limits().max_bytes => Err(limit(
            "query_remote_v3_reply_bytes",
            "staged successful reply exceeds its byte budget",
        )),
        "committed" => Ok(()),
        "failed" => {
            if peek.answers.get() != "[]" {
                return Err(invalid(
                    "query_remote_v3_partial_answers",
                    "failed transactions do not expose partial answers",
                ));
            }
            let diagnostic = outcome.diagnostic.ok_or_else(|| {
                invalid(
                    "query_remote_v3_reply_shape",
                    "staged failed reply is missing its diagnostic",
                )
            })?;
            if diagnostic.get().len() > crate::limits::MAX_DIAGNOSTIC_BYTES {
                return Err(limit(
                    "query_remote_v3_diagnostic_limit",
                    "SDK diagnostic exceeds its byte ceiling",
                ));
            }
            Ok(())
        }
        _ => Err(invalid(
            "query_remote_v3_reply_shape",
            "staged reply has an unsupported outcome",
        )),
    }
}
