//! Static model projection for authored native read plans.

use super::descriptor_plan::{
    MatchRequestAdapterAuthority, build_hydration, descriptor_id_type, integrity,
};
use super::{
    DescriptorId, FetchShape, MatchBinding, MatchMode, MatchOperation, MatchPlan, MatchRequest,
    RowCardinality, ThingKind, Window,
};
use crate::_registry::DescriptorRegistry;
use std::collections::{BTreeMap, BTreeSet};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::v3::{
    Expr, Operation, Pattern, PlanSpec, QueryPlanV3, Stage, ThingDocument,
};
use type_bridge_contract::query_plan::{
    HydrationProjectionV2, QueryOperand, QueryOutput, QueryPattern,
};

/// A validated selection and its static, complete model hydration plans.
///
/// Execute `plans()` in order in one transaction, then call `resolve()` before
/// publishing any model or scalar result.
pub struct PlanFetchStack {
    pub(crate) plans: Vec<QueryPlanV3>,
    bulk: Option<Box<type_bridge_contract::query_plan::v3::BuiltPlans>>,
    pub(crate) selection: Option<usize>,
    pub(crate) mapping: Option<usize>,
    pub(crate) partitions: Vec<(usize, BindingId, Vec<TypeId>)>,
    pub(crate) hydration: HydrationProjectionV2,
    pub(crate) model_slots: Vec<(BindingId, DescriptorId)>,
    schema_proof: Option<(
        Vec<crate::_registry::DescriptorFingerprintRoot>,
        super::SchemaFingerprint,
    )>,
}

impl PlanFetchStack {
    /// Executed selection metadata for native scalar decoding.
    pub fn selection_plan(&self) -> &QueryPlanV3 {
        self.bulk.as_ref().map_or_else(
            || &self.plans[self.selection.unwrap_or(0)],
            |plans| plans.projection_plan(),
        )
    }
    /// Ordered read-only plans sharing one selection prefix.
    pub fn plans(&self) -> &[QueryPlanV3] {
        self.bulk
            .as_ref()
            .map_or(self.plans.as_slice(), |plans| plans.plans())
    }
    /// Whether the authored group writes, including an empty model bulk.
    pub fn writes(&self) -> bool {
        self.bulk.as_ref().map_or_else(
            || self.plans.iter().any(QueryPlanV3::writes),
            |plans| plans.writes(),
        )
    }
}

/// Split a native read selection into a static complete model-fetch stack.
///
/// `model_slots` identifies every selected thing binding and its declared model
/// descriptor. Scalar columns retain the authored plan's ordering and multiplicity.
/// Unsupported shapes are refused before provider execution.
pub fn build_plan_fetch_stack(
    registry: &DescriptorRegistry,
    plan: &QueryPlanV3,
    model_slots: &[(BindingId, DescriptorId)],
) -> Result<PlanFetchStack, Diagnostic> {
    build_fetch_stack(registry, plan, model_slots, false)
}

/// Manager selections already collapse roots before sorting and windowing.
pub(crate) fn build_unique_plan_fetch_stack(
    registry: &DescriptorRegistry,
    plan: &QueryPlanV3,
    model_slots: &[(BindingId, DescriptorId)],
) -> Result<PlanFetchStack, Diagnostic> {
    build_fetch_stack(registry, plan, model_slots, true)
}

