//! Versioned fail-closed wire envelopes for remote query execution.
//!
//! One validated plan/result contract serves direct and server execution:
//! the request carries the exact canonical plan bytes plus the invocation
//! (operation, input rows, caller budgets), the exact executor advertisement,
//! an absolute bounded expiry, and a caller nonce; the response binds that
//! nonce and the whole request fingerprint so replayed or foreign evidence is
//! rejected before any host object is constructed. Envelope formats are
//! versioned independently of the plan format.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::TypeId;
use crate::limits::{
    MAX_REMOTE_ENVELOPE_BYTES, QueryLimits, REMOTE_ENVELOPE_CODEC_LIMITS, answer_limit_field,
};
use crate::query_plan::{QueryPlan, decode_query_plan_with_limits};
use crate::value::CanonicalValue;

/// The authenticated outer reply format used for both successes and failures.
pub const QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1: &str = "typebridge.query-remote-signed-reply/v1";
/// Domain separating Ed25519 reply signatures from every other signed value.
pub const QUERY_REMOTE_REPLY_SIGNATURE_DOMAIN: &str = "typebridge.query.remote-reply-signature/v1";
/// Domain separating deterministic reply-signing key identifiers.
pub const QUERY_REMOTE_REPLY_KEY_ID_DOMAIN: &str = "typebridge.query.remote-reply-key-id/v1";

const EXECUTOR_COMPONENT_MIN_BYTES: usize = 16;
const EXECUTOR_COMPONENT_MAX_BYTES: usize = 128;
/// Default lifetime for a remote request with no explicit caller deadline.
///
/// This matches the standalone executor's mandatory execution timeout and
/// keeps one omitted field from occupying replay capacity for hours.
pub const DEFAULT_REMOTE_DEADLINE_MS: u64 = 30 * 1_000;
/// Longest caller deadline admitted by the first remote format: five minutes.
pub const MAX_REMOTE_DEADLINE_MS: u64 = 5 * 60 * 1_000;
/// Maximum positive client/server wall-clock skew admitted by remote preflight.
///
/// The request lifetime is still bounded to [`MAX_REMOTE_DEADLINE_MS`]
/// between its fingerprint-bound preparation and expiry timestamps. This allowance only
/// prevents a client clock up to one minute ahead from being rejected as a
/// forged future request.
pub const MAX_REMOTE_CLOCK_SKEW_MS: u64 = 60 * 1_000;

/// One typed value of a remote result row.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteValue {
    /// An entity or relation reference.
    Thing {
        /// The provider instance identity.
        iid: String,
        /// The validated runtime type.
        type_id: TypeId,
    },
    /// An attribute instance with its parsed canonical value.
    Attribute {
        /// The validated runtime attribute type.
        type_id: TypeId,
        /// The exact typed scalar value.
        value: CanonicalValue,
    },
    /// A pure typed value.
    Value {
        /// The exact typed scalar value.
        value: CanonicalValue,
    },
    /// An explicit absence in an optional column.
    Absent,
}

/// One typed field value of a remote fetched document.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteFieldValue {
    /// Nested named fields of a V3 thing document.
    Document {
        /// Schema-checked members.
        fields: BTreeMap<String, RemoteFieldValue>,
    },
    /// One exact typed scalar.
    Scalar {
        /// The exact typed scalar value.
        value: CanonicalValue,
    },
    /// An explicit absence in an optional scalar field.
    Absent,
    /// A typed list of attribute values.
    List {
        /// The exact typed list elements.
        values: Vec<CanonicalValue>,
    },
}

/// Fingerprint domain for whole remote request envelopes.
pub const QUERY_REMOTE_REQUEST_FINGERPRINT_DOMAIN: &str = "typebridge.query.remote-request";
/// Exact Ed25519 public key trusted to authenticate replies from one executor.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct RemoteSigningPublicKey([u8; 32]);

impl RemoteSigningPublicKey {
    /// Construct a public key from its exact Ed25519 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Borrow the exact Ed25519 public-key bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode the key as fixed-width lowercase hexadecimal wire text.
    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn from_hex(value: &str) -> Result<Self, ()> {
        decode_fixed_hex(value).map(Self)
    }
}

