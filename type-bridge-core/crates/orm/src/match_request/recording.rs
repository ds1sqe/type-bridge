//! Recording executor used to prove validation, capability, and result seams.
//!
//! This executor performs no TypeQL lowering or I/O. It accepts only an
//! already validated request, runs schema/capability preflight before counting
//! a provider call, answers with the V2 outcome a live executor would return
//! for its canned response, and routes that outcome through the one released
//! result mapping every live query ends in.

use std::collections::VecDeque;
use std::sync::Arc;

use type_bridge_contract::migration_assertion::BindingId as ContractBindingId;
use type_bridge_contract::query_plan::ModelQueryV2;
use type_bridge_contract::query_remote_v2::{
    HydrationGraphV2, RemoteOutcomeV2, RemoteReducedValueV2, RemoteReductionRowV2,
};

use crate::_registry::DescriptorRegistry;
use crate::query_v2_adapter::{MatchRequestAdapterAuthority, adapt_match_request};

use super::capability::CapabilitySet;
use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::BindingId;
use super::model::{MatchOperation, Window};
use super::result::{ReducedValue, ValidatedMatchResult};
use super::result_validation::validated_match_result_from_v2;
use super::selected_result_executor::adapter_match_error;
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

        let outcome = match response {
            RecordingMatchResponse::EmptyRows => RemoteOutcomeV2::HydratedRows {
                graph: empty_graph()?,
                rows: Vec::new(),
            },
            RecordingMatchResponse::EmptyPage { total } => {
                let (root, window) = page_contract(validated);
                RemoteOutcomeV2::HydratedPage {
                    entries: Vec::new(),
                    graph: empty_graph()?,
                    limit: window.limit,
                    offset: window.offset,
                    root: contract_binding(root)?,
                    total,
                }
            }
            RecordingMatchResponse::Count(value) => RemoteOutcomeV2::DistinctCount {
                root: contract_binding(operation_root(validated))?,
                value,
            },
            RecordingMatchResponse::Exists(value) => RemoteOutcomeV2::DistinctExists {
                root: contract_binding(operation_root(validated))?,
                value,
            },
            RecordingMatchResponse::Reduction(values) => model_reduction_outcome(
                &self.registry,
                validated,
                empty_graph()?,
                vec![RemoteReductionRowV2::new(
                    None,
                    values.iter().map(remote_reduced_value).collect(),
                )],
            )?,
            RecordingMatchResponse::EmptyGroupedReduction => {
                model_reduction_outcome(&self.registry, validated, empty_graph()?, Vec::new())?
            }
            RecordingMatchResponse::ProviderFailure { code, message } => {
                return Err(MatchError::new(MatchErrorCategory::Provider, code, message)
                    .at(MatchErrorPathSegment::ProviderEvidence));
            }
        };

        validated_match_result_from_v2(&self.registry, validated, outcome)
    }
}

/// Build the model-reduction outcome a live executor returns for `validated`.
///
/// The outcome echoes the root, group, and reducer contract of the adapted V2
/// plan, obtained through the same registry authority and adapter as live
/// execution. A request that is not a reduction echoes its own root with no
/// contract, which the released mapping refuses as an operation mismatch.
pub(crate) fn model_reduction_outcome(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    graph: HydrationGraphV2,
    rows: Vec<RemoteReductionRowV2>,
) -> Result<RemoteOutcomeV2, MatchError> {
    if !matches!(
        validated.request().operation,
        MatchOperation::ReduceBy { .. }
            | MatchOperation::ReduceByField { .. }
            | MatchOperation::ReduceByFields { .. }
    ) {
        return Ok(RemoteOutcomeV2::ModelReduction {
            graph,
            root: contract_binding(operation_root(validated))?,
            group: None,
            reducers: Vec::new(),
            rows,
        });
    }
    let authority =
        MatchRequestAdapterAuthority::from_registry(registry).map_err(adapter_match_error)?;
    let adapted = adapt_match_request(validated, registry, &authority.context())
        .map_err(adapter_match_error)?;
    let Some(ModelQueryV2::Reduction {
        root,
        group,
        reducers,
        ..
    }) = adapted
        .validated()
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
    else {
        return Err(MatchError::new(
            MatchErrorCategory::ResultDecode,
            "recording_reduction_contract_missing",
            "adapted reduction plan lacks its model reduction contract",
        )
        .at(MatchErrorPathSegment::Operation));
    };
    Ok(RemoteOutcomeV2::ModelReduction {
        graph,
        root: *root,
        group: group.clone(),
        reducers: reducers.clone(),
        rows,
    })
}

fn empty_graph() -> Result<HydrationGraphV2, MatchError> {
    HydrationGraphV2::new(Vec::new()).map_err(adapter_match_error)
}

fn contract_binding(binding: BindingId) -> Result<ContractBindingId, MatchError> {
    ContractBindingId::new(binding.get()).map_err(adapter_match_error)
}

pub(super) fn remote_reduced_value(value: &ReducedValue) -> RemoteReducedValueV2 {
    match value {
        ReducedValue::Count(value) => RemoteReducedValueV2::Count { value: *value },
        ReducedValue::Long(value) => RemoteReducedValueV2::Long { value: *value },
        ReducedValue::Double(value) => RemoteReducedValueV2::DoubleBits {
            value: value.map(f64::to_bits),
        },
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
