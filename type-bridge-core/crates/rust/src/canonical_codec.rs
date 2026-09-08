//! Cancellation, deadline, and tighten-only controls for canonical records and archives.

use std::time::{Duration, Instant};

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::limits::{
    CodecLimits, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH, MAX_CANONICAL_STRING_BYTES,
};
use type_bridge_contract::projected_record::{
    MAX_PROJECTED_ARCHIVE_BYTES, MAX_PROJECTED_ARCHIVE_RECORDS, MAX_PROJECTED_DECODED_WEIGHT,
    MAX_PROJECTED_RECORD_BYTES,
};
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;

use crate::{AnswerCancellation, Error, ModelValidationPhase};

/// Tighten-only structural and byte ceilings for canonical codec work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalCodecLimits {
    max_input_bytes: usize,
    max_output_bytes: usize,
    max_depth: usize,
    max_records: usize,
    max_members: usize,
}

impl Default for CanonicalCodecLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: MAX_PROJECTED_ARCHIVE_BYTES,
            max_output_bytes: MAX_PROJECTED_ARCHIVE_BYTES,
            max_depth: MAX_CANONICAL_DEPTH,
            max_records: MAX_PROJECTED_ARCHIVE_RECORDS,
            max_members: MAX_PROJECTED_DECODED_WEIGHT.min(MAX_CANONICAL_COLLECTION_LEN),
        }
    }
}

impl CanonicalCodecLimits {
    /// Return the default canonical codec ceilings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tighten the canonical input byte ceiling.
    #[must_use]
    pub fn with_max_input_bytes(mut self, value: usize) -> Self {
        self.max_input_bytes = self.max_input_bytes.min(value);
        self
    }

    /// Tighten the canonical output byte ceiling.
    #[must_use]
    pub fn with_max_output_bytes(mut self, value: usize) -> Self {
        self.max_output_bytes = self.max_output_bytes.min(value);
        self
    }

    /// Tighten the canonical JSON nesting-depth ceiling.
    #[must_use]
    pub fn with_max_depth(mut self, value: usize) -> Self {
        self.max_depth = self.max_depth.min(value);
        self
    }

    /// Tighten the ordered archive record ceiling.
    #[must_use]
    pub fn with_max_records(mut self, value: usize) -> Self {
        self.max_records = self.max_records.min(value);
        self
    }

    /// Tighten the decoded member/value/reference weight ceiling.
    #[must_use]
    pub fn with_max_members(mut self, value: usize) -> Self {
        self.max_members = self.max_members.min(value);
        self
    }
}

/// One owned cancellation/deadline/limit policy captured for a codec invocation.
#[derive(Clone, Debug, Default)]
pub struct CanonicalCodecOptions {
    limits: CanonicalCodecLimits,
    cancellation: AnswerCancellation,
    timeout: Option<Duration>,
}

impl CanonicalCodecOptions {
    /// Return unconstrained-by-caller codec options under the contract ceilings.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Use exact tighten-only codec limits.
    #[must_use]
    pub fn with_limits(mut self, limits: CanonicalCodecLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Use one independently owned sticky cancellation signal.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: AnswerCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Apply a relative timeout captured once as a monotonic deadline at invocation.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

pub(crate) struct CapturedCanonicalCodecControl {
    limits: CanonicalCodecLimits,
    cancellation: AnswerCancellation,
    deadline: Option<Instant>,
    archive: bool,
}

impl CapturedCanonicalCodecControl {
    pub(crate) fn capture(options: &CanonicalCodecOptions, archive: bool) -> crate::Result<Self> {
        let deadline = options
            .timeout
            .map(|timeout| {
                Instant::now().checked_add(timeout).ok_or_else(|| {
                    codec_error(SdkExecutionDiagnostic::projected_codec_deadline_exceeded())
                })
            })
            .transpose()?;
        Ok(Self {
            limits: options.limits,
            cancellation: options.cancellation.clone(),
            deadline,
            archive,
        })
    }

    pub(crate) fn check(&self) -> crate::Result<()> {
        if self.cancellation.is_cancelled() {
            Err(codec_error(
                SdkExecutionDiagnostic::projected_codec_cancelled(),
            ))
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(codec_error(
                SdkExecutionDiagnostic::projected_codec_deadline_exceeded(),
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn input_limits(&self) -> CodecLimits {
        let ceiling = if self.archive {
            MAX_PROJECTED_ARCHIVE_BYTES
        } else {
            MAX_PROJECTED_RECORD_BYTES
        };
        let max_bytes = self.limits.max_input_bytes.min(ceiling);
        CodecLimits {
            max_bytes,
            max_depth: self.limits.max_depth,
            max_collection_len: self.limits.max_members,
            max_string_bytes: MAX_CANONICAL_STRING_BYTES.min(max_bytes),
        }
    }

    pub(crate) fn output_limits(&self) -> CodecLimits {
        let ceiling = if self.archive {
            MAX_PROJECTED_ARCHIVE_BYTES
        } else {
            MAX_PROJECTED_RECORD_BYTES
        };
        let max_bytes = self.limits.max_output_bytes.min(ceiling);
        CodecLimits {
            max_bytes,
            max_depth: self.limits.max_depth,
            max_collection_len: self.limits.max_members,
            max_string_bytes: MAX_CANONICAL_STRING_BYTES.min(max_bytes),
        }
    }

    pub(crate) fn check_record_count(&self, count: usize) -> crate::Result<()> {
        if count > self.limits.max_records {
            Err(codec_error(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn check_decoded_weight(&self, weight: usize) -> crate::Result<()> {
        if weight > self.limits.max_members {
            Err(codec_error(
                SdkExecutionDiagnostic::projected_codec_member_limit(),
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn check_output_bytes(&self, bytes: usize) -> crate::Result<()> {
        if bytes > self.limits.max_output_bytes {
            Err(codec_error(
                SdkExecutionDiagnostic::projected_codec_output_limit(),
            ))
        } else {
            Ok(())
        }
    }
}

pub(crate) fn input_error(error: Diagnostic) -> Error {
    limit_error(error, true)
}

pub(crate) fn output_error(error: Diagnostic) -> Error {
    limit_error(error, false)
}

fn limit_error(error: Diagnostic, input: bool) -> Error {
    let diagnostic = match error.code().as_str() {
        "canonical_json_too_deep" => SdkExecutionDiagnostic::projected_codec_depth_limit(),
        "canonical_collection_too_large" => SdkExecutionDiagnostic::projected_codec_member_limit(),
        "canonical_json_too_large" | "canonical_string_too_large" if input => {
            SdkExecutionDiagnostic::projected_codec_input_limit()
        }
        "canonical_json_too_large" | "canonical_string_too_large" => {
            SdkExecutionDiagnostic::projected_codec_output_limit()
        }
        _ => return Error::from_contract_diagnostic(error),
    };
    codec_error(diagnostic)
}

fn codec_error(error: SdkExecutionDiagnostic) -> Error {
    Error::from_sdk_execution(error, ModelValidationPhase::Input)
}