impl std::fmt::Debug for RemoteSigningPublicKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RemoteSigningPublicKey")
            .field(&self.to_hex())
            .finish()
    }
}

impl Serialize for RemoteSigningPublicKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RemoteSigningPublicKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(|()| serde::de::Error::custom("invalid remote signing key"))
    }
}

/// Deterministic identity of one exact remote reply-signing public key.
///
/// The identifier is domain-separated from every other digest and is carried
/// alongside the key in capability advertisements and signed outer replies.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct RemoteSigningKeyId([u8; 32]);

impl RemoteSigningKeyId {
    /// Derive the identifier for one exact Ed25519 public key.
    #[must_use]
    pub fn for_public_key(key: RemoteSigningPublicKey) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(QUERY_REMOTE_REPLY_KEY_ID_DOMAIN.as_bytes());
        hasher.update([0]);
        hasher.update(key.as_bytes());
        Self(hasher.finalize().into())
    }

    /// Borrow the exact key-identifier bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Encode the identifier as fixed-width lowercase hexadecimal wire text.
    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn from_hex(value: &str) -> Result<Self, ()> {
        decode_fixed_hex(value).map(Self)
    }
}

impl std::fmt::Debug for RemoteSigningKeyId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("RemoteSigningKeyId")
            .field(&self.to_hex())
            .finish()
    }
}

impl Serialize for RemoteSigningKeyId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for RemoteSigningKeyId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value)
            .map_err(|()| serde::de::Error::custom("invalid remote signing key identifier"))
    }
}

/// Exact Ed25519 signature carried by the authenticated outer reply.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct RemoteReplySignature([u8; 64]);

impl RemoteReplySignature {
    /// Construct a signature from its exact Ed25519 bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Borrow the exact Ed25519 signature bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    fn to_hex(self) -> String {
        encode_hex(&self.0)
    }

    fn from_hex(value: &str) -> Result<Self, ()> {
        decode_fixed_hex(value).map(Self)
    }
}

impl std::fmt::Debug for RemoteReplySignature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RemoteReplySignature(..)")
    }
}

/// Domain-separated digest signed for one canonical unsigned outer reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RemoteReplySigningDigest([u8; 32]);

impl RemoteReplySigningDigest {
    /// Borrow the digest bytes passed to the Ed25519 implementation.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Binding-neutral signing operation used by the contract wire encoder.
pub trait RemoteReplySigner {
    /// Return the public key paired with this signer.
    fn public_key(&self) -> RemoteSigningPublicKey;

    /// Sign one domain-separated canonical reply digest.
    fn sign(&self, digest: &RemoteReplySigningDigest) -> RemoteReplySignature;
}

/// Binding-neutral signature verifier used before any reply payload is decoded.
pub trait RemoteReplyVerifier {
    /// Verify one digest against the exact trusted public key.
    fn verify(
        &self,
        key: RemoteSigningPublicKey,
        digest: &RemoteReplySigningDigest,
        signature: &RemoteReplySignature,
    ) -> bool;
}

/// The exact wire discriminator for first-format capability advertisements.
pub const QUERY_REMOTE_CAPABILITIES_FORMAT_V1: &str = "typebridge.query-remote-capabilities/v1";
/// Fingerprint domain for exact executor capability advertisements.
pub const QUERY_REMOTE_CAPABILITIES_FINGERPRINT_DOMAIN: &str =
    "typebridge.query.remote-capabilities";
/// Canonicalization identifier for capability-advertisement fingerprints.
pub const QUERY_REMOTE_CAPABILITIES_CANONICALIZATION: &str =
    "typebridge.query-remote-capabilities/v1";

/// One logical executor identity and one concrete process/shared-store epoch.
///
/// Standalone executors generate a fresh pair at startup. A multi-instance
/// deployment may share a pair only together with a globally atomic replay
/// store; otherwise advertisements differ and cross-instance requests fail
/// closed at preflight.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteExecutorBinding {
    epoch: String,
    identity: String,
}

