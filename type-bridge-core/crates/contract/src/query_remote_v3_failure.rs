//! Authenticated refusals before a staged request can be admitted.

use crate::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::limits::{
    MAX_DIAGNOSTIC_BYTES, MAX_REMOTE_ENVELOPE_BYTES, REMOTE_ENVELOPE_CODEC_LIMITS,
};
use crate::query_remote::{
    RemoteCapabilitiesFingerprint, RemoteReplySigner, RemoteReplyVerifier, RemoteSigningPublicKey,
};
use crate::query_remote_diagnostic::{
    RemoteDiagnostic, RemoteDiagnosticDetail, RemoteDiagnosticPathSegment,
    validate_remote_diagnostic_wire,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Exact discriminator for a pre-request staged failure.
pub const QUERY_REMOTE_FAILURE_FORMAT_V3: &str = "typebridge.query-remote-failure/v3";
const FAILURE_FRAMING_ALLOWANCE_BYTES: usize = 1024;

pub(super) fn validate_nonce(nonce: &str) -> Result<(), Diagnostic> {
    if !(16..=128).contains(&nonce.len())
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(failure_diagnostic(
            "query_remote_v3_nonce",
            "staged nonce is outside its canonical grammar",
        ));
    }
    Ok(())
}

fn diagnostic_failure(category: DiagnosticCategory, code: &str, message: &str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("fixed diagnostic code"),
        message,
    )
}

fn failure_diagnostic(code: &str, message: &str) -> Diagnostic {
    diagnostic_failure(DiagnosticCategory::InvalidContract, code, message)
}

fn encode_minimal_signed_failure(
    original: &StagedFailureV3,
    advertisement: &RemoteCapabilitiesFingerprint,
    signer: &impl RemoteReplySigner,
) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"category\":\"integrity\",\"code\":\"query_remote_v3_internal_failure\",\"details\":{},\"format\":\"typebridge.query-remote-failure/v3\",\"message\":\"executor could not encode the original V3 remote failure\",\"nonce\":";
    let nonce = original
        .nonce
        .as_deref()
        .filter(|nonce| validate_nonce(nonce).is_ok());
    let mut payload = Vec::with_capacity(PREFIX.len() + 256);
    payload.extend_from_slice(PREFIX);
    append_optional_safe_ascii(&mut payload, nonce);
    payload.extend_from_slice(b",\"path\":[],\"request\":");
    payload.extend_from_slice(b"null");
    payload.push(b'}');
    crate::query_remote::encode_signed_reply_payload_unchecked(&payload, advertisement, signer)
}

fn append_optional_safe_ascii(encoded: &mut Vec<u8>, value: Option<&str>) {
    if let Some(value) = value {
        encoded.push(b'"');
        encoded.extend_from_slice(value.as_bytes());
        encoded.push(b'"');
    } else {
        encoded.extend_from_slice(b"null");
    }
}

/// An authenticated structured refusal before a staged request is admitted.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StagedFailureV3 {
    category: DiagnosticCategory,
    code: DiagnosticCode,
    details: BTreeMap<String, RemoteDiagnosticDetail>,
    format: String,
    message: String,
    nonce: Option<String>,
    path: Vec<RemoteDiagnosticPathSegment>,
    request: Option<String>,
}

impl StagedFailureV3 {
    /// Construct an uncorrelated V3 failure for a pre-request transport error.
    pub fn new(nonce: Option<String>, diagnostic: &Diagnostic) -> Result<Self, Diagnostic> {
        let diagnostic = RemoteDiagnostic::new(diagnostic)?;
        if let Some(nonce) = nonce.as_deref() {
            validate_nonce(nonce)?;
        }
        Ok(Self {
            category: diagnostic.category,
            code: diagnostic.code,
            details: diagnostic.details,
            format: QUERY_REMOTE_FAILURE_FORMAT_V3.to_owned(),
            message: diagnostic.message,
            nonce,
            path: diagnostic.path,
            request: None,
        })
    }

    /// Encode one authenticated V3 failure in the unchanged signed outer
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
    /// with a fixed bounded V3 internal failure while retaining a safe nonce.
    #[must_use]
    pub fn encode_signed_or_fallback(
        &self,
        advertisement: &RemoteCapabilitiesFingerprint,
        signer: &impl RemoteReplySigner,
    ) -> Vec<u8> {
        match self.encode_signed(advertisement, signer) {
            Ok(encoded) => encoded,
            Err(_) => encode_minimal_signed_failure(self, advertisement, signer),
        }
    }

    fn encode_payload(&self) -> Result<Vec<u8>, Diagnostic> {
        if self.format != QUERY_REMOTE_FAILURE_FORMAT_V3 || self.request.is_some() {
            return Err(failure_diagnostic(
                "query_remote_v3_failure_binding",
                "invalid pre-request failure format or binding",
            ));
        }
        if let Some(nonce) = self.nonce.as_deref() {
            validate_nonce(nonce)?;
        }
        self.remote_diagnostic()?.validate_size()?;
        to_canonical_json_with_limits(self, REMOTE_ENVELOPE_CODEC_LIMITS)
    }

    /// Decode an already authenticated V3 failure payload.
    pub fn decode_payload(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let maximum = MAX_DIAGNOSTIC_BYTES.saturating_add(FAILURE_FRAMING_ALLOWANCE_BYTES);
        if bytes.len() > maximum {
            return Err(diagnostic_failure(
                DiagnosticCategory::ResourceLimit,
                "query_remote_v3_diagnostic_too_large",
                "V3 remote diagnostic exceeds the canonical byte ceiling",
            ));
        }
        let failure = from_canonical_json_with_limits::<Self>(bytes, REMOTE_ENVELOPE_CODEC_LIMITS)?;
        if failure.format != QUERY_REMOTE_FAILURE_FORMAT_V3 {
            return Err(failure_diagnostic(
                "query_remote_v3_format",
                "unsupported staged failure format",
            ));
        }
        if failure.request.is_some() {
            return Err(failure_diagnostic(
                "query_remote_v3_failure_unexpected_binding",
                "a pre-request failure must not carry a request fingerprint",
            ));
        }
        if let Some(nonce) = failure.nonce.as_deref() {
            validate_nonce(nonce)?;
        }
        failure.remote_diagnostic()?.validate_size()?;
        if failure.encode_payload()? != bytes {
            return Err(diagnostic_failure(
                DiagnosticCategory::Integrity,
                "query_remote_v3_failure_wire_mismatch",
                "V3 failure bytes normalize after trusted reconstruction",
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

    fn remote_diagnostic(&self) -> Result<RemoteDiagnostic, Diagnostic> {
        let diagnostic = RemoteDiagnostic {
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

/// Authenticate and decode an uncorrelated V3 pre-request failure.
pub fn decode_signed_staged_failure(
    bytes: &[u8],
    expected_advertisement: &RemoteCapabilitiesFingerprint,
    trusted_key: RemoteSigningPublicKey,
    max_bytes: u64,
    verifier: &impl RemoteReplyVerifier,
) -> Result<StagedFailureV3, Diagnostic> {
    // A failure carries no result data, so it never needs more than the
    // default envelope whatever byte budget the caller raised.
    crate::query_remote::preflight_signed_reply_size(
        bytes,
        max_bytes.min(u64::try_from(MAX_REMOTE_ENVELOPE_BYTES).unwrap_or(u64::MAX)),
    )?;
    let payload = crate::query_remote::verify_signed_reply_payload(
        bytes,
        expected_advertisement,
        trusted_key,
        verifier,
    )?;
    let failure = StagedFailureV3::decode_payload(payload)?;
    Ok(failure)
}
