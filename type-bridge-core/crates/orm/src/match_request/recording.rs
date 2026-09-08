//! Recording executor used to prove validation, capability, and result seams.
//!
//! This executor performs no TypeQL lowering or I/O. It accepts only an
//! already validated request, runs schema/capability preflight before counting
//! a provider call, constructs invocation-bound canned evidence in Rust, and
//! routes that evidence through the canonical result validator.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::_registry::DescriptorRegistry;

use super::capability::CapabilitySet;
use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::BindingId;
use super::model::{MatchOperation, Window};
use super::result::{ProviderResultEvidence, ReducedValue, ReductionRow, ValidatedMatchResult};
use super::result_validation::validate_provider_result;
use super::validation::ValidatedMatchRequest;

/// One canned provider behavior for [`RecordingMatchExecutor`].
///
/// Row and page responses are deliberately empty: non-empty provider evidence
/// remains an internal Rust executor concern and cannot be forged through this
/// public recording seam. Scalar reduction values are forgeable by design so
/// canonical reduction result validation stays provable offline.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordingMatchResponse {
    /// Return no selected-row solutions.
    EmptyRows,
    /// Return no page solutions and the supplied optional total.
    EmptyPage {
        /// Same-snapshot total claimed by the recording provider.
        total: Option<u64>,
    },
    /// Return one lossless distinct-root count.
    Count(u64),
    /// Return one distinct-root existence value.
    Exists(bool),
    /// Return one ungrouped typed reduction row of scalar values.
    Reduction(Vec<ReducedValue>),
    /// Return a grouped typed reduction with zero witnessed groups.
    EmptyGroupedReduction,
    /// Fail at the provider callback boundary.
    ProviderFailure {
        /// Stable recording-provider error code.
        code: String,
        /// Human-readable recording-provider diagnostic.
        message: String,
    },
}

/// Pure recording executor for canonical request/result integration tests.
#[derive(Debug)]
pub struct RecordingMatchExecutor {
    registry: Arc<DescriptorRegistry>,
    available_capabilities: CapabilitySet,
    responses: VecDeque<RecordingMatchResponse>,
    calls: usize,
}

impl RecordingMatchExecutor {
    /// Create a recorder advertising the complete canonical capability matrix.
    pub fn new(registry: Arc<DescriptorRegistry>) -> Self {
        Self::with_capabilities(registry, CapabilitySet::all())
    }

    /// Create a recorder with an explicit provider capability set.
    pub fn with_capabilities(
        registry: Arc<DescriptorRegistry>,
        available_capabilities: CapabilitySet,
    ) -> Self {
        Self {
            registry,
            available_capabilities,
            responses: VecDeque::new(),
            calls: 0,
        }
    }

    /// Queue one provider behavior in FIFO order.
    pub fn push(&mut self, response: RecordingMatchResponse) {
        self.responses.push_back(response);
    }

    /// Return the number of provider callbacks reached after preflight.
    pub const fn calls(&self) -> usize {
        self.calls
    }