impl RemoteExecutorBinding {
    /// Construct one validated executor identity/epoch pair.
    pub fn new(identity: impl Into<String>, epoch: impl Into<String>) -> Result<Self, Diagnostic> {
        let binding = Self {
            epoch: epoch.into(),
            identity: identity.into(),
        };
        validate_executor_component(&binding.identity)?;
        validate_executor_component(&binding.epoch)?;
        Ok(binding)
    }

    /// Return the logical executor identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Return the concrete process/shared-store epoch.
    #[must_use]
    pub fn epoch(&self) -> &str {
        &self.epoch
    }
}

/// The canonical fingerprint of one exact capability advertisement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCapabilitiesFingerprint(Fingerprint);

impl RemoteCapabilitiesFingerprint {
    /// Return the generic fingerprint.
    #[must_use]
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    /// Return the fixed-width lowercase advertisement digest.
    #[must_use]
    pub fn digest_hex(&self) -> String {
        self.0.digest().to_hex()
    }
}

/// One executor capability advertisement for pre-flight negotiation.
///
/// A client checks its plan's required capabilities against this set and
/// refuses to send unsupported plans; the executor re-checks on receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteCapabilities {
    capabilities: crate::capability::CapabilitySet,
    // Pre-release wire ledger: /v1 gained an executor identity and epoch
    // before 2.0.0 shipped. Requests fingerprint these exact bytes so a
    // restart or a different standalone instance cannot accept captured
    // requests prepared for an earlier executor incarnation.
    executor: RemoteExecutorBinding,
    format: String,
    // The executor's query limits, by stable name, holding only those that
    // differ from the canonical defaults. An executor with default limits
    // omits the member, so its advertisement bytes and fingerprint are the
    // released ones. The executor enforces exactly this statement.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    query_limits: BTreeMap<String, u64>,
    // The executor's answer ceilings, by stable name, holding only those
    // that differ from the defaults, under the same rule as `query_limits`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    answer_limits: BTreeMap<String, u64>,
    // This key is an explicit caller trust input. Its inclusion in the exact
    // advertisement fingerprint binds every prepared request to one signer.
    reply_key: RemoteSigningPublicKey,
    // The domain-separated identity is redundant by design: decoding verifies
    // it against `reply_key`, while signed replies bind both exact values.
    reply_key_id: RemoteSigningKeyId,
}

impl RemoteCapabilities {
    /// Advertise one executor capability set.
    #[must_use]
    pub fn new(
        capabilities: crate::capability::CapabilitySet,
        executor: RemoteExecutorBinding,
        reply_key: RemoteSigningPublicKey,
    ) -> Self {
        let reply_key_id = RemoteSigningKeyId::for_public_key(reply_key);
        Self {
            capabilities,
            executor,
            format: QUERY_REMOTE_CAPABILITIES_FORMAT_V1.to_owned(),
            query_limits: BTreeMap::new(),
            answer_limits: BTreeMap::new(),
            reply_key,
            reply_key_id,
        }
    }

    /// Advertise the answer ceilings this executor enforces.
    ///
    /// `ceilings` names the dimensions that differ from their defaults; a
    /// default-valued, unknown, or unsupported entry is refused.
    pub fn with_answer_limits(
        mut self,
        ceilings: BTreeMap<String, u64>,
    ) -> Result<Self, Diagnostic> {
        self.answer_limits = ceilings;
        self.validate_answer_limits()?;
        Ok(self)
    }

    /// Return the answer ceilings this executor advertises and enforces, by
    /// stable name. A dimension the advertisement does not name has its
    /// default.
    #[must_use]
    pub const fn answer_limits(&self) -> &BTreeMap<String, u64> {
        &self.answer_limits
    }

    /// Require the canonical form of the advertised answer ceilings: known
    /// names, no default-valued entry, and no value past its maximum.
    fn validate_answer_limits(&self) -> Result<(), Diagnostic> {
        for (name, value) in &self.answer_limits {
            let field = answer_limit_field(name).ok_or_else(advertised_limits_invalid)?;
            if *value == field.proxy_default || *value > field.maximum {
                return Err(advertised_limits_invalid());
            }
        }
        Ok(())
    }

