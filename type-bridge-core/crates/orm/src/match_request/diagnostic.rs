//! Deterministic, read-only diagnostics for unvalidated match requests.
//!
//! Diagnostic bytes are useful for debugging, revalidation, and golden tests.
//! They never contain a live request token and never deserialize into a
//! validated request. The decoder rejects unknown versions, mismatched derived
//! capabilities, non-canonical JSON bytes, and oversized input before callers
//! can hand the contained request to canonical validation.

use serde::{Deserialize, Serialize};

use super::capability::CapabilitySet;
use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::limits::MAX_DIAGNOSTIC_BYTES;
use super::model::{MATCH_REQUEST_VERSION_V1, MatchRequest, MatchRequestVersion};

/// Wire-format version for the complete match-request diagnostic envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MatchDiagnosticVersion(u16);

impl MatchDiagnosticVersion {
    /// Initial diagnostic envelope version.
    pub const V1: Self = Self(1);

    /// Preserve an unvalidated version read from diagnostic bytes.
    pub const fn from_raw(value: u16) -> Self {
        Self(value)
    }

    /// Return the raw diagnostic version.
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Version constant used by V1 diagnostic producers.
pub const MATCH_DIAGNOSTIC_VERSION_V1: MatchDiagnosticVersion = MatchDiagnosticVersion::V1;

/// A decoded diagnostic that still requires canonical request validation.
///
/// This type deliberately does not implement `Deserialize`. Call
/// [`Self::from_canonical_bytes`] so version, size, canonical encoding, and
/// derived-capability checks cannot be skipped. It contains no schema proof,
/// result-shape proof, stable-order proof, or live request token.
#[derive(Debug, Clone, PartialEq)]
pub struct UnvalidatedMatchRequest {
    diagnostic_version: MatchDiagnosticVersion,
    request: MatchRequest,
    required_capabilities: CapabilitySet,
}

#[derive(Deserialize)]
struct DiagnosticWire {
    diagnostic_version: MatchDiagnosticVersion,
    request: MatchRequest,
    required_capabilities: CapabilitySet,
}

#[derive(Serialize)]
struct DiagnosticWireRef<'a> {
    diagnostic_version: MatchDiagnosticVersion,
    request: &'a MatchRequest,
    required_capabilities: &'a CapabilitySet,
}

impl UnvalidatedMatchRequest {
    /// Construct a V1 diagnostic from one unvalidated V1 request.
    pub fn from_request(request: MatchRequest) -> Result<Self, MatchError> {
        ensure_request_version(request.version)?;
        let required_capabilities = CapabilitySet::for_request(&request);
        let diagnostic = Self {
            diagnostic_version: MATCH_DIAGNOSTIC_VERSION_V1,
            request,
            required_capabilities,
        };
        // Enforce the output ceiling as well as the decoder's input ceiling.
        diagnostic.encode_canonical()?;
        Ok(diagnostic)
    }