fn build_fetch_stack(
    registry: &DescriptorRegistry,
    plan: &QueryPlanV3,
    model_slots: &[(BindingId, DescriptorId)],
    unique: bool,
) -> Result<PlanFetchStack, Diagnostic> {
    if model_slots.len() > type_bridge_contract::limits::MAX_SELECTED_SLOTS {
        return Err(integrity(
            "plan_returning_models_slot_limit",
            "model projection exceeds the supported model slot count",
        ));
    }
    let authority = MatchRequestAdapterAuthority::from_registry(registry)?;
    let mut stack = build_under_authority(
        registry,
        plan,
        model_slots,
        &authority,
        unique,
        &[],
        FetchProof::CompleteModel,
    )?;
    let roots = model_slots
        .iter()
        .map(|(binding, descriptor)| {
            let declared = descriptor_id_type(descriptor.as_str())?;
            let exact = plan
                .binding_types(*binding)
                .is_some_and(|types| types.len() == 1 && types.contains(&declared));
            Ok(crate::_registry::DescriptorFingerprintRoot::new(
                descriptor.clone(),
                !exact,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let fingerprint = registry.request_relevant_fingerprint(&roots).map_err(|_| {
        integrity(
            "plan_returning_models_descriptor",
            "model descriptors cannot be fingerprinted",
        )
    })?;
    stack.schema_proof = Some((roots, fingerprint));
    Ok(stack)
}

/// Prepare complete authored bulk model results for precommit group publication.
/// The core retains the chunk correlation evidence and restores original input
/// order before issuing one model proof. Stage this with Transaction::stage_plan_fetch.
pub fn build_bulk_plan_fetch_stack(
    registry: &DescriptorRegistry,
    plans: type_bridge_contract::query_plan::v3::BuiltPlans,
    model_slot: (BindingId, DescriptorId),
) -> Result<PlanFetchStack, Diagnostic> {
    plans.validate_model_projection()?;
    let mut stack = build_plan_fetch_stack(registry, plans.projection_plan(), &[model_slot])?;
    if plans.bulk_rows().is_some() {
        stack.plans.clear();
        stack.bulk = Some(Box::new(plans));
    }
    Ok(stack)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchProof {
    /// Proves complete models against the authored selection baseline.
    CompleteModel,
    /// Proves selected models and their correlated hidden witnesses.
    CorrelatedMatch,
}

pub(crate) fn build_under_authority(
    registry: &DescriptorRegistry,
    plan: &QueryPlanV3,
    model_slots: &[(BindingId, DescriptorId)],
    authority: &MatchRequestAdapterAuthority,
    unique_selection: bool,
    witness_columns: &[BindingId],
    proof: FetchProof,
) -> Result<PlanFetchStack, Diagnostic> {
    let refusal = || {
        integrity(
            "plan_returning_models_shape",
            "the authored plan cannot be split into a complete static model projection",
        )
    };
    if plan.fetch_document_shape().is_some() {
        return build_single_fetch(registry, plan, model_slots, authority);
    }
    if plan.writes() {
        return Err(integrity(
            "plan_returning_models_after_write",
            "model projection after writes requires one complete model-only terminal fetch",
        ));
    }
    if plan.operation() != Operation::Rows
        || plan.compatibility().is_some()
        || model_slots.is_empty()
        || plan.managed_semantics()
            != authority
                .context()
                .managed_state()
                .managed_semantic_schema()
    {
        return Err(refusal());
    }
    let QueryOutput::Rows { columns } = plan.output() else {
        return Err(refusal());
    };
    if plan
        .pipeline()
        .iter()
        .any(|stage| matches!(stage, Stage::Fetch(_) | Stage::Reduce { .. }))
    {
        return Err(refusal());
    }
    let mut slots = model_slots.to_vec();
    slots.sort_by_key(|(binding, _)| *binding);
    if slots.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(refusal());
    }
    let mut bindings = Vec::new();
    for (binding, descriptor) in &slots {
        let declared = descriptor_id_type(descriptor.as_str())?;
        let mode = match plan.binding_types(*binding) {
            Some(types) if types.len() == 1 && types.contains(&declared) => MatchMode::Exact,
            Some(types) if !types.is_empty() => MatchMode::Subtypes,
            _ => return Err(refusal()),
        };
        bindings.push(MatchBinding {
            id: super::BindingId::new(binding.get()),
            descriptor: descriptor.clone(),
            thing_kind: match declared.kind() {
                TypeKind::Entity => ThingKind::Entity,
                TypeKind::Relation => ThingKind::Relation,
                _ => return Err(refusal()),
            },
            match_mode: mode,
        });
    }
    let metadata = MatchRequest::v1(
        MatchPlan {
            bindings,
            predicate: None,
            allowed_cross_joins: BTreeSet::new(),
        },
        MatchOperation::FetchRows {
            output: FetchShape::Positional { slots: vec![] },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 1,
            },
            cardinality: RowCardinality::Complete,
        },
    );
    let hydration = build_hydration(&metadata, registry, authority.context().resolved_schema())?;
    for binding in hydration.bindings() {
        if plan.binding_types(binding.binding()).is_none_or(|types| {
            !types.iter().all(|id| {
                binding.concrete_descriptors().contains(id)
                    || authority
                        .context()
                        .resolved_schema()
                        .types()
                        .get(id)
                        .is_some_and(|model| model.is_abstract())
            })
        }) {
            return Err(refusal());
        }
    }
    let descriptor = |id: &TypeId| {
        hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == id)
            .ok_or_else(refusal)
    };
    let first = hydration
        .bindings()
        .first()
        .and_then(|binding| binding.concrete_descriptors().first());
    let first_roles = first
        .map(|id| super::descriptor_plan::document_roles(descriptor(id)?, authority))
        .transpose()?;
    let shared_roles = hydration.bindings().first().is_some_and(|binding| {
        binding.concrete_descriptors().iter().all(|id| {
            descriptor(id)
                .and_then(|model| {
                    let fields_match = first.is_some_and(|first| {
                        descriptor(first).is_ok_and(|first| same_hydration_fields(first, model))
                    });
                    Ok((
                        super::descriptor_plan::document_roles(model, authority)?,
                        fields_match,
                    ))
                })
                .is_ok_and(|(roles, fields)| fields && Some(&roles) == first_roles.as_ref())
        })
    });
    let identity_columns = plan
        .bindings()
        .iter()
        .filter(|binding| {
            plan.binding_types(binding.id()).is_some_and(|types| {
                types
                    .iter()
                    .any(|id| matches!(id.kind(), TypeKind::Entity | TypeKind::Relation))
            })
        })
        .map(|binding| binding.id())
        .collect::<Vec<_>>();
    let one_fetch = unique_selection
        && (proof == FetchProof::CompleteModel || identity_columns.len() == 1)
        && slots.len() == 1
        && columns == &[slots[0].0]
        && shared_roles
        && first_roles.is_some();
    if one_fetch {
        let binding = slots[0].0;
        let mut pipeline = plan.pipeline().to_vec();
        pipeline.push(Stage::Fetch(ThingDocument::new(
            binding,
            first_roles.unwrap(),
        )));
        return Ok(PlanFetchStack {
            plans: vec![rebuild(plan, pipeline, vec![], authority)?],
            selection: None,
            mapping: None,
            partitions: vec![(
                0,
                binding,
                hydration.bindings()[0].concrete_descriptors().to_vec(),
            )],
            hydration,
            model_slots: slots,
            schema_proof: None,
            bulk: None,
        });
    }
    let mut source = retain_inputs(plan, authority)?;
    let mut prefix = source.pipeline().to_vec();
    if !prefix
        .iter()
        .any(|stage| matches!(stage, Stage::Select { .. }))
    {
        let mut retained = columns.clone();
        for stage in &prefix {
            match stage {
                Stage::Sort { terms } => retained.extend(terms.iter().map(|term| term.binding())),
                Stage::Match { patterns } => {
                    retained.extend(patterns.iter().filter_map(|pattern| match pattern {
                        Pattern::Let {
                            binding,
                            expr: Expr::Operand(QueryOperand::Input { .. }),
                        } => Some(*binding),
                        _ => None,
                    }))
                }
                _ => {}
            }
        }
        retained.sort();
        retained.dedup();
        prefix.insert(1, Stage::Select { bindings: retained });
        source = rebuild(&source, prefix.clone(), columns.clone(), authority)?;
    }
    let Stage::Match { patterns: original } = prefix.first().ok_or_else(refusal)? else {
        return Err(refusal());
    };
    if prefix
        .iter()
        .skip(1)
        .any(|stage| matches!(stage, Stage::Match { .. }))
    {
        return Err(refusal());
    }
    let retained = prefix
        .iter()
        .rev()
        .find_map(|stage| match stage {
            Stage::Select { bindings } => Some(bindings.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            source
                .bindings()
                .iter()
                .map(|binding| binding.id())
                .collect()
        });
    let rematch = original
        .iter()
        .filter(|pattern| match pattern {
            Pattern::Let { binding, .. } => !retained.contains(binding),
            Pattern::Read(QueryPattern::FunctionCall { assigned, .. }) => {
                !retained.contains(assigned)
            }
            _ => true,
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut plans = Vec::new();
    let (selection, mapping, partitions) = {
        // A lone thing's independent fetch is also its selection baseline.
        let baseline = (unique_selection
            && columns.len() == 1
            && slots.iter().any(|(binding, _)| binding == &columns[0]))
        .then_some(columns[0]);
        let selection = if !unique_selection || proof == FetchProof::CompleteModel {
            plans.push(source.clone());
            Some(0)
        } else {
            None
        };
        let mut pipeline = prefix.clone();
        if !pipeline
            .iter()
            .any(|stage| matches!(stage, Stage::Distinct))
        {
            pipeline.push(Stage::Distinct);
        }
        pipeline.push(Stage::Match {
            patterns: rematch.clone(),
        });
        let mut mapping_columns = columns.clone();
        mapping_columns.extend(&identity_columns);
        mapping_columns.extend(witness_columns);
        mapping_columns.sort();
        mapping_columns.dedup();
        pipeline.push(Stage::Select {
            bindings: mapping_columns.clone(),
        });
        pipeline.push(Stage::Distinct);
        let mapping = plans.len();
        plans.push(rebuild(&source, pipeline, mapping_columns, authority)?);
        let partitions = append_hydration_partitions(
            FetchPrefix {
                source: &source,
                stages: &prefix,
                rematch: &rematch,
                baseline,
            },
            hydration.bindings(),
            &hydration,
            authority,
            &mut plans,
        )?;
        (selection, Some(mapping), partitions)
    };
    Ok(PlanFetchStack {
        plans,
        selection,
        mapping,
        partitions,
        hydration,
        model_slots: slots,
        schema_proof: None,
        bulk: None,
    })
}

fn rebuild(
    plan: &QueryPlanV3,
    pipeline: Vec<Stage>,
    columns: Vec<BindingId>,
    authority: &MatchRequestAdapterAuthority,
) -> Result<QueryPlanV3, Diagnostic> {
    rebuild_with_bindings(plan, plan.bindings().to_vec(), pipeline, columns, authority)
}

fn rebuild_with_bindings(
    plan: &QueryPlanV3,
    bindings: Vec<AssertionBinding>,
    pipeline: Vec<Stage>,
    columns: Vec<BindingId>,
    authority: &MatchRequestAdapterAuthority,
) -> Result<QueryPlanV3, Diagnostic> {
    QueryPlanV3::new_with_limits(
        PlanSpec {
            bindings,
            inputs: plan.inputs().to_vec(),
            given_rows: plan.given_rows().map(<[_]>::to_vec),
            operation: Operation::Rows,
            pipeline,
            output: QueryOutput::Rows { columns },
            managed_semantics: plan.managed_semantics().clone(),
            compatibility: None,
            functions: plan.functions().to_vec(),
        },
        authority.declared(),
        plan.limits(),
    )
}

fn retain_inputs(
    plan: &QueryPlanV3,
    authority: &MatchRequestAdapterAuthority,
) -> Result<QueryPlanV3, Diagnostic> {
    if plan.inputs().is_empty() {
        return Ok(plan.clone());
    }
    let mut bindings = plan.bindings().to_vec();
    let mut aliases = BTreeMap::new();
    let mut lets = Vec::new();
    for input in plan.inputs() {
        if input.optional() {
            return Err(integrity(
                "plan_returning_models_optional_input",
                "splitting a model selection with optional given columns is unsupported",
            ));
        }
        let id = BindingId::new(u16::try_from(bindings.len()).map_err(|_| {
            integrity(
                "plan_returning_models_binding_limit",
                "model projection exceeds the binding range",
            )
        })?)?;
        let mut name = format!("plan_model_input{}", input.id().get());
        while bindings
            .iter()
            .any(|binding| binding.variable().as_str() == name)
            || plan
                .inputs()
                .iter()
                .any(|input| input.public_name().as_str() == name)
        {
            name.push('x');
        }
        bindings.push(AssertionBinding::new(id, QueryVariable::new(name)?));
        aliases.insert(input.id(), id);
        lets.push(Pattern::Let {
            binding: id,
            expr: Expr::Operand(QueryOperand::Input { column: input.id() }),
        });
    }
    if plan.given_rows().is_some_and(|rows| rows.len() > 1)
        && plan
            .pipeline()
            .iter()
            .any(|stage| matches!(stage, Stage::Distinct))
    {
        return Err(integrity(
            "plan_returning_models_given_window",
            "splitting distinct model selections across multiple given rows is unsupported",
        ));
    }
    let mut pipeline = plan.pipeline().to_vec();
    for stage in &mut pipeline {
        if let Stage::Select { bindings } = stage {
            bindings.extend(aliases.values());
            bindings.sort();
            bindings.dedup();
        }
    }
    let Some(Stage::Match { patterns }) = pipeline.first_mut() else {
        return Err(integrity(
            "plan_returning_models_shape",
            "model selection must start with a match",
        ));
    };
    patterns.splice(0..0, lets);
    QueryPlanV3::new_with_limits(
        PlanSpec {
            bindings,
            inputs: plan.inputs().to_vec(),
            given_rows: plan.given_rows().map(<[_]>::to_vec),
            operation: plan.operation(),
            pipeline,
            output: plan.output().clone(),
            managed_semantics: plan.managed_semantics().clone(),
            compatibility: None,
            functions: plan.functions().to_vec(),
        },
        authority.declared(),
        plan.limits(),
    )
}

/// One sealed model-tuple proof and core-produced references for selection rows.
pub struct PlanFetchResult<'a> {
    selection: &'a crate::query_plan_exec::PlanAnswer,
    request: super::ValidatedMatchRequest,
    result: super::ValidatedMatchResult,
    model_bindings: Vec<BindingId>,
    model_rows: Vec<usize>,
    #[cfg(test)]
    validation_count: usize,
}
/// One cell selected by an authored model/scalar tuple, borrowed from its proof.
pub enum PlanFetchCell<'a> {
    /// Already validated model proof row and slot.
    Model {
        /// Row of the shared complete-model proof.
        row: usize,
        /// Slot of that proof row.
        slot: usize,
    },
    /// Canonical scalar retained in the validated selection evidence.
    Scalar(&'a type_bridge_contract::value::CanonicalValue),
}

/// Owned proof published once for one committed authored model projection.
pub struct OwnedPlanFetchResult {
    evidence: Option<std::sync::Arc<type_bridge_contract::query_remote_v3::StagedAnswerV3>>,
    columns: Vec<BindingId>,
    selection: std::sync::Arc<crate::query_plan_exec::PlanAnswer>,
    request: super::ValidatedMatchRequest,
    result: super::ValidatedMatchResult,
    model_bindings: Vec<BindingId>,
    model_rows: Vec<usize>,
}
impl OwnedPlanFetchResult {
    /// Original selection, preserving scalar values and duplicate rows.
    pub fn selection(&self) -> &crate::query_plan_exec::PlanAnswer {
        &self.selection
    }
    /// Share the original selection without copying rows or scalar values.
    pub fn shared_selection(&self) -> std::sync::Arc<crate::query_plan_exec::PlanAnswer> {
        std::sync::Arc::clone(&self.selection)
    }
    /// Selected bindings in authored tuple order.
    pub fn columns(&self) -> &[BindingId] {
        &self.columns
    }
    /// Number of authored row occurrences, including duplicates.
    pub fn row_count(&self) -> usize {
        self.model_rows.len()
    }
    /// Borrow a scalar or obtain a model ordinal without rejoining provider rows.
    pub fn cell(&self, row: usize, binding: BindingId) -> Option<PlanFetchCell<'_>> {
        if let Some((row, slot)) = self.model_ordinal(row, binding) {
            return Some(PlanFetchCell::Model { row, slot });
        }
        let type_bridge_contract::query_remote_v3::StagedAnswerV3::Rows { rows } =
            self.evidence.as_deref()?
        else {
            return None;
        };
        let column = self
            .columns
            .iter()
            .position(|candidate| *candidate == binding)?;
        match rows.get(row)?.get(column)? {
            type_bridge_contract::query_remote::RemoteValue::Value { value }
            | type_bridge_contract::query_remote::RemoteValue::Attribute { value, .. } => {
                Some(PlanFetchCell::Scalar(value))
            }
            _ => None,
        }
    }
    /// Shared request proof for projected model mapping.
    pub fn request(&self) -> &super::ValidatedMatchRequest {
        &self.request
    }
    /// Complete model result proof.
    pub fn result(&self) -> &super::ValidatedMatchResult {
        &self.result
    }
    /// Authored model bindings in proof-slot order.
    pub fn model_bindings(&self) -> &[BindingId] {
        &self.model_bindings
    }
    /// Selection row to proof row, including duplicate occurrences.
    pub fn model_rows(&self) -> &[usize] {
        &self.model_rows
    }
    /// Proof row and slot for one selected model occurrence.
    pub fn model_ordinal(&self, row: usize, binding: BindingId) -> Option<(usize, usize)> {
        Some((
            *self.model_rows.get(row)?,
            self.model_bindings.binary_search(&binding).ok()?,
        ))
    }
}
impl PlanFetchResult<'_> {
    pub(crate) fn into_owned(
        self,
        selection: std::sync::Arc<crate::query_plan_exec::PlanAnswer>,
    ) -> OwnedPlanFetchResult {
        debug_assert!(std::ptr::eq(self.selection, selection.as_ref()));
        OwnedPlanFetchResult {
            evidence: None,
            columns: self.model_bindings.clone(),
            selection,
            request: self.request,
            result: self.result,
            model_bindings: self.model_bindings,
            model_rows: self.model_rows,
        }
    }
}
impl AsRef<[QueryPlanV3]> for PlanFetchStack {
    fn as_ref(&self) -> &[QueryPlanV3] {
        self.plans()
    }
}
impl AsRef<PlanFetchStack> for PlanFetchStack {
    fn as_ref(&self) -> &PlanFetchStack {
        self
    }
}
impl PlanFetchResult<'_> {
    /// Original scalar selection, preserving order and duplicate occurrences.
    pub fn selection(&self) -> &crate::query_plan_exec::PlanAnswer {
        self.selection
    }
    /// Dense model-tuple request proof consumed by the shared projected mapper.
    pub fn request(&self) -> &super::ValidatedMatchRequest {
        &self.request
    }
    /// Complete model tuples, validated once under the shared request proof.
    pub fn result(&self) -> &super::ValidatedMatchResult {
        &self.result
    }
    /// Authored model bindings in proof-slot order.
    pub fn model_bindings(&self) -> &[BindingId] {
        &self.model_bindings
    }
    /// Selection row to shared proof row, including repeated occurrences.
    pub fn model_rows(&self) -> &[usize] {
        &self.model_rows
    }
    #[cfg(test)]
    pub(crate) fn validation_count(&self) -> usize {
        self.validation_count
    }
    /// Core-owned proof row and slot for one selected model occurrence.
    pub fn model_ordinal(&self, row: usize, binding: BindingId) -> Option<(usize, usize)> {
        Some((
            *self.model_rows.get(row)?,
            self.model_bindings.binary_search(&binding).ok()?,
        ))
    }
}

impl PlanFetchStack {
    /// Resolve an unset statement policy to this adapter's actual static need.
    pub fn execution_limits(
        &self,
        limits: crate::QueryExecutionResourceLimits,
    ) -> crate::QueryExecutionResourceLimits {
        limits.for_adapter_statements(self.plans().len())
    }

    /// Validate the entire stack and publish model proofs atomically.
    pub fn resolve<'a>(
        &self,
        registry: &DescriptorRegistry,
        answers: &'a [crate::query_plan_exec::PlanAnswer],
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<PlanFetchResult<'a>, super::MatchError> {
        if self.bulk.is_some() {
            return Err(super::MatchError::new(
                super::MatchErrorCategory::ResultDecode,
                "plan_batch_requires_staging",
                "bulk model proofs require the staged group path",
            ));
        }
        let normalized = self
            .plans()
            .iter()
            .zip(answers)
            .map(|(plan, answer)| {
                let wire = answer.to_staged_v3(plan)?;
                if let type_bridge_contract::query_remote_v3::StagedAnswerV3::Documents {
                    documents,
                } = &wire
                {
                    if let Some(expected) = plan.standalone_entity_rows() {
                        expected.correlate_proof(documents)?;
                    }
                } else {
                    wire.validate_for(plan)?;
                }
                Ok(wire)
            })
            .collect::<Result<Vec<_>, Diagnostic>>()
            .map_err(|error| {
                super::selected_result_executor::released_plan_execution_error(&error)
            })?;
        self.resolve_verified(
            registry,
            &answers.iter().collect::<Vec<_>>(),
            &normalized.iter().collect::<Vec<_>>(),
            limits,
        )
    }

    pub(crate) fn resolve_verified<'a>(
        &self,
        registry: &DescriptorRegistry,
        answers: &[&'a crate::query_plan_exec::PlanAnswer],
        normalized: &[&type_bridge_contract::query_remote_v3::StagedAnswerV3],
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<PlanFetchResult<'a>, super::MatchError> {
        let fail = |error| super::selected_result_executor::released_plan_execution_error(&error);
        self.meter_verified(answers, normalized, limits)
            .map_err(fail)?;
        self.check_schema(registry)?;
        if self.mapping.is_none()
            && self.selection.is_none()
            && self.selection_plan().fetch_document_shape().is_some()
        {
            if answers.len() != 1 {
                return Err(super::MatchError::new(
                    super::MatchErrorCategory::ResultDecode,
                    "query_plan_result_stack",
                    "single model fetch requires one answer",
                ));
            }
            let crate::query_plan_exec::PlanAnswer::Documents(documents) = &answers[0] else {
                return Err(super::MatchError::new(
                    super::MatchErrorCategory::ResultDecode,
                    "query_plan_result_documents",
                    "single model fetch did not return documents",
                ));
            };
            return self.resolve_single(registry, answers[0], documents, limits);
        }
        self.resolve_inner(registry, answers, normalized, limits)
            .map_err(|error| super::selected_result_executor::released_plan_execution_error(&error))
    }

    fn check_schema(&self, registry: &DescriptorRegistry) -> Result<(), super::MatchError> {
        if let Some((roots, expected)) = &self.schema_proof {
            let actual = registry.request_relevant_fingerprint(roots).map_err(|_| {
                super::MatchError::new(
                    super::MatchErrorCategory::StaleSchema,
                    "stale_schema",
                    "model descriptors changed after plan construction",
                )
            })?;
            if &actual != expected {
                return Err(super::MatchError::new(
                    super::MatchErrorCategory::StaleSchema,
                    "stale_schema",
                    "model descriptors changed after plan construction",
                )
                .with_detail("expected", expected.as_str())
                .with_detail("actual", actual.as_str()));
            }
        }
        Ok(())
    }

    fn resolve_single<'a>(
        &self,
        registry: &DescriptorRegistry,
        answer: &'a crate::query_plan_exec::PlanAnswer,
        documents: &[serde_json::Value],
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<PlanFetchResult<'a>, super::MatchError> {
        let (request, result) =
            super::result_validation::validated_match_result_from_verified_plan(
                registry,
                self.selection_plan(),
                documents,
                &self.model_slots[0].1,
                limits,
            )?;
        let super::MatchResult::Rows { rows } = result.for_request(&request)? else {
            unreachable!("single fetch model result")
        };
        let model_rows = (0..rows.len()).collect();
        Ok(PlanFetchResult {
            selection: answer,
            request,
            result,
            model_bindings: vec![self.model_slots[0].0],
            model_rows,
            #[cfg(test)]
            validation_count: 1,
        })
    }

    pub(crate) fn resolve_group(
        &self,
        registry: &DescriptorRegistry,
        answers: &[std::sync::Arc<crate::query_plan_exec::PlanAnswer>],
        evidence: &[std::sync::Arc<type_bridge_contract::query_remote_v3::StagedAnswerV3>],
        correlations: &[Option<&type_bridge_contract::query_plan::v3::BulkCorrelation>],
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<OwnedPlanFetchResult, super::MatchError> {
        let references = answers
            .iter()
            .map(|answer| answer.as_ref())
            .collect::<Vec<_>>();
        let normalized = evidence
            .iter()
            .map(|wire| wire.as_ref())
            .collect::<Vec<_>>();
        if let Some(plans) = &self.bulk {
            let fail =
                |error| super::selected_result_executor::released_plan_execution_error(&error);
            self.meter_verified(&references, &normalized, limits)
                .map_err(fail)?;
            self.check_schema(registry)?;
            let merged = crate::query_plan_exec::merge_plan_answers(plans, answers, correlations)
                .map_err(fail)?;
            let crate::query_plan_exec::PlanAnswer::Documents(documents) = merged.as_ref() else {
                unreachable!("model bulk retains a document projection")
            };
            let proof = self.resolve_single(registry, merged.as_ref(), documents, limits)?;
            return Ok(proof.into_owned(std::sync::Arc::clone(&merged)));
        }
        let proof = self.resolve_verified(registry, &references, &normalized, limits)?;
        let mut owned =
            proof.into_owned(std::sync::Arc::clone(&answers[self.selection.unwrap_or(0)]));
        if let Some(index) = self.selection {
            owned.evidence = Some(std::sync::Arc::clone(&evidence[index]));
            if let QueryOutput::Rows { columns } = self.selection_plan().output() {
                owned.columns = columns.clone();
            }
        }
        Ok(owned)
    }

    fn meter_verified(
        &self,
        answers: &[&crate::query_plan_exec::PlanAnswer],
        normalized: &[&type_bridge_contract::query_remote_v3::StagedAnswerV3],
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<(), Diagnostic> {
        use crate::query_plan_exec::PlanAnswer;
        if answers.len() != self.plans().len() || normalized.len() != answers.len() {
            return Err(integrity(
                "query_plan_result_stack",
                "model fetch answer count differs from its plans",
            ));
        }
        let cancel = crate::session::backend::AnswerCancellation::default();
        let answer_limits = crate::query_plan_exec::plan_answer_limits(
            limits,
            &cancel,
            crate::QueryExecutionDeadline::for_limits(limits),
        )
        .map_err(|error| budget_failure(error, limits))?;
        let mut bytes = crate::answer_meter::AnswerMeter::new(&answer_limits.answer);
        let mut meter = crate::answer_meter::PlanMeter::new(&answer_limits);
        for (answer, wire) in answers.iter().zip(normalized) {
            if let PlanAnswer::Rows(values) | PlanAnswer::Documents(values) = answer {
                for value in values {
                    bytes
                        .accept_value(value)
                        .map_err(|error| budget_failure(error, limits))?;
                }
            }
            meter
                .accept(wire)
                .map_err(|error| budget_failure(error, limits))?;
        }
        Ok(())
    }

    fn resolve_inner<'a>(
        &self,
        registry: &DescriptorRegistry,
        answers: &[&'a crate::query_plan_exec::PlanAnswer],
        normalized: &[&type_bridge_contract::query_remote_v3::StagedAnswerV3],
        limits: crate::QueryExecutionResourceLimits,
    ) -> Result<PlanFetchResult<'a>, Diagnostic> {
        use super::plan_hydration::{GraphBuilder, HydrationAuthority, insert_plan_documents};
        use crate::query_plan_exec::PlanAnswer;
        use type_bridge_contract::query_remote::RemoteValue;
        use type_bridge_contract::query_remote_v2::{
            HydratedRowV2, HydrationSlotV2, RemoteOutcomeV2, RemoteReplyDecodeLimitsV2,
        };
        use type_bridge_contract::query_remote_v3::StagedAnswerV3;
        let fail = || {
            integrity(
                "query_plan_result_window",
                "model projection evidence does not match its authored selection",
            )
        };
        let shared = |error| match error {
            crate::error::PlanExecutionError::Validation(error) => error,
            crate::error::PlanExecutionError::Provider(_) => fail(),
        };
        let index = self.selection.ok_or_else(fail)?;
        let selection = answers[index];
        let StagedAnswerV3::Rows { rows: selected } = &normalized[index] else {
            return Err(fail());
        };
        let QueryOutput::Rows { columns } = self.selection_plan().output() else {
            return Err(fail());
        };
        let mapping_index = self.mapping.ok_or_else(fail)?;
        let mapping_plan = &self.plans()[mapping_index];
        let StagedAnswerV3::Rows { rows: mapping } = &normalized[mapping_index] else {
            return Err(fail());
        };
        let QueryOutput::Rows {
            columns: mapping_columns,
        } = mapping_plan.output()
        else {
            return Err(fail());
        };
        let key = |values: &[RemoteValue]| serde_json::to_string(values).map_err(|_| fail());
        let expected = selected
            .iter()
            .map(|row| key(row))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let actual = mapping
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| {
                        mapping_columns
                            .iter()
                            .position(|candidate| candidate == column)
                            .and_then(|index| row.get(index))
                            .cloned()
                            .ok_or_else(fail)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|values| key(&values))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if expected != actual {
            return Err(fail());
        }
        let authority = HydrationAuthority::new(&self.hydration).map_err(shared)?;
        let reply_limits = RemoteReplyDecodeLimitsV2 {
            max_bytes: limits.bytes,
            max_items: limits.items,
            max_graph_nodes: limits.graph_nodes,
            max_attribute_values: limits.attribute_values,
            max_collection_members: limits.collection_members,
            max_role_players: limits.role_players,
        };
        let mut graph = GraphBuilder::new(reply_limits);
        for (index, binding, concrete) in &self.partitions {
            let expected = mapping
                .iter()
                .map(|row| {
                    let position = mapping_columns
                        .iter()
                        .position(|candidate| candidate == binding)
                        .ok_or_else(fail)?;
                    match row.get(position) {
                        Some(RemoteValue::Thing { iid, type_id }) if concrete.contains(type_id) => {
                            Ok(Some(iid.clone()))
                        }
                        Some(RemoteValue::Thing { type_id, .. })
                            if authority
                                .binding(binding.get())
                                .map_err(shared)?
                                .concrete_descriptors()
                                .contains(type_id) =>
                        {
                            Ok(None)
                        }
                        _ => Err(fail()),
                    }
                })
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten()
                .collect::<BTreeSet<_>>();
            let Some(PlanAnswer::Documents(documents)) = answers.get(*index) else {
                return Err(fail());
            };
            let actual = insert_plan_documents(
                &mut graph,
                &authority,
                binding.get(),
                &self.plans()[*index],
                documents,
            )
            .map_err(shared)?
            .into_iter()
            .collect::<BTreeSet<_>>();
            if expected != actual {
                return Err(fail());
            }
        }
        let positions = self
            .model_slots
            .iter()
            .map(|(binding, _)| {
                columns
                    .iter()
                    .position(|candidate| candidate == binding)
                    .ok_or_else(fail)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let selected_tuples = selected
            .iter()
            .map(|row| {
                positions
                    .iter()
                    .zip(&self.model_slots)
                    .map(|(position, (binding, _))| match row.get(*position) {
                        Some(RemoteValue::Thing { iid, type_id })
                            if authority
                                .binding(binding.get())
                                .map_err(shared)?
                                .concrete_descriptors()
                                .contains(type_id) =>
                        {
                            Ok(iid.clone())
                        }
                        _ => Err(fail()),
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tuples = selected_tuples.iter().cloned().collect::<BTreeSet<_>>();
        let roots = tuples.iter().flatten().cloned().collect::<BTreeSet<_>>();
        let finished = graph.finish(&roots).map_err(shared)?;
        let declared = self
            .model_slots
            .iter()
            .map(|(_, descriptor)| descriptor_id_type(descriptor.as_str()))
            .collect::<Result<Vec<_>, _>>()?;
        let rows = tuples
            .iter()
            .map(|tuple| {
                tuple
                    .iter()
                    .zip(&declared)
                    .map(|(iid, declared)| {
                        finished
                            .reference(declared, iid)
                            .map(|value| HydrationSlotV2::Singular { value })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(HydratedRowV2::new)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(shared)?;
        let dense = |index| {
            super::BindingId::new(u16::try_from(index).expect("validated model slot range"))
        };
        let mut cross_joins = BTreeSet::new();
        for left in 0..self.model_slots.len() {
            for right in left + 1..self.model_slots.len() {
                cross_joins.insert(super::BindingPair::new(dense(left), dense(right)));
            }
        }
        let bindings = self
            .model_slots
            .iter()
            .zip(&declared)
            .enumerate()
            .map(|(index, ((_, descriptor), declared))| MatchBinding {
                id: dense(index),
                descriptor: descriptor.clone(),
                thing_kind: if declared.kind() == TypeKind::Entity {
                    ThingKind::Entity
                } else {
                    ThingKind::Relation
                },
                match_mode: MatchMode::Subtypes,
            })
            .collect();
        let request = super::validate_match_request_with_limits(
            registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings,
                    predicate: None,
                    allowed_cross_joins: cross_joins,
                },
                MatchOperation::FetchRows {
                    output: FetchShape::Positional {
                        slots: (0..self.model_slots.len())
                            .map(|index| super::FetchSlot::One {
                                binding: dense(index),
                            })
                            .collect(),
                    },
                    order: vec![],
                    window: Window {
                        offset: 0,
                        limit: limits.items.max(1),
                    },
                    cardinality: RowCardinality::Complete,
                },
            ),
            self.selection_plan().limits(),
        )
        .map_err(|_| fail())?;
        let result = super::result_validation::validated_match_result_from_outcome(
            registry,
            &request,
            RemoteOutcomeV2::HydratedRows {
                graph: finished.graph,
                rows,
            },
        )
        .map_err(|_| fail())?;
        let ordinals = tuples
            .into_iter()
            .enumerate()
            .map(|(index, tuple)| (tuple, index))
            .collect::<BTreeMap<_, _>>();
        let model_rows = selected_tuples
            .iter()
            .map(|tuple| ordinals.get(tuple).copied().ok_or_else(fail))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PlanFetchResult {
            selection,
            request,
            result,
            model_bindings: self
                .model_slots
                .iter()
                .map(|(binding, _)| *binding)
                .collect(),
            model_rows,
            #[cfg(test)]
            validation_count: normalized.len(),
        })
    }
}

fn build_single_fetch(
    registry: &DescriptorRegistry,
    plan: &QueryPlanV3,
    model_slots: &[(BindingId, DescriptorId)],
    authority: &MatchRequestAdapterAuthority,
) -> Result<PlanFetchStack, Diagnostic> {
    let failure = || {
        integrity(
            if plan.writes() {
                "plan_returning_models_after_write"
            } else {
                "plan_returning_models_shape"
            },
            "model projection requires one complete model-only terminal fetch",
        )
    };
    let [(binding, descriptor)] = model_slots else {
        return Err(failure());
    };
    if plan.managed_semantics()
        != authority
            .context()
            .managed_state()
            .managed_semantic_schema()
    {
        return Err(failure());
    }
    let document = plan
        .pipeline()
        .iter()
        .find_map(|stage| match stage {
            Stage::Fetch(document) => Some(document),
            _ => None,
        })
        .ok_or_else(failure)?;
    if document.thing != *binding || document.roles.iter().any(|role| role.required) {
        return Err(failure());
    }
    let declared = descriptor_id_type(descriptor.as_str())?;
    let metadata = MatchRequest::v1(
        MatchPlan {
            bindings: vec![MatchBinding {
                id: super::BindingId::new(binding.get()),
                descriptor: descriptor.clone(),
                thing_kind: match declared.kind() {
                    TypeKind::Entity => ThingKind::Entity,
                    TypeKind::Relation => ThingKind::Relation,
                    _ => return Err(failure()),
                },
                match_mode: MatchMode::Subtypes,
            }],
            predicate: None,
            allowed_cross_joins: BTreeSet::new(),
        },
        MatchOperation::FetchRows {
            output: FetchShape::Positional { slots: vec![] },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 1,
            },
            cardinality: RowCardinality::Complete,
        },
    );
    let hydration = build_hydration(&metadata, registry, authority.context().resolved_schema())?;
    let shape = plan.fetch_document_shape().ok_or_else(failure)?;
    if shape.type_labels().next().is_none()
        && plan.binding_types(*binding).is_none_or(|types| {
            types.iter().any(|id| {
                id != &declared && !hydration.bindings()[0].concrete_descriptors().contains(id)
            })
        })
    {
        return Err(failure());
    }
    for label in shape.type_labels() {
        let concrete = hydration.bindings()[0]
            .concrete_descriptors()
            .iter()
            .find(|id| id.label().as_str() == label)
            .ok_or_else(failure)?;
        let descriptor = hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
            .ok_or_else(failure)?;
        if document.roles.len() != descriptor.roles().len()
            || descriptor.roles().iter().any(|role| {
                !document.roles.iter().any(|actual| {
                    actual.role == *role.role() || role.reference_roles().contains(&actual.role)
                })
            })
        {
            return Err(failure());
        }
    }
    Ok(PlanFetchStack {
        plans: vec![plan.clone()],
        selection: None,
        mapping: None,
        partitions: vec![],
        hydration,
        model_slots: model_slots.to_vec(),
        schema_proof: None,
        bulk: None,
    })
}

pub(crate) struct FetchPrefix<'a> {
    pub(crate) source: &'a QueryPlanV3,
    pub(crate) stages: &'a [Stage],
    pub(crate) rematch: &'a [Pattern],
    pub(crate) baseline: Option<BindingId>,
}
fn unique_partition_iids(
    source: &QueryPlanV3,
    pipeline: &mut Vec<Stage>,
    binding: BindingId,
) -> Result<Vec<AssertionBinding>, Diagnostic> {
    use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};
    let exhausted = || {
        integrity(
            "plan_returning_models_binding_limit",
            "partition IID grouping exceeds the native binding ceiling",
        )
    };
    if source.bindings().len() >= source.limits().structural.bindings {
        return Err(exhausted());
    }
    let count = u16::try_from(source.bindings().len())
        .ok()
        .and_then(|id| BindingId::new(id).ok())
        .ok_or_else(exhausted)?;
    let mut name = "model_partition_count".to_owned();
    while source
        .bindings()
        .iter()
        .any(|binding| binding.variable().as_str() == name)
        || source
            .inputs()
            .iter()
            .any(|input| input.public_name().as_str() == name)
    {
        name.push('x');
    }
    let mut bindings = source.bindings().to_vec();
    bindings.push(AssertionBinding::new(count, QueryVariable::new(name)?));
    // shortcut: TypeDB 3.12.3/3.13.6/3.14.0 ignore a second Distinct before Fetch with given windows; see second_distinct_given_window_provider_quirk. Replace with Select+Distinct when fixed.
    pipeline.push(Stage::Reduce {
        assignments: vec![ReduceAssignment::new(count, Reducer::Count, None)],
        groups: vec![binding],
    });
    Ok(bindings)
}

fn same_hydration_fields(
    left: &type_bridge_contract::query_plan::HydrationDescriptorV2,
    right: &type_bridge_contract::query_plan::HydrationDescriptorV2,
) -> bool {
    left.fields().len() == right.fields().len()
        && left
            .fields()
            .iter()
            .zip(right.fields())
            .all(|(left, right)| {
                left.alias() == right.alias()
                    && left.attribute() == right.attribute()
                    && left.value_type() == right.value_type()
                    && left.cardinality() == right.cardinality()
                    && left.ordered() == right.ordered()
                    && left.distinct() == right.distinct()
                    && left.unique() == right.unique()
            })
}

pub(crate) fn append_hydration_partitions(
    prefix: FetchPrefix<'_>,
    bindings: &[type_bridge_contract::query_plan::HydrationBindingV2],
    hydration: &HydrationProjectionV2,
    authority: &MatchRequestAdapterAuthority,
    plans: &mut Vec<QueryPlanV3>,
) -> Result<Vec<(usize, BindingId, Vec<TypeId>)>, Diagnostic> {
    let mut partitions = Vec::new();
    for binding in bindings {
        let mut groups: Vec<(
            Vec<TypeId>,
            &type_bridge_contract::query_plan::HydrationDescriptorV2,
            Vec<type_bridge_contract::query_plan::v3::DocumentRole>,
        )> = Vec::new();
        for concrete in binding.concrete_descriptors() {
            let descriptor = hydration
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.descriptor() == concrete)
                .ok_or_else(|| {
                    integrity(
                        "plan_returning_models_descriptor",
                        "fetch partition has no descriptor authority",
                    )
                })?;
            let roles = super::descriptor_plan::document_roles(descriptor, authority)?;
            if let Some((types, _, _)) = groups.iter_mut().find(|(_, other, listed)| {
                listed.as_slice() == roles.as_slice() && same_hydration_fields(other, descriptor)
            }) {
                types.push(concrete.clone());
            } else {
                groups.push((vec![concrete.clone()], descriptor, roles));
            }
        }
        for (concretes, _, roles) in groups {
            let mut pipeline = prefix.stages.to_vec();
            if !pipeline
                .iter()
                .any(|stage| matches!(stage, Stage::Distinct))
            {
                pipeline.push(Stage::Distinct);
            }
            let mut patterns = if prefix.baseline == Some(binding.binding()) {
                vec![]
            } else {
                prefix.rematch.to_vec()
            };
            let mut types = concretes
                .iter()
                .map(|concrete| QueryPattern::Isa {
                    binding: binding.binding(),
                    type_id: concrete.clone(),
                    include_subtypes: false,
                })
                .collect::<Vec<_>>();
            patterns.push(
                if types.len() == 1 {
                    types.pop().unwrap()
                } else {
                    QueryPattern::Or {
                        branches: types.into_iter().map(|pattern| vec![pattern]).collect(),
                    }
                }
                .into(),
            );
            pipeline.push(Stage::Match { patterns });
            let bindings = if prefix.baseline != Some(binding.binding()) {
                unique_partition_iids(prefix.source, &mut pipeline, binding.binding())?
            } else {
                prefix.source.bindings().to_vec()
            };
            pipeline.push(Stage::Fetch(ThingDocument::new(binding.binding(), roles)));
            partitions.push((plans.len(), binding.binding(), concretes));
            plans.push(rebuild_with_bindings(
                prefix.source,
                bindings,
                pipeline,
                vec![],
                authority,
            )?);
        }
    }
    Ok(partitions)
}

fn budget_failure(
    error: crate::OrmError,
    limits: crate::QueryExecutionResourceLimits,
) -> Diagnostic {
    use type_bridge_contract::diagnostic::{DiagnosticCategory, DiagnosticCode};
    let crate::OrmError::Match(error) = error else {
        return integrity("query_plan_result_state", "plan answer metering failed");
    };
    let category = match error.category() {
        super::MatchErrorCategory::ResourceLimit => DiagnosticCategory::ResourceLimit,
        super::MatchErrorCategory::Cancelled => DiagnosticCategory::Cancelled,
        _ => DiagnosticCategory::Integrity,
    };
    let mut diagnostic = Diagnostic::new(
        category,
        DiagnosticCode::new(error.code().as_str()).expect("internal meter diagnostic code"),
        error.message(),
    );
    if let Some((limit, maximum, source)) = limits.refusal_details(
        crate::query_execution_limits::AnswerTransport::Direct,
        error.code().as_str(),
    ) {
        diagnostic = diagnostic
            .with_detail("limit", limit)
            .with_detail("maximum", i64::try_from(maximum).unwrap_or(i64::MAX))
            .with_detail("source", source);
    }
    diagnostic
}