    /// Advertise the query limits this executor enforces.
    ///
    /// `limits` is admitted through [`QueryLimits::validate`]. Only limits
    /// that differ from the canonical defaults reach the wire.
    pub fn with_query_limits(mut self, limits: QueryLimits) -> Result<Self, Diagnostic> {
        let limits = limits.validate().map_err(|_| advertised_limits_invalid())?;
        self.query_limits = QueryLimits::FIELDS
            .iter()
            .filter_map(|name| {
                let value = limits.get(name)?;
                (Some(value) != QueryLimits::CANONICAL.get(name))
                    .then(|| ((*name).to_owned(), u64::try_from(value).unwrap_or(u64::MAX)))
            })
            .collect();
        Ok(self)
    }

    /// Return the query limits this executor advertises and enforces.
    ///
    /// A limit the advertisement does not name has its canonical default.
    #[must_use]
    pub fn query_limits(&self) -> QueryLimits {
        let mut limits = QueryLimits::CANONICAL;
        for (name, value) in &self.query_limits {
            limits.set(name, usize::try_from(*value).unwrap_or(usize::MAX));
        }
        limits
    }

    /// Require the canonical form of the advertised limits: known names,
    /// no default-valued entry, and a value set validation admits.
    fn validate_query_limits(&self) -> Result<(), Diagnostic> {
        for (name, value) in &self.query_limits {
            let canonical = QueryLimits::CANONICAL
                .get(name)
                .ok_or_else(advertised_limits_invalid)?;
            if usize::try_from(*value).ok() == Some(canonical) || usize::try_from(*value).is_err() {
                return Err(advertised_limits_invalid());
            }
        }
        self.query_limits()
            .validate()
            .map(|_| ())
            .map_err(|_| advertised_limits_invalid())
    }

    /// Encode exact canonical envelope bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    /// Decode one advertisement, rejecting unknown fields and formats.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let advertisement =
            from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        if advertisement.format != QUERY_REMOTE_CAPABILITIES_FORMAT_V1 {
            return Err(envelope_failure(
                DiagnosticCategory::InvalidContract,
                "query_remote_format_unsupported",
                "remote capability wire format is unsupported",
            ));
        }
        validate_executor_component(advertisement.executor.identity())?;
        validate_executor_component(advertisement.executor.epoch())?;
        advertisement.validate_query_limits()?;
        advertisement.validate_answer_limits()?;
        if advertisement.reply_key_id != RemoteSigningKeyId::for_public_key(advertisement.reply_key)
        {
            return Err(remote_signature_invalid());
        }
        if advertisement.encode()? != bytes {
            return Err(envelope_failure(
                DiagnosticCategory::Integrity,
                "query_remote_capabilities_wire_mismatch",
                "remote capability bytes normalize after trusted reconstruction",
            ));
        }
        Ok(advertisement)
    }

    /// Return the advertised capability set.
    #[must_use]
    pub const fn capabilities(&self) -> &crate::capability::CapabilitySet {
        &self.capabilities
    }

    /// Return this advertisement's executor identity and epoch.
    #[must_use]
    pub const fn executor(&self) -> &RemoteExecutorBinding {
        &self.executor
    }

    /// Return the exact public key trusted to authenticate executor replies.
    #[must_use]
    pub const fn reply_key(&self) -> RemoteSigningPublicKey {
        self.reply_key
    }

    /// Return the deterministic identity of the advertised reply key.
    #[must_use]
    pub const fn reply_key_id(&self) -> RemoteSigningKeyId {
        self.reply_key_id
    }

    /// Fingerprint the exact canonical advertisement, including its epoch.
    pub fn fingerprint(&self) -> Result<RemoteCapabilitiesFingerprint, Diagnostic> {
        Ok(RemoteCapabilitiesFingerprint(Fingerprint::compute(
            FingerprintDomain::new(QUERY_REMOTE_CAPABILITIES_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(QUERY_REMOTE_CAPABILITIES_CANONICALIZATION)?,
            None,
            &self.encode()?,
        )))
    }
}