    /// Decode exact canonical V1 diagnostic JSON into unvalidated request state.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, MatchError> {
        ensure_size(bytes.len())?;
        let wire: DiagnosticWire = serde_json::from_slice(bytes).map_err(|_| {
            MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "malformed_diagnostic",
                "match-request diagnostic is not valid diagnostic JSON",
            )
            .at(MatchErrorPathSegment::Request)
            .with_detail("actual_bytes", count_detail(bytes.len()))
        })?;

        ensure_diagnostic_version(wire.diagnostic_version)?;
        ensure_request_version(wire.request.version)?;

        let derived = CapabilitySet::for_request(&wire.request);
        if wire.required_capabilities != derived {
            return Err(MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "capability_set_mismatch",
                "diagnostic capabilities do not match canonical request derivation",
            )
            .at(MatchErrorPathSegment::Request)
            .with_detail(
                "actual_count",
                count_detail(wire.required_capabilities.len()),
            )
            .with_detail("derived_count", count_detail(derived.len())));
        }

        let diagnostic = Self {
            diagnostic_version: wire.diagnostic_version,
            request: wire.request,
            required_capabilities: wire.required_capabilities,
        };
        let canonical = diagnostic.encode_canonical()?;
        if canonical != bytes {
            return Err(MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "non_canonical_diagnostic",
                "diagnostic bytes are valid JSON but not the canonical encoding",
            )
            .at(MatchErrorPathSegment::Request)
            .with_detail("actual_bytes", count_detail(bytes.len()))
            .with_detail("canonical_bytes", count_detail(canonical.len())));
        }
        Ok(diagnostic)
    }

    /// Serialize this wrapper to exact deterministic compact JSON bytes.
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, MatchError> {
        self.encode_canonical()
    }

    /// Return the diagnostic envelope version.
    pub const fn diagnostic_version(&self) -> MatchDiagnosticVersion {
        self.diagnostic_version
    }

    /// Return the contained request, which remains unvalidated.
    pub fn request(&self) -> &MatchRequest {
        &self.request
    }

    /// Return the deterministic capabilities derived from the request shape.
    pub fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Consume the diagnostic wrapper into its unvalidated request and derived capabilities.
    pub fn into_parts(self) -> (MatchRequest, CapabilitySet) {
        (self.request, self.required_capabilities)
    }

    fn encode_canonical(&self) -> Result<Vec<u8>, MatchError> {
        let wire = DiagnosticWireRef {
            diagnostic_version: self.diagnostic_version,
            request: &self.request,
            required_capabilities: &self.required_capabilities,
        };
        let bytes = serde_json::to_vec(&wire).map_err(|_| {
            MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "diagnostic_encode_failed",
                "match-request diagnostic could not be encoded",
            )
            .at(MatchErrorPathSegment::Request)
        })?;
        ensure_size(bytes.len())?;
        Ok(bytes)
    }
}

fn ensure_diagnostic_version(version: MatchDiagnosticVersion) -> Result<(), MatchError> {
    if version == MATCH_DIAGNOSTIC_VERSION_V1 {
        return Ok(());
    }
    Err(MatchError::new(
        MatchErrorCategory::InvalidPlan,
        "unsupported_diagnostic_version",
        "match-request diagnostic version is not supported",
    )
    .at(MatchErrorPathSegment::Request)
    .with_detail("actual", u64::from(version.get()))
    .with_detail("supported", u64::from(MATCH_DIAGNOSTIC_VERSION_V1.get())))
}

fn ensure_request_version(version: MatchRequestVersion) -> Result<(), MatchError> {
    if version == MATCH_REQUEST_VERSION_V1 {
        return Ok(());
    }
    Err(MatchError::new(
        MatchErrorCategory::InvalidPlan,
        "unsupported_request_version",
        "match request version is not supported by this diagnostic codec",
    )
    .at(MatchErrorPathSegment::Request)
    .with_detail("actual", u64::from(version.get()))
    .with_detail("supported", u64::from(MATCH_REQUEST_VERSION_V1.get())))
}

fn ensure_size(actual: usize) -> Result<(), MatchError> {
    if actual <= MAX_DIAGNOSTIC_BYTES {
        return Ok(());
    }
    Err(MatchError::new(
        MatchErrorCategory::ResourceLimit,
        "diagnostic_too_large",
        "match-request diagnostic exceeds the canonical byte ceiling",
    )
    .at(MatchErrorPathSegment::Request)
    .with_detail("actual_bytes", count_detail(actual))
    .with_detail("maximum_bytes", count_detail(MAX_DIAGNOSTIC_BYTES)))
}