    /// Execute one validated request through preflight and result validation.
    pub fn execute(
        &mut self,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, MatchError> {
        validated.recheck_schema(&self.registry)?;
        validated.require_capabilities(&self.available_capabilities)?;

        let response = self.responses.pop_front().ok_or_else(|| {
            MatchError::new(
                MatchErrorCategory::Provider,
                "recording_response_missing",
                "recording executor has no queued provider response",
            )
            .at(MatchErrorPathSegment::ProviderEvidence)
        })?;
        self.calls = self.calls.checked_add(1).ok_or_else(|| {
            MatchError::new(
                MatchErrorCategory::ResourceLimit,
                "recording_call_overflow",
                "recording executor call counter overflowed",
            )
        })?;

        let request_token = validated.request_token();
        let shape_id = validated.shape_id().clone();
        let evidence = match response {
            RecordingMatchResponse::EmptyRows => {
                ProviderResultEvidence::rows(request_token, shape_id, Vec::new())
            }
            RecordingMatchResponse::EmptyPage { total } => {
                let (root, window) = page_contract(validated);
                ProviderResultEvidence::page(
                    request_token,
                    shape_id,
                    root,
                    Vec::new(),
                    window,
                    total,
                )
            }
            RecordingMatchResponse::Count(value) => ProviderResultEvidence::count(
                request_token,
                shape_id,
                operation_root(validated),
                value,
            ),
            RecordingMatchResponse::Exists(value) => ProviderResultEvidence::exists(
                request_token,
                shape_id,
                operation_root(validated),
                value,
            ),
            RecordingMatchResponse::Reduction(values) => ProviderResultEvidence::reduction(
                request_token,
                shape_id,
                operation_root(validated),
                operation_group(validated),
                vec![ReductionRow::new(None, values)],
            ),
            RecordingMatchResponse::EmptyGroupedReduction => match &validated.request().operation {
                MatchOperation::ReduceByField { root, group, .. } => {
                    ProviderResultEvidence::field_reduction(
                        request_token,
                        shape_id,
                        *root,
                        group.clone(),
                        Vec::new(),
                    )
                }
                MatchOperation::ReduceByFields { root, groups, .. } => {
                    ProviderResultEvidence::field_tuple_reduction(
                        request_token,
                        shape_id,
                        *root,
                        groups.clone(),
                        Vec::new(),
                    )
                }
                _ => ProviderResultEvidence::reduction(
                    request_token,
                    shape_id,
                    operation_root(validated),
                    operation_group(validated),
                    Vec::new(),
                ),
            },
            RecordingMatchResponse::ProviderFailure { code, message } => {
                return Err(MatchError::new(MatchErrorCategory::Provider, code, message)
                    .at(MatchErrorPathSegment::ProviderEvidence));
            }
        };

        validate_provider_result(&self.registry, validated, evidence)
    }
}

fn operation_root(validated: &ValidatedMatchRequest) -> BindingId {
    match &validated.request().operation {
        MatchOperation::PageBy { root, .. }
        | MatchOperation::CountBy { root }
        | MatchOperation::ExistsBy { root }
        | MatchOperation::ReduceBy { root, .. }
        | MatchOperation::ReduceByField { root, .. }
        | MatchOperation::ReduceByFields { root, .. } => *root,
        MatchOperation::FetchRows { .. } => BindingId::new(0),
    }
}

fn operation_group(validated: &ValidatedMatchRequest) -> Option<BindingId> {
    match &validated.request().operation {
        MatchOperation::ReduceBy { group, .. } => *group,
        _ => None,
    }
}

fn page_contract(validated: &ValidatedMatchRequest) -> (BindingId, Window) {
    match &validated.request().operation {
        MatchOperation::PageBy { root, window, .. } => (*root, *window),
        MatchOperation::FetchRows { window, .. } => (BindingId::new(0), *window),
        MatchOperation::CountBy { root }
        | MatchOperation::ExistsBy { root }
        | MatchOperation::ReduceBy { root, .. }
        | MatchOperation::ReduceByField { root, .. }
        | MatchOperation::ReduceByFields { root, .. } => (
            *root,
            Window {
                offset: 0,
                limit: 1,
            },
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use crate::_entity::Annotation;
    use crate::match_request::ids::DescriptorId;
    use crate::match_request::model::{
        FetchShape, FetchSlot, MatchBinding, MatchMode, MatchPlan, MatchRequest, RowCardinality,
        ThingKind,
    };

    fn fixture() -> (Arc<DescriptorRegistry>, MatchRequest) {
        let registry = Arc::new(DescriptorRegistry::new());
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "name".into(),
                    attr_name: "person-name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_optional: false,
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let request = MatchRequest::v1(
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
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        );
        (registry, request)
    }

    #[test]
    fn capability_preflight_fails_before_recording_callback() {
        let (registry, request) = fixture();
        let validated = request.validate(&registry).unwrap();
        let mut executor =
            RecordingMatchExecutor::with_capabilities(registry, CapabilitySet::new());
        executor.push(RecordingMatchResponse::EmptyRows);

        let error = executor.execute(&validated).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
        assert_eq!(executor.calls(), 0);
    }

    #[test]
    fn empty_exactly_one_uses_canonical_no_result_error() {
        let (registry, request) = fixture();
        let validated = request.validate(&registry).unwrap();
        let mut executor = RecordingMatchExecutor::new(registry);
        executor.push(RecordingMatchResponse::EmptyRows);

        let error = executor.execute(&validated).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::Cardinality);
        assert_eq!(error.code().as_str(), "no_result");
        assert_eq!(executor.calls(), 1);
    }
}