/// Attribute a request-envelope codec failure: its byte ceiling is the
/// remote request budget, not the plan artifact ceiling the code table names.
pub(crate) fn annotate_request_envelope(limits: QueryLimits, diagnostic: Diagnostic) -> Diagnostic {
    if diagnostic.code().as_str() == "canonical_json_too_large" {
        limits.annotate_limit(diagnostic, "remote_request_bytes")
    } else {
        limits.annotate(diagnostic)
    }
}

/// Hold one plan to an executor's advertised limits before anything is sent.
///
/// Returns the merged limits the request must also be encoded under. The
/// plan is re-decoded only where the advertisement is stricter than the
/// limits it was built under, which is the same check the executor runs.
pub(crate) fn negotiate_query_limits(
    plan: &QueryPlan,
    advertisement: &RemoteCapabilities,
) -> Result<QueryLimits, Diagnostic> {
    let configured = plan.limits();
    let negotiated = configured.min(advertisement.query_limits());
    if negotiated != configured {
        decode_query_plan_with_limits(&plan.canonical_bytes()?, negotiated)
            .map_err(|diagnostic| negotiated.mark_advertised(configured, diagnostic))?;
    }
    Ok(negotiated)
}

fn advertised_limits_invalid() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_capabilities_limits_invalid",
        "remote capability limits are unknown, redundant, or unsupported",
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SignedRemoteReplyPeek<'wire> {
    #[serde(borrow)]
    advertisement: &'wire str,
    #[serde(borrow)]
    format: &'wire str,
    #[serde(borrow)]
    key: &'wire str,
    #[serde(borrow)]
    key_id: &'wire str,
    #[serde(borrow)]
    payload: &'wire serde_json::value::RawValue,
    #[serde(borrow)]
    signature: &'wire str,
}

/// Shared authenticated-outer-envelope byte preflight for additive payload
/// versions. This does not inspect or reconstruct a version-specific payload.
///
/// The byte ceiling is the caller's own. A caller that asked for less than
/// the default envelope is told its budget was exceeded; one at or above it
/// is told the envelope was.
pub(crate) fn preflight_signed_reply_size(
    bytes: &[u8],
    caller_max_bytes: u64,
) -> Result<(), Diagnostic> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= caller_max_bytes {
        return Ok(());
    }
    if caller_max_bytes < u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX) {
        return Err(remote_response_oversized());
    }
    Err(remote_envelope_too_large())
}

fn verify_signed_reply<'wire>(
    bytes: &'wire [u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    verifier: &impl RemoteReplyVerifier,
) -> Result<&'wire [u8], Diagnostic> {
    let outer = serde_json::from_slice::<SignedRemoteReplyPeek<'wire>>(bytes).map_err(|_| {
        envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_reply_malformed",
            "remote reply is not a signed JSON envelope",
        )
    })?;
    let expected_advertisement = expected_advertisement.digest_hex();
    let expected_key = trusted_key.to_hex();
    let expected_key_id = RemoteSigningKeyId::for_public_key(trusted_key).to_hex();
    let signature =
        RemoteReplySignature::from_hex(outer.signature).map_err(|()| remote_signature_invalid())?;
    if outer.advertisement != expected_advertisement
        || outer.key != expected_key
        || outer.key_id != expected_key_id
        || outer.signature != signature.to_hex()
    {
        return Err(remote_signature_invalid());
    }
    let digest = remote_reply_signing_digest(
        outer.advertisement,
        outer.format,
        outer.key,
        outer.key_id,
        outer.payload.get().as_bytes(),
    );
    if !verifier.verify(trusted_key, &digest, &signature) {
        return Err(remote_signature_invalid());
    }
    if outer.format != QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1 {
        return Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_format_unsupported",
            "signed remote reply wire format is unsupported",
        ));
    }
    let canonical = canonical_signed_reply(
        outer.advertisement,
        outer.format,
        outer.key,
        outer.key_id,
        outer.payload.get().as_bytes(),
        outer.signature,
    );
    if canonical != bytes {
        return Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "non_canonical_json",
            "input is valid JSON but not the canonical encoding",
        ));
    }
    Ok(outer.payload.get().as_bytes())
}