fn count_detail(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::match_request::capability::Capability;
    use crate::match_request::ids::{BindingId, DescriptorId};
    use crate::match_request::model::{
        MatchBinding, MatchMode, MatchOperation, MatchPlan, ThingKind,
    };

    fn request() -> MatchRequest {
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: BindingId::new(0),
                    descriptor: DescriptorId::new("entity:person"),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::CountBy {
                root: BindingId::new(0),
            },
        )
    }

    fn code(error: &MatchError) -> &str {
        error.code().as_str()
    }

    #[test]
    fn canonical_envelope_is_explicitly_versioned_and_deterministic() {
        let first = UnvalidatedMatchRequest::from_request(request()).unwrap();
        let second = UnvalidatedMatchRequest::from_request(request()).unwrap();
        let expected = concat!(
            r#"{"diagnostic_version":1,"request":{"version":1,"plan":{"bindings":["#,
            r#"{"id":0,"descriptor":"entity:person","thing_kind":"entity","match_mode":"exact"}"#,
            r#"],"predicate":null,"allowed_cross_joins":[]},"operation":{"kind":"count_by","root":0}},"#,
            r#""required_capabilities":["RESOURCE_BOUNDED_STREAMING","EXACT_ENTITY_TARGET","DISTINCT_ROOT_COUNT"]}"#,
        );

        assert_eq!(first.to_canonical_bytes().unwrap(), expected.as_bytes());
        assert_eq!(
            first.to_canonical_bytes().unwrap(),
            second.to_canonical_bytes().unwrap()
        );
        assert!(
            first
                .required_capabilities()
                .contains(Capability::DistinctRootCount)
        );
    }

    #[test]
    fn canonical_bytes_decode_only_to_unvalidated_wrapper() {
        let original = UnvalidatedMatchRequest::from_request(request()).unwrap();
        let bytes = original.to_canonical_bytes().unwrap();
        let decoded = UnvalidatedMatchRequest::from_canonical_bytes(&bytes).unwrap();

        assert_eq!(decoded, original);
        assert_eq!(decoded.diagnostic_version(), MATCH_DIAGNOSTIC_VERSION_V1);
        assert_eq!(decoded.request().version, MATCH_REQUEST_VERSION_V1);
        assert_eq!(decoded.into_parts().0, request());
    }

    #[test]
    fn unknown_diagnostic_version_is_rejected_before_canonicality() {
        let bytes = UnvalidatedMatchRequest::from_request(request())
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        let changed = String::from_utf8(bytes).unwrap().replacen(
            r#""diagnostic_version":1"#,
            r#""diagnostic_version":9"#,
            1,
        );

        let error = UnvalidatedMatchRequest::from_canonical_bytes(changed.as_bytes()).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(code(&error), "unsupported_diagnostic_version");
    }

    #[test]
    fn unknown_request_version_is_rejected_before_canonicality() {
        let bytes = UnvalidatedMatchRequest::from_request(request())
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        let changed = String::from_utf8(bytes).unwrap().replacen(
            r#""request":{"version":1"#,
            r#""request":{"version":99"#,
            1,
        );

        let error = UnvalidatedMatchRequest::from_canonical_bytes(changed.as_bytes()).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(code(&error), "unsupported_request_version");
    }

    #[test]
    fn whitespace_and_other_noncanonical_bytes_are_rejected() {
        let mut bytes = UnvalidatedMatchRequest::from_request(request())
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        bytes.push(b'\n');

        let error = UnvalidatedMatchRequest::from_canonical_bytes(&bytes).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(code(&error), "non_canonical_diagnostic");
    }

    #[test]
    fn mismatched_derived_capabilities_are_rejected() {
        let bytes = UnvalidatedMatchRequest::from_request(request())
            .unwrap()
            .to_canonical_bytes()
            .unwrap();
        let changed = String::from_utf8(bytes).unwrap().replacen(
            "DISTINCT_ROOT_COUNT",
            "DISTINCT_ROOT_EXISTS",
            1,
        );

        let error = UnvalidatedMatchRequest::from_canonical_bytes(changed.as_bytes()).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(code(&error), "capability_set_mismatch");
    }

    #[test]
    fn malformed_and_oversized_input_have_stable_errors() {
        let malformed = UnvalidatedMatchRequest::from_canonical_bytes(b"{").unwrap_err();
        assert_eq!(malformed.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(code(&malformed), "malformed_diagnostic");

        let oversized = vec![b' '; MAX_DIAGNOSTIC_BYTES + 1];
        let error = UnvalidatedMatchRequest::from_canonical_bytes(&oversized).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(code(&error), "diagnostic_too_large");
    }
}