/// Authenticate the unchanged signed outer envelope for an additive payload
/// version. Payload dispatch remains the caller's version-specific boundary.
pub(crate) fn verify_signed_reply_payload<'wire>(
    bytes: &'wire [u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    verifier: &impl RemoteReplyVerifier,
) -> Result<&'wire [u8], Diagnostic> {
    verify_signed_reply(bytes, expected_advertisement, trusted_key, verifier)
}

fn encode_signed_reply(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Result<Vec<u8>, Diagnostic> {
    encode_signed_reply_bounded(payload, advertisement, signer, MAX_REMOTE_ENVELOPE_BYTES)
}

fn encode_signed_reply_bounded(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
    max_bytes: usize,
) -> Result<Vec<u8>, Diagnostic> {
    let encoded = encode_signed_reply_unchecked(payload, advertisement, signer);
    if encoded.len() > max_bytes {
        return Err(envelope_failure(
            DiagnosticCategory::ResourceLimit,
            "query_remote_envelope_too_large",
            "remote reply exceeds the envelope byte ceiling",
        ));
    }
    Ok(encoded)
}

/// Encode additive versioned payload bytes in the unchanged signed outer
/// envelope.
pub(crate) fn encode_signed_reply_payload(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Result<Vec<u8>, Diagnostic> {
    encode_signed_reply(payload, advertisement, signer)
}

/// Encode additive versioned payload bytes under the envelope byte ceiling
/// the two peers agreed on.
pub(crate) fn encode_signed_reply_payload_bounded(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
    max_bytes: usize,
) -> Result<Vec<u8>, Diagnostic> {
    encode_signed_reply_bounded(payload, advertisement, signer, max_bytes)
}

/// Encode already bounded, canonical additive payload bytes.
///
/// This is used only by the fixed internal-failure fallback after the normal
/// checked encoder has failed. Version-specific modules must construct the
/// payload from static text plus previously validated ASCII bindings.
pub(crate) fn encode_signed_reply_payload_unchecked(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    encode_signed_reply_unchecked(payload, advertisement, signer)
}

fn encode_signed_reply_unchecked(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    let advertisement = advertisement.digest_hex();
    let key = signer.public_key().to_hex();
    let key_id = RemoteSigningKeyId::for_public_key(signer.public_key()).to_hex();
    let digest = remote_reply_signing_digest(
        &advertisement,
        QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1,
        &key,
        &key_id,
        payload,
    );
    let signature = signer.sign(&digest).to_hex();
    canonical_signed_reply(
        &advertisement,
        QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1,
        &key,
        &key_id,
        payload,
        &signature,
    )
}

fn signed_reply_encoded_len(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    key: RemoteSigningPublicKey,
) -> usize {
    canonical_signed_reply(
        &advertisement.digest_hex(),
        QUERY_REMOTE_SIGNED_REPLY_FORMAT_V1,
        &key.to_hex(),
        &RemoteSigningKeyId::for_public_key(key).to_hex(),
        payload,
        &encode_hex(&[0_u8; 64]),
    )
    .len()
}

/// Return the exact unchanged signed-envelope length for additive payload
/// bytes without constructing a signature.
pub(crate) fn signed_reply_payload_encoded_len(
    payload: &[u8],
    advertisement: &RemoteCapabilitiesFingerprint,
    key: RemoteSigningPublicKey,
) -> usize {
    signed_reply_encoded_len(payload, advertisement, key)
}

fn remote_reply_signing_digest(
    advertisement: &str,
    format: &str,
    key: &str,
    key_id: &str,
    payload: &[u8],
) -> RemoteReplySigningDigest {
    let prefix = canonical_signed_reply_prefix(advertisement, format, key, key_id);
    let mut hasher = Sha256::new();
    hasher.update(QUERY_REMOTE_REPLY_SIGNATURE_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(&prefix);
    hasher.update(payload);
    hasher.update(b"}");
    RemoteReplySigningDigest(hasher.finalize().into())
}

fn canonical_signed_reply_prefix(
    advertisement: &str,
    format: &str,
    key: &str,
    key_id: &str,
) -> Vec<u8> {
    format!(
        "{{\"advertisement\":\"{advertisement}\",\"format\":\"{format}\",\"key\":\"{key}\",\"key_id\":\"{key_id}\",\"payload\":"
    )
    .into_bytes()
}

fn canonical_signed_reply(
    advertisement: &str,
    format: &str,
    key: &str,
    key_id: &str,
    payload: &[u8],
    signature: &str,
) -> Vec<u8> {
    let prefix = canonical_signed_reply_prefix(advertisement, format, key, key_id);
    let suffix = format!(",\"signature\":\"{signature}\"}}");
    let mut encoded = Vec::with_capacity(prefix.len() + payload.len() + suffix.len());
    encoded.extend_from_slice(&prefix);
    encoded.extend_from_slice(payload);
    encoded.extend_from_slice(suffix.as_bytes());
    encoded
}

/// Stable rejection for an unauthenticated or foreign remote reply.
#[must_use]
pub fn remote_signature_invalid() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::Integrity,
        "query_remote_signature_invalid",
        "remote reply signature or trusted executor binding is invalid",
    )
}

fn remote_response_oversized() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_response_oversized",
        "response envelope exceeds the caller byte budget",
    )
}

fn remote_envelope_too_large() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_envelope_too_large",
        "remote reply exceeds the envelope byte ceiling",
    )
}

/// Convert one caller-supplied limit into the unsigned wire range.
///
/// Every binding funnels its limit arguments through this exact
/// conversion, so a negative or out-of-range budget fails with one
/// stable diagnostic in every language instead of silently saturating
/// to zero or dropping the budget entirely.
pub fn checked_remote_limit(value: i128) -> Result<u64, Diagnostic> {
    u64::try_from(value).map_err(|_| remote_limit_invalid())
}

/// The stable rejection every out-of-range limit argument maps to.
///
/// Exposed so bindings whose integer representation exceeds `i128`
/// (JavaScript `BigInt`) reject unrepresentable values with exactly
/// this diagnostic instead of inventing their own.
#[must_use]
pub fn remote_limit_invalid() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::InvalidContract,
        "query_remote_limit_invalid",
        "remote limits are unsigned 64-bit integers",
    )
}

/// Convert one optional caller-supplied deadline into the wire range.
///
/// A negative deadline is rejected — never silently mapped to "no
/// deadline", which would remove the bound instead of enforcing it.
pub fn checked_remote_deadline(value: Option<i128>) -> Result<Option<u64>, Diagnostic> {
    value
        .map(|value| {
            let value = checked_remote_limit(value)?;
            if value > MAX_REMOTE_DEADLINE_MS {
                return Err(remote_deadline_limit());
            }
            Ok(value)
        })
        .transpose()
}

/// Stable rejection for a deadline outside the supported monotonic range.
#[must_use]
pub fn remote_deadline_limit() -> Diagnostic {
    envelope_failure(
        DiagnosticCategory::ResourceLimit,
        "query_remote_deadline_limit",
        "remote deadline exceeds the maximum supported duration",
    )
}

fn validate_executor_component(value: &str) -> Result<(), Diagnostic> {
    let valid = (EXECUTOR_COMPONENT_MIN_BYTES..=EXECUTOR_COMPONENT_MAX_BYTES)
        .contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(envelope_failure(
            DiagnosticCategory::InvalidContract,
            "query_remote_executor_invalid",
            "executor identity and epoch are 16-128 safe ASCII bytes",
        ))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_fixed_hex<const N: usize>(value: &str) -> Result<[u8; N], ()> {
    if value.len() != N * 2 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let mut decoded = [0_u8; N];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Ok(decoded)
}

fn hex_nibble(byte: u8) -> Result<u8, ()> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(()),
    }
}

fn envelope_failure(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static remote envelope code"),
        message,
    )
}
