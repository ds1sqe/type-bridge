//! Same-snapshot execution of schema-validated V1 compatibility models.
//!
//! This is deliberately not a second semantic lowerer. The adapter-authored
//! compatibility tree is lowered once by `query_v2_compatibility`; this module
//! executes only that closed typed provider plan and turns complete provider
//! evidence into the contract-owned hydration graph.

use crate::match_request::plan_hydration::*;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use serde::Deserialize;
use serde_json::Value as JsonValue;
use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::id::{
    MAX_THING_IID_HEX_DIGITS, TypeId, TypeKind, is_canonical_thing_iid,
};
use type_bridge_contract::limits::MAX_SUPPORTED_BINDINGS;
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    HydrationDescriptorV2, HydrationProjectionV2, ModelQueryV2, QueryFieldV2, QueryInvocation,
    QueryModelOutputSlotV2, QueryModelOutputV2, QueryReductionGroupV2, QueryReductionKindV2,
    QueryReductionTermV2, QueryRowCardinalityV2, QueryStableOrderV2,
};
use type_bridge_contract::query_remote_v2::{
    HydrationGraphV2, HydrationNodeKindV2, RemoteOutcomeV2, RemoteReducedValueV2,
    RemoteReductionGroupV2, RemoteReductionRowV2, RemoteReplyDecodeLimitsV2, RemoteResultKindV2,
    validate_remote_outcome_v2,
};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedHydrationDescriptor, TypedHydrationField,
    TypedHydrationRole, TypedHydrationTarget, TypedPageRematch, TypedRootScan, TypedThingKind,
};
use type_bridge_query::ValidatedQuery;

use crate::error::PlanExecutionError;
use crate::match_request::descriptor_plan::adapt_value;
use crate::match_request::reducer::reduce_rows;
use crate::match_request::reducer::{
    LoweredReduceGroup, LoweredReduceInput, LoweredReduceTerm, ReduceDomain,
};
use crate::match_request::result::{
    BoundConceptEvidence, ConceptId, HydratedAttribute, HydratedThing, ProviderSolutionEvidence,
    ReducedValue, ReductionRow,
};
use crate::match_request::result_validation::released_attribute_value;
use crate::match_request::{
    BindingId as MatchBindingId, BoundFieldId, DescriptorId, FieldId, Reduction, ThingKind,
};
use crate::plan_lowering::failure;
use crate::query_v2::preflight_invocation_transport;
use crate::query_v2_compatibility::{
    CompatibilityProviderPlan, lower_validated_compatibility_query,
};
use crate::session::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, AnswerLimits, BoundedAnswerLimits,
    BoundedAnswerStats, MAX_ERROR_DRAIN_BYTES, MAX_ERROR_DRAIN_ITEMS,
};
use crate::session::transaction::OwnedTransaction as Transaction;
use crate::value::AttributeValue;

type ExecResult<T> = Result<T, PlanExecutionError>;
type HydrationBatchPlan = (Vec<TypedHydrateThings>, BTreeSet<(u16, String)>);
const MAX_MODEL_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const TUPLE_PROOF_ROWS: u64 = 2;
const TUPLE_PROOF_ROW_ENVELOPE_BYTES: u64 = 64;
const TUPLE_PROOF_BINDING_FIXED_BYTES: u64 = 192 + MAX_THING_IID_HEX_DIGITS as u64;

type ModelExecutionTarget<'target> = &'target mut Transaction;

/// Execute one adapter-authored model plan inside the caller's existing
/// schema-fenced transaction.
pub(crate) async fn execute_validated_model_query_with_statement_limit(
    transaction: &mut Transaction,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: AnswerLimits,
    reply_limits: RemoteReplyDecodeLimitsV2,
    max_statements: u8,
) -> ExecResult<RemoteOutcomeV2> {
    execute_validated_model_query_in_target(
        transaction,
        validated,
        invocation,
        limits,
        reply_limits,
        max_statements,
    )
    .await
}

async fn execute_validated_model_query_in_target(
    mut transaction: ModelExecutionTarget<'_>,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: AnswerLimits,
    reply_limits: RemoteReplyDecodeLimitsV2,
    max_statements: u8,
) -> ExecResult<RemoteOutcomeV2> {
    preflight_invocation_transport(validated.plan(), invocation)
        .map_err(PlanExecutionError::Validation)?;
    let model = validated
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.model_query())
        .ok_or_else(|| {
            validation_error(
                "query_v2_model_contract_missing",
                "model execution requires an adapter-authored compatibility terminal",
            )
        })?;
    let lowered =
        lower_validated_compatibility_query(validated, invocation, invocation.operation())
            .map_err(PlanExecutionError::Validation)?
            .ok_or_else(|| {
                validation_error(
                    "query_v2_model_provider_plan_missing",
                    "validated model execution lacks its sole typed provider plan",
                )
            })?;
    let provider_plan = lowered.provider_plan().clone();
    let mut budget = ExecutionBudget::new(limits, reply_limits, max_statements);
    let (outcome, expected) = match (model, provider_plan) {
        (
            ModelQueryV2::Rows {
                cardinality,
                hydration,
                order,
                output,
                window,
            },
            CompatibilityProviderPlan::Rows {
                statement,
                tuple_proof,
            },
        ) => (
            execute_rows(
                &mut transaction,
                validated,
                *cardinality,
                hydration,
                order.as_ref(),
                output,
                window.offset(),
                window.limit(),
                statement,
                tuple_proof,
                &mut budget,
                reply_limits,
            )
            .await?,
            RemoteResultKindV2::HydratedRows,
        ),
        (
            ModelQueryV2::Page {
                hydration,
                include_total,
                output,
                root,
                window,
                ..
            },
            CompatibilityProviderPlan::Page {
                selection,
                total,
                rematch,
            },
        ) => (
            execute_page(
                &mut transaction,
                validated,
                hydration,
                output,
                *root,
                window.offset(),
                window.limit(),
                *include_total,
                selection,
                total,
                rematch,
                &mut budget,
                reply_limits,
            )
            .await?,
            RemoteResultKindV2::HydratedPage,
        ),
        (
            ModelQueryV2::DistinctCount { root, .. },
            CompatibilityProviderPlan::DistinctCount { scan },
        ) => (
            RemoteOutcomeV2::DistinctCount {
                root: *root,
                value: execute_count(&mut transaction, *root, scan, &mut budget).await?,
            },
            RemoteResultKindV2::DistinctCount,
        ),
        (
            ModelQueryV2::DistinctExists { root, .. },
            CompatibilityProviderPlan::DistinctExists { scan },
        ) => (
            RemoteOutcomeV2::DistinctExists {
                root: *root,
                value: execute_exists(&mut transaction, *root, scan, &mut budget).await?,
            },
            RemoteResultKindV2::DistinctExists,
        ),
        (
            ModelQueryV2::Reduction {
                hydration,
                root,
                group,
                reducers,
            },
            CompatibilityProviderPlan::Reduction { scan, rematch },
        ) => (
            execute_reduction(
                &mut transaction,
                validated,
                hydration,
                *root,
                group.as_ref(),
                reducers,
                scan,
                rematch,
                &mut budget,
                reply_limits,
            )
            .await?,
            RemoteResultKindV2::ModelReduction,
        ),
        _ => {
            return Err(validation_error(
                "query_v2_model_provider_plan_mismatch",
                "typed provider plan contradicts the schema-validated model terminal",
            ));
        }
    };
    validate_remote_outcome_v2(&outcome, expected, reply_limits, validated.plan())
        .map_err(PlanExecutionError::Validation)?;
    Ok(outcome)
}

struct ExecutionBudget {
    remaining_items: u64,
    remaining_bytes: u64,
    remaining_collection_members: u64,
    remaining_statements: u8,
    deadline: Option<std::time::Instant>,
    cancellation: crate::session::backend::AnswerCancellation,
}

impl ExecutionBudget {
    fn new(
        limits: AnswerLimits,
        reply_limits: RemoteReplyDecodeLimitsV2,
        max_statements: u8,
    ) -> Self {
        Self {
            remaining_items: limits.answer.max_items,
            remaining_bytes: limits.answer.max_bytes,
            remaining_collection_members: limits
                .max_collection_members
                .min(reply_limits.max_collection_members),
            remaining_statements: max_statements,
            deadline: limits.answer.deadline,
            cancellation: limits.answer.cancellation,
        }
    }

    fn begin_statement(&mut self) -> ExecResult<()> {
        self.remaining_statements = self.remaining_statements.checked_sub(1).ok_or_else(|| {
            resource_error(
                "statement_count_limit",
                "match execution exceeded its statement ceiling",
            )
        })?;
        Ok(())
    }

    fn limits(&self, requested_items: u64) -> BoundedAnswerLimits {
        BoundedAnswerLimits {
            max_items: requested_items.min(self.remaining_items),
            max_bytes: self.remaining_bytes,
            deadline: self.deadline,
            cancellation: self.cancellation.clone(),
        }
    }

    fn charge(&mut self, stats: BoundedAnswerStats) -> ExecResult<()> {
        self.remaining_items = self
            .remaining_items
            .checked_sub(stats.processed_items)
            .ok_or_else(|| {
                resource_error(
                    "query_v2_model_item_limit",
                    "model execution exceeded its aggregate provider-item budget",
                )
            })?;
        self.remaining_bytes = self
            .remaining_bytes
            .checked_sub(stats.response_bytes)
            .ok_or_else(|| {
                resource_error(
                    "query_v2_model_byte_limit",
                    "model execution exceeded its aggregate provider-byte budget",
                )
            })?;
        Ok(())
    }

    fn charge_collection(&mut self, count: usize) -> ExecResult<()> {
        let count = u64::try_from(count).map_err(|_| {
            resource_error(
                "query_v2_model_collection_limit",
                "model collection length exceeds the execution counter range",
            )
        })?;
        self.remaining_collection_members = self
            .remaining_collection_members
            .checked_sub(count)
            .ok_or_else(|| {
                resource_error(
                    "query_v2_model_collection_limit",
                    "model execution exceeded its aggregate collection-member budget",
                )
            })?;
        Ok(())
    }

    const fn remaining_items(&self) -> u64 {
        self.remaining_items
    }

    fn check_before_await(&self) -> ExecResult<()> {
        if self.cancellation.is_cancelled() {
            return Err(cancelled_error(
                "provider_cancelled",
                "provider answer processing was cancelled",
            ));
        }
        if self.deadline.is_some_and(|deadline| {
            tokio::time::Instant::now() >= tokio::time::Instant::from_std(deadline)
        }) {
            return Err(resource_error(
                "transaction_deadline_exceeded",
                "provider transaction deadline expired",
            ));
        }
        Ok(())
    }

    async fn await_provider<T>(
        &self,
        future: impl Future<Output = Result<T, crate::error::OrmError>>,
    ) -> ExecResult<T> {
        self.check_before_await()?;
        tokio::pin!(future);
        let cancellation = self.cancellation.cancelled();
        tokio::pin!(cancellation);
        let result = if let Some(deadline) = self.deadline {
            let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
            tokio::pin!(deadline);
            tokio::select! {
                biased;
                result = &mut future => result.map_err(PlanExecutionError::Provider),
                () = &mut cancellation => Err(cancelled_error(
                    "provider_cancelled",
                    "provider answer processing was cancelled",
                )),
                () = &mut deadline => Err(resource_error(
                    "transaction_deadline_exceeded",
                    "provider transaction deadline expired",
                )),
            }
        } else {
            tokio::select! {
                biased;
                result = &mut future => result.map_err(PlanExecutionError::Provider),
                () = &mut cancellation => Err(cancelled_error(
                    "provider_cancelled",
                    "provider answer processing was cancelled",
                )),
            }
        };
        self.check_before_await()?;
        result
    }
}

trait ModelConsumer: Send {
    fn accept_model(&mut self, item: AnswerItem) -> ExecResult<AnswerControl>;
}

/// Locally account for every provider item even when an extension backend
/// under-reports its own counters. The first typed-evidence failure is sticky,
/// and only a small finite suffix is drained before the statement is stopped.
struct ModelDrainingConsumer<'a> {
    inner: &'a mut dyn ModelConsumer,
    max_items: u64,
    max_bytes: u64,
    processed_items: u64,
    response_bytes: u64,
    drained_items: u64,
    drained_bytes: u64,
    first_error: Option<PlanExecutionError>,
    consumer_stopped: bool,
}

impl<'a> ModelDrainingConsumer<'a> {
    fn new(inner: &'a mut dyn ModelConsumer, limits: &BoundedAnswerLimits) -> Self {
        Self {
            inner,
            max_items: limits.max_items,
            max_bytes: limits.max_bytes,
            processed_items: 0,
            response_bytes: 0,
            drained_items: 0,
            drained_bytes: 0,
            first_error: None,
            consumer_stopped: false,
        }
    }

    fn complete(self, provider: ExecResult<BoundedAnswerStats>) -> ExecResult<BoundedAnswerStats> {
        if let Some(error) = self.first_error {
            return Err(error);
        }
        let mut stats = provider?;
        stats.processed_items = stats.processed_items.max(self.processed_items);
        stats.response_bytes = stats.response_bytes.max(self.response_bytes);
        stats.stopped_early |= self.consumer_stopped;
        Ok(stats)
    }

    fn reject(&mut self, error: PlanExecutionError) -> AnswerControl {
        if self.first_error.is_none() {
            self.first_error = Some(error);
        }
        if self.has_drain_capacity() {
            AnswerControl::Continue
        } else {
            self.consumer_stopped = true;
            AnswerControl::Stop
        }
    }

    fn has_drain_capacity(&self) -> bool {
        self.processed_items < self.max_items
            && self.response_bytes < self.max_bytes
            && self.drained_items < MAX_ERROR_DRAIN_ITEMS
            && self.drained_bytes < MAX_ERROR_DRAIN_BYTES
    }

    fn accept_suffix(&mut self, item: AnswerItem) -> AnswerControl {
        let Ok(item_bytes) = item.encoded_bytes() else {
            self.consumer_stopped = true;
            return AnswerControl::Stop;
        };
        let Some(next_items) = self.processed_items.checked_add(1) else {
            self.consumer_stopped = true;
            return AnswerControl::Stop;
        };
        let Some(next_bytes) = self.response_bytes.checked_add(item_bytes) else {
            self.consumer_stopped = true;
            return AnswerControl::Stop;
        };
        let Some(next_drained_items) = self.drained_items.checked_add(1) else {
            self.consumer_stopped = true;
            return AnswerControl::Stop;
        };
        let Some(next_drained_bytes) = self.drained_bytes.checked_add(item_bytes) else {
            self.consumer_stopped = true;
            return AnswerControl::Stop;
        };
        if next_items > self.max_items
            || next_bytes > self.max_bytes
            || next_drained_items > MAX_ERROR_DRAIN_ITEMS
            || next_drained_bytes > MAX_ERROR_DRAIN_BYTES
        {
            self.consumer_stopped = true;
            return AnswerControl::Stop;
        }
        self.processed_items = next_items;
        self.response_bytes = next_bytes;
        self.drained_items = next_drained_items;
        self.drained_bytes = next_drained_bytes;
        if self.has_drain_capacity() {
            AnswerControl::Continue
        } else {
            self.consumer_stopped = true;
            AnswerControl::Stop
        }
    }
}

impl AnswerConsumer for ModelDrainingConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, crate::error::OrmError> {
        if self.first_error.is_some() {
            return Ok(self.accept_suffix(item));
        }
        let next_items = match self.processed_items.checked_add(1) {
            Some(next_items) => next_items,
            None => {
                return Ok(self.reject(resource_error(
                    "processed_item_counter_overflow",
                    "processed provider item counter overflowed",
                )));
            }
        };
        if next_items > self.max_items {
            return Ok(self.reject(resource_error(
                "processed_item_limit",
                "provider answer exceeded the processed-item ceiling",
            )));
        }
        let item_bytes = match item.encoded_bytes() {
            Ok(item_bytes) => item_bytes,
            Err(_) => {
                return Ok(self.reject(resource_error(
                    "answer_byte_counter_overflow",
                    "encoded provider answer length exceeds the counter range",
                )));
            }
        };
        let next_bytes = match self.response_bytes.checked_add(item_bytes) {
            Some(next_bytes) => next_bytes,
            None => {
                return Ok(self.reject(resource_error(
                    "answer_byte_counter_overflow",
                    "provider answer byte counter overflowed",
                )));
            }
        };
        if next_bytes > self.max_bytes {
            return Ok(self.reject(resource_error(
                "response_byte_limit",
                "provider answer exceeded the response-byte ceiling",
            )));
        }
        self.processed_items = next_items;
        self.response_bytes = next_bytes;
        match self.inner.accept_model(item) {
            Ok(AnswerControl::Continue) => Ok(AnswerControl::Continue),
            Ok(AnswerControl::Stop) => {
                self.consumer_stopped = true;
                Ok(AnswerControl::Stop)
            }
            Err(error) => Ok(self.reject(error)),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolutionWire {
    bindings: Vec<SolutionBindingWire>,
    #[serde(default)]
    satisfied_role_edges: Vec<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SolutionBindingWire {
    binding: u16,
    concept_id: String,
}

struct SolutionConsumer {
    expected: BTreeSet<u16>,
    selected: Vec<u16>,
    retain: u64,
    seen: BTreeSet<Vec<String>>,
    solutions: Vec<Solution>,
    stop_when_complete: bool,
}

impl SolutionConsumer {
    fn new(statement: &TypedFetchRows, retain: u64, stop_when_complete: bool) -> Self {
        Self {
            expected: statement
                .targets
                .iter()
                .map(|target| target.binding)
                .collect(),
            selected: statement.projection.clone(),
            retain,
            seen: BTreeSet::new(),
            solutions: Vec::new(),
            stop_when_complete,
        }
    }

    fn target_count(&self) -> u64 {
        self.retain
    }

    fn complete(&self) -> bool {
        u64::try_from(self.seen.len()).unwrap_or(u64::MAX) >= self.target_count()
    }

    fn finish(self) -> Vec<Solution> {
        self.solutions
    }

    fn reject(&self, code: &'static str, message: &'static str) -> ExecResult<AnswerControl> {
        Err(evidence_error(code, message))
    }
}

impl ModelConsumer for SolutionConsumer {
    fn accept_model(&mut self, item: AnswerItem) -> ExecResult<AnswerControl> {
        let AnswerItem::Row(value) = item else {
            return self.reject(
                "query_v2_model_solution_kind",
                "model solution scan returned a document instead of a row",
            );
        };
        let wire: SolutionWire = match serde_json::from_value(value) {
            Ok(wire) => wire,
            Err(_) => {
                return self.reject(
                    "query_v2_model_solution_malformed",
                    "model solution row does not match the closed typed evidence shape",
                );
            }
        };
        let mut bindings = BTreeMap::new();
        for assignment in wire.bindings {
            if !self.expected.contains(&assignment.binding) {
                return self.reject(
                    "query_v2_model_solution_unknown_binding",
                    "model solution contains an unknown binding",
                );
            }
            if !is_canonical_thing_iid(&assignment.concept_id) {
                return self.reject(
                    "query_v2_model_solution_malformed_iid",
                    "model solution contains a malformed provider IID",
                );
            }
            if bindings
                .insert(assignment.binding, assignment.concept_id)
                .is_some()
            {
                return self.reject(
                    "query_v2_model_solution_duplicate_binding",
                    "model solution assigns one binding more than once",
                );
            }
        }
        if bindings.len() != self.expected.len() {
            return self.reject(
                "query_v2_model_solution_binding",
                "model solution omits a positive binding",
            );
        }
        if wire
            .satisfied_role_edges
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != wire.satisfied_role_edges.len()
        {
            return self.reject(
                "query_v2_model_role_edge_evidence",
                "model solution repeats a role-edge claim",
            );
        }
        let identity = self
            .selected
            .iter()
            .map(|binding| bindings[binding].clone())
            .collect::<Vec<_>>();
        if self.seen.insert(identity) {
            let ordinal = u64::try_from(self.seen.len()).unwrap_or(u64::MAX);
            if ordinal <= self.target_count() {
                self.solutions.push(Solution { bindings });
            }
        }
        if self.stop_when_complete && self.complete() {
            Ok(AnswerControl::Stop)
        } else {
            Ok(AnswerControl::Continue)
        }
    }
}

struct RootConsumer {
    root: u16,
    roots: Vec<String>,
    seen: BTreeSet<String>,
}

impl RootConsumer {
    fn new(root: BindingId) -> Self {
        Self {
            root: root.get(),
            roots: Vec::new(),
            seen: BTreeSet::new(),
        }
    }

    fn finish(self) -> Vec<String> {
        self.roots
    }

    fn reject(&self, code: &'static str, message: &'static str) -> ExecResult<AnswerControl> {
        Err(evidence_error(code, message))
    }
}

impl ModelConsumer for RootConsumer {
    fn accept_model(&mut self, item: AnswerItem) -> ExecResult<AnswerControl> {
        let AnswerItem::Row(value) = item else {
            return self.reject(
                "query_v2_model_root_kind",
                "model root scan returned a document instead of a row",
            );
        };
        let wire: SolutionWire = match serde_json::from_value(value) {
            Ok(wire) => wire,
            Err(_) => {
                return self.reject(
                    "query_v2_model_root_malformed",
                    "model root row does not match the closed typed evidence shape",
                );
            }
        };
        if !wire.satisfied_role_edges.is_empty() || wire.bindings.len() != 1 {
            return self.reject(
                "query_v2_model_root_malformed",
                "model root row must contain exactly one unclaimed root binding",
            );
        }
        let assignment = &wire.bindings[0];
        if assignment.binding != self.root || !is_canonical_thing_iid(&assignment.concept_id) {
            return self.reject(
                "query_v2_model_root_binding",
                "model root row contains the wrong binding or a malformed provider IID",
            );
        }
        if self.seen.insert(assignment.concept_id.clone()) {
            self.roots.push(assignment.concept_id.clone());
        }
        Ok(AnswerControl::Continue)
    }
}

async fn execute_root_scan(
    transaction: &mut ModelExecutionTarget<'_>,
    root: BindingId,
    mut scan: TypedRootScan,
    budget: &mut ExecutionBudget,
    requested_items: u64,
) -> ExecResult<(Vec<String>, BoundedAnswerStats)> {
    scan.limit = Some(requested_items.max(1));
    let mut consumer = RootConsumer::new(root);
    budget.begin_statement()?;
    let limits = budget.limits(requested_items);
    let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
    let stats = budget
        .await_provider(transaction.query_root_typed_bounded(&scan, limits, &mut draining))
        .await;
    let stats = draining.complete(stats)?;
    budget.charge(stats)?;
    require_exhausted(stats)?;
    Ok((consumer.finish(), stats))
}

async fn execute_count(
    transaction: &mut ModelExecutionTarget<'_>,
    root: BindingId,
    scan: TypedRootScan,
    budget: &mut ExecutionBudget,
) -> ExecResult<u64> {
    let scan_limit = budget.remaining_items();
    let (roots, stats) = execute_root_scan(transaction, root, scan, budget, scan_limit).await?;
    if stats.processed_items >= scan_limit {
        return Err(resource_error(
            "query_v2_model_solution_limit",
            "provider solution ceiling was reached before result completeness was proven",
        ));
    }
    u64::try_from(roots.len()).map_err(|_| {
        resource_error(
            "query_v2_model_count_overflow",
            "distinct-root count exceeds the result counter range",
        )
    })
}

async fn execute_exists(
    transaction: &mut ModelExecutionTarget<'_>,
    root: BindingId,
    scan: TypedRootScan,
    budget: &mut ExecutionBudget,
) -> ExecResult<bool> {
    let (roots, _) = execute_root_scan(transaction, root, scan, budget, 1).await?;
    Ok(!roots.is_empty())
}

fn require_exhausted(stats: BoundedAnswerStats) -> ExecResult<()> {
    if stats.stopped_early {
        Err(validation_error(
            "query_v2_model_provider_not_exhausted",
            "typed model statement stopped before its finite terminal frame",
        ))
    } else {
        Ok(())
    }
}

fn provider_projection_is_public(statement: &TypedFetchRows) -> bool {
    let targets = statement
        .targets
        .iter()
        .map(|target| target.binding)
        .collect::<BTreeSet<_>>();
    let projection = statement
        .projection
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    statement.distinct
        && targets.len() == statement.targets.len()
        && projection.len() == statement.projection.len()
        && targets == projection
        && statement.order.iter().all(|order| {
            let mut fields = statement
                .fields
                .iter()
                .filter(|field| field.id == order.field);
            fields
                .next()
                .is_some_and(|field| projection.contains(&field.owner))
                && fields.next().is_none()
        })
}

fn exactly_one_proof_byte_limit(
    authority: &HydrationAuthority<'_>,
    selection: &TypedFetchRows,
) -> ExecResult<u64> {
    let projected_bindings = selection.projection.len();
    if projected_bindings == 0 || projected_bindings > MAX_SUPPORTED_BINDINGS {
        return Err(evidence_error(
            "query_v2_model_tuple_evidence",
            "distinct-tuple proof projection exceeds the validated binding ceiling",
        ));
    }
    let projected = selection
        .projection
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if projected.len() != projected_bindings {
        return Err(evidence_error(
            "query_v2_model_tuple_evidence",
            "distinct-tuple proof projection repeats a selected binding",
        ));
    }
    let mut targets = BTreeMap::new();
    for target in &selection.targets {
        if projected.contains(&target.binding) && targets.insert(target.binding, target).is_some() {
            return Err(evidence_error(
                "query_v2_model_tuple_evidence",
                "distinct-tuple proof contains duplicate projected targets",
            ));
        }
    }
    if targets.len() != projected_bindings {
        return Err(evidence_error(
            "query_v2_model_tuple_evidence",
            "distinct-tuple proof projection is missing a typed target",
        ));
    }
    let max_label_bytes = targets
        .values()
        .map(|target| {
            if target.exact {
                return Ok(target.type_name.len());
            }
            authority
                .binding(target.binding)?
                .concrete_descriptors()
                .iter()
                .map(|descriptor| descriptor.label().as_str().len())
                .max()
                .ok_or_else(|| {
                    evidence_error(
                        "query_v2_model_tuple_evidence",
                        "tuple-proof target has an empty concrete descriptor closure",
                    )
                })
        })
        .collect::<ExecResult<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    let bindings = u64::try_from(projected_bindings).map_err(|_| {
        resource_error(
            "answer_byte_counter_overflow",
            "distinct-tuple proof binding count exceeds the counter range",
        )
    })?;
    let max_label_bytes = u64::try_from(max_label_bytes).map_err(|_| {
        resource_error(
            "answer_byte_counter_overflow",
            "distinct-tuple proof label length exceeds the counter range",
        )
    })?;
    let binding_bytes = TUPLE_PROOF_BINDING_FIXED_BYTES
        .checked_add(max_label_bytes)
        .ok_or_else(|| {
            resource_error(
                "answer_byte_counter_overflow",
                "distinct-tuple proof byte ceiling overflowed",
            )
        })?;
    TUPLE_PROOF_ROW_ENVELOPE_BYTES
        .checked_add(bindings.checked_mul(binding_bytes).ok_or_else(|| {
            resource_error(
                "answer_byte_counter_overflow",
                "distinct-tuple proof byte ceiling overflowed",
            )
        })?)
        .and_then(|row| row.checked_mul(TUPLE_PROOF_ROWS))
        .map(|limit| limit.min(MAX_MODEL_RESPONSE_BYTES))
        .ok_or_else(|| {
            resource_error(
                "answer_byte_counter_overflow",
                "distinct-tuple proof byte ceiling overflowed",
            )
        })
}

struct TupleConsumer {
    expected: BTreeSet<u16>,
    selected: Vec<u16>,
    identities: BTreeSet<Vec<String>>,
}

impl TupleConsumer {
    fn new(selected: &[u16]) -> Self {
        Self {
            expected: selected.iter().copied().collect(),
            selected: selected.to_vec(),
            identities: BTreeSet::new(),
        }
    }

    fn finish(self) -> usize {
        self.identities.len()
    }

    fn reject(&self, message: &'static str) -> ExecResult<AnswerControl> {
        Err(evidence_error("query_v2_model_tuple_evidence", message))
    }
}

impl ModelConsumer for TupleConsumer {
    fn accept_model(&mut self, item: AnswerItem) -> ExecResult<AnswerControl> {
        let AnswerItem::Row(value) = item else {
            return self.reject("distinct-tuple proof returned a document instead of a row");
        };
        let wire: SolutionWire = match serde_json::from_value(value) {
            Ok(wire) => wire,
            Err(_) => {
                return self.reject("distinct-tuple proof returned malformed evidence");
            }
        };
        if !wire.satisfied_role_edges.is_empty() {
            return self.reject("distinct-tuple proof must not claim role-edge evidence");
        }
        let mut bindings = BTreeMap::new();
        for assignment in wire.bindings {
            if !self.expected.contains(&assignment.binding)
                || !is_canonical_thing_iid(&assignment.concept_id)
                || bindings
                    .insert(assignment.binding, assignment.concept_id)
                    .is_some()
            {
                return self.reject(
                    "distinct-tuple proof has an unknown, duplicate, or malformed binding",
                );
            }
        }
        if bindings.len() != self.expected.len() {
            return self.reject("distinct-tuple proof omits a selected binding");
        }
        self.identities.insert(
            self.selected
                .iter()
                .map(|binding| bindings[binding].clone())
                .collect(),
        );
        Ok(AnswerControl::Continue)
    }
}

struct HydrationConsumer<'a, 'plan> {
    authority: &'a HydrationAuthority<'plan>,
    graph: &'a mut GraphBuilder,
    expected: BTreeSet<(u16, String)>,
    seen: BTreeSet<(u16, String)>,
}

impl<'a, 'plan> HydrationConsumer<'a, 'plan> {
    fn new(
        authority: &'a HydrationAuthority<'plan>,
        graph: &'a mut GraphBuilder,
        expected: BTreeSet<(u16, String)>,
    ) -> Self {
        Self {
            authority,
            graph,
            expected,
            seen: BTreeSet::new(),
        }
    }

    fn finish(self) -> ExecResult<()> {
        if self.seen != self.expected {
            return Err(evidence_error(
                "query_v2_model_hydration_missing",
                "batched hydration omitted a requested binding and provider IID pair",
            ));
        }
        Ok(())
    }
}

impl ModelConsumer for HydrationConsumer<'_, '_> {
    fn accept_model(&mut self, item: AnswerItem) -> ExecResult<AnswerControl> {
        let AnswerItem::Document(value) = item else {
            return Err(evidence_error(
                "query_v2_model_hydration_kind",
                "batched model hydration returned a row instead of a document",
            ));
        };
        let decoded = decode_hydrated_document(value, None, self.authority)?;
        let key = (decoded.binding, decoded.node.iid.clone());
        if !self.expected.contains(&key) || !self.seen.insert(key) {
            return Err(evidence_error(
                "query_v2_model_hydration_unexpected",
                "batched hydration returned an unexpected or duplicate binding and IID pair",
            ));
        }
        for nested in decoded.nested {
            self.graph.insert(nested)?;
        }
        self.graph.insert(decoded.node)?;
        Ok(AnswerControl::Continue)
    }
}

fn typed_hydration_batches(
    authority: &HydrationAuthority<'_>,
    solutions: &[Solution],
) -> ExecResult<HydrationBatchPlan> {
    let mut entity_targets = Vec::new();
    let mut relation_targets = Vec::new();
    let mut expected = BTreeSet::new();
    for binding in authority.projection.bindings() {
        let concept_ids = solutions
            .iter()
            .map(|solution| {
                solution
                    .bindings
                    .get(&binding.binding().get())
                    .cloned()
                    .ok_or_else(|| {
                        evidence_error(
                            "query_v2_model_solution_binding",
                            "retained solution omits a hydration binding",
                        )
                    })
            })
            .collect::<ExecResult<BTreeSet<_>>>()?
            .into_iter()
            .collect::<Vec<_>>();
        for iid in &concept_ids {
            expected.insert((binding.binding().get(), iid.clone()));
        }
        let concrete_descriptors = binding
            .concrete_descriptors()
            .iter()
            .map(|descriptor| {
                authority
                    .descriptor(descriptor)
                    .and_then(typed_hydration_descriptor)
            })
            .collect::<ExecResult<Vec<_>>>()?;
        let target = TypedHydrationTarget {
            binding: binding.binding().get(),
            declared_type: binding.declared_descriptor().label().as_str().to_owned(),
            kind: typed_thing_kind(binding.declared_descriptor().kind())?,
            concept_ids,
            concrete_descriptors,
        };
        match binding.declared_descriptor().kind() {
            TypeKind::Entity => entity_targets.push(target),
            TypeKind::Relation => relation_targets.push(target),
            TypeKind::Attribute | TypeKind::Struct => {
                return Err(validation_error(
                    "query_v2_model_hydration_kind",
                    "model hydration binding has a non-thing descriptor",
                ));
            }
        }
    }
    let batches = [entity_targets, relation_targets]
        .into_iter()
        .filter(|targets| !targets.is_empty())
        .map(|targets| TypedHydrateThings { targets })
        .collect();
    Ok((batches, expected))
}

fn typed_hydration_descriptor(
    descriptor: &HydrationDescriptorV2,
) -> ExecResult<TypedHydrationDescriptor> {
    Ok(TypedHydrationDescriptor {
        type_name: descriptor.descriptor().label().as_str().to_owned(),
        kind: typed_thing_kind(descriptor.descriptor().kind())?,
        fields: descriptor
            .fields()
            .iter()
            .map(|field| TypedHydrationField {
                field_name: field.alias().to_owned(),
                attribute_type: field.attribute().label().as_str().to_owned(),
                value_type: value_type_label(field.value_type()).to_owned(),
            })
            .collect(),
        roles: descriptor
            .roles()
            .iter()
            .map(|role| TypedHydrationRole {
                role_name: role.role().label().as_str().to_owned(),
                player_types: role
                    .players()
                    .iter()
                    .map(|player| player.declared_descriptor().label().as_str().to_owned())
                    .collect(),
            })
            .collect(),
    })
}

fn typed_thing_kind(kind: TypeKind) -> ExecResult<TypedThingKind> {
    match kind {
        TypeKind::Entity => Ok(TypedThingKind::Entity),
        TypeKind::Relation => Ok(TypedThingKind::Relation),
        TypeKind::Attribute | TypeKind::Struct => Err(validation_error(
            "query_v2_model_hydration_kind",
            "typed model hydration contains a non-thing descriptor",
        )),
    }
}

fn hydration_answer_limit(batch: &TypedHydrateThings) -> ExecResult<u64> {
    batch.targets.iter().try_fold(0_u64, |total, target| {
        total
            .checked_add(u64::try_from(target.concept_ids.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| {
                resource_error(
                    "query_v2_model_hydration_limit",
                    "typed hydration identity counter overflowed",
                )
            })
    })
}

async fn hydrate_solutions(
    transaction: &mut ModelExecutionTarget<'_>,
    authority: &HydrationAuthority<'_>,
    solutions: &[Solution],
    graph: &mut GraphBuilder,
    budget: &mut ExecutionBudget,
) -> ExecResult<()> {
    if solutions.is_empty() {
        return Ok(());
    }
    let (batches, expected) = typed_hydration_batches(authority, solutions)?;
    let mut consumer = HydrationConsumer::new(authority, graph, expected);
    for batch in batches {
        let requested = hydration_answer_limit(&batch)?;
        budget.begin_statement()?;
        let limits = budget.limits(requested);
        let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
        let stats = budget
            .await_provider(transaction.hydrate_typed_bounded(&batch, limits, &mut draining))
            .await;
        let stats = draining.complete(stats)?;
        budget.charge(stats)?;
        require_exhausted(stats)?;
    }
    consumer.finish()
}

#[allow(clippy::too_many_arguments)]
async fn execute_rows(
    transaction: &mut ModelExecutionTarget<'_>,
    validated: &ValidatedQuery,
    cardinality: QueryRowCardinalityV2,
    hydration: &HydrationProjectionV2,
    order: Option<&QueryStableOrderV2>,
    output: &QueryModelOutputV2,
    offset: u64,
    limit: u64,
    mut statement: TypedFetchRows,
    tuple_proof: Option<TypedFetchRows>,
    budget: &mut ExecutionBudget,
    reply_limits: RemoteReplyDecodeLimitsV2,
) -> ExecResult<RemoteOutcomeV2> {
    if output.slots().iter().any(|slot| slot.collection()) {
        return Err(validation_error(
            "query_v2_model_rows_collection",
            "released selected-row execution does not admit collection output slots",
        ));
    }
    if cardinality == QueryRowCardinalityV2::BoundedMany && limit == 0 {
        let graph = HydrationGraphV2::new(Vec::new()).map_err(PlanExecutionError::Validation)?;
        return Ok(RemoteOutcomeV2::HydratedRows {
            graph,
            rows: Vec::new(),
        });
    }
    let provider_terminal = provider_projection_is_public(&statement);
    let needs_tuple_proof =
        cardinality.is_exactly_one() && !provider_terminal && tuple_proof.is_some();
    let retain = match cardinality {
        QueryRowCardinalityV2::ExactlyOne => 2,
        QueryRowCardinalityV2::BoundedMany => offset.saturating_add(limit),
        QueryRowCardinalityV2::Complete => limit.saturating_add(1),
    };
    statement.offset = 0;
    statement.limit = if provider_terminal {
        retain.max(1)
    } else {
        budget.remaining_items().max(1)
    };
    budget.begin_statement()?;
    let limits = budget.limits(statement.limit);
    let scan_limit = limits.max_items;
    let mut consumer = SolutionConsumer::new(&statement, retain, !provider_terminal);
    let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
    let stats = budget
        .await_provider(transaction.query_typed_bounded(&statement, limits, &mut draining))
        .await;
    let stats = draining.complete(stats)?;
    budget.charge(stats)?;
    let complete = consumer.complete();
    if !provider_terminal && stats.stopped_early && complete {
        // The released prefix consumer deliberately terminates a wider
        // hidden-witness statement once enough public identities are known.
    } else if (scan_limit < retain || !provider_terminal) && stats.processed_items >= scan_limit {
        return Err(resource_error(
            "query_v2_model_solution_limit",
            "provider solution ceiling was reached before selected-row completeness was proven",
        ));
    } else {
        require_exhausted(stats)?;
    }
    let solutions = consumer.finish();
    if cardinality == QueryRowCardinalityV2::Complete
        && u64::try_from(solutions.len()).unwrap_or(u64::MAX) > limit
    {
        return Err(resource_error(
            "query_v2_model_complete_rows_exceeded",
            "more distinct rows matched than the complete fetch accepts",
        ));
    }

    let authority = HydrationAuthority::new(hydration)?;
    let mut graph = GraphBuilder::new(reply_limits);
    hydrate_solutions(transaction, &authority, &solutions, &mut graph, budget).await?;
    for solution in &solutions {
        validate_solution_predicate(validated, solution, &graph)?;
    }
    charge_solution_semantics(&solutions, &graph, reply_limits)?;
    if let Some(order) = order {
        validate_solution_order(&solutions, order, &graph)?;
    }
    if cardinality.is_exactly_one() && solutions.len() != 1 {
        return Err(cardinality_error(solutions.len()));
    }
    if needs_tuple_proof
        && budget
            .await_provider({
                let transaction = &mut **transaction;
                async move { transaction.supports_exactly_one_tuple_proof() }
            })
            .await?
    {
        let proof = tuple_proof.as_ref().ok_or_else(|| {
            validation_error(
                "query_v2_model_tuple_plan",
                "exactly-one compatibility plan lost its tuple proof",
            )
        })?;
        let mut consumer = TupleConsumer::new(&proof.projection);
        let limits = BoundedAnswerLimits {
            max_items: 2,
            max_bytes: exactly_one_proof_byte_limit(&authority, proof)?,
            deadline: budget.deadline,
            cancellation: budget.cancellation.clone(),
        };
        let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
        let stats = budget
            .await_provider(transaction.query_tuple_typed_bounded(proof, limits, &mut draining))
            .await;
        let stats = draining.complete(stats)?;
        require_exhausted(stats)?;
        let actual = consumer.finish();
        if actual != 1 {
            return Err(cardinality_error(actual));
        }
    }

    let selected = if cardinality.is_exactly_one() {
        solutions.as_slice()
    } else {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(solutions.len());
        let count = usize::try_from(limit).unwrap_or(usize::MAX);
        &solutions[start..start.saturating_add(count).min(solutions.len())]
    };
    if u64::try_from(selected.len()).unwrap_or(u64::MAX) > reply_limits.max_items {
        return Err(resource_error(
            "query_v2_model_item_limit",
            "hydrated row count exceeds the caller's result-item budget",
        ));
    }
    let rows = selected
        .iter()
        .map(|solution| {
            output
                .slots()
                .into_iter()
                .map(|slot| {
                    let iid = solution
                        .bindings
                        .get(&slot.binding().get())
                        .cloned()
                        .ok_or_else(|| {
                            evidence_error(
                                "query_v2_model_solution_binding",
                                "selected row omits one public output binding",
                            )
                        })?;
                    Ok(SlotDraft::Singular {
                        declared: slot.declared().clone(),
                        iid,
                    })
                })
                .collect::<ExecResult<Vec<_>>>()
        })
        .collect::<ExecResult<Vec<_>>>()?;
    let (graph, rows) = finish_hydrated_rows(graph, rows)?;
    Ok(RemoteOutcomeV2::HydratedRows { graph, rows })
}

struct PageRematchConsumer<'a, 'plan> {
    authority: &'a HydrationAuthority<'plan>,
    graph: &'a mut GraphBuilder,
    expected_bindings: BTreeSet<u16>,
    selected_roots: BTreeSet<String>,
    seen_roots: BTreeSet<String>,
    root: u16,
    solutions: Vec<Solution>,
}

impl<'a, 'plan> PageRematchConsumer<'a, 'plan> {
    fn new(
        authority: &'a HydrationAuthority<'plan>,
        graph: &'a mut GraphBuilder,
        root: BindingId,
        selected_roots: &[String],
    ) -> Self {
        Self {
            authority,
            graph,
            expected_bindings: authority.bindings.keys().copied().collect(),
            selected_roots: selected_roots.iter().cloned().collect(),
            seen_roots: BTreeSet::new(),
            root: root.get(),
            solutions: Vec::new(),
        }
    }

    fn finish(self) -> ExecResult<Vec<Solution>> {
        if self.seen_roots != self.selected_roots {
            return Err(evidence_error(
                "query_v2_model_page_root_set",
                "page re-match root identities do not equal the selected root set",
            ));
        }
        Ok(self.solutions)
    }

    fn decode_solution(&self, value: JsonValue) -> ExecResult<Vec<DecodedThing>> {
        if let Ok(wire) = serde_json::from_value::<RematchSolutionWire>(value.clone()) {
            if wire
                .satisfied_role_edges
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
                != wire.satisfied_role_edges.len()
            {
                return Err(evidence_error(
                    "query_v2_model_role_edge_evidence",
                    "page re-match repeats a role-edge claim",
                ));
            }
            return wire
                .bindings
                .into_iter()
                .map(|thing| decode_exact_thing(thing, self.authority))
                .collect();
        }
        let object = value.as_object().ok_or_else(|| {
            evidence_error(
                "query_v2_model_page_document",
                "page re-match document is not an object",
            )
        })?;
        if object.len() != self.expected_bindings.len() {
            return Err(evidence_error(
                "query_v2_model_page_document",
                "page re-match document has missing or extra binding members",
            ));
        }
        self.expected_bindings
            .iter()
            .map(|binding| {
                let value = object.get(&format!("b{binding}")).cloned().ok_or_else(|| {
                    evidence_error(
                        "query_v2_model_page_document",
                        "page re-match omits a positive binding member",
                    )
                })?;
                decode_hydrated_document(value, Some(*binding), self.authority)
            })
            .collect()
    }
}

impl ModelConsumer for PageRematchConsumer<'_, '_> {
    fn accept_model(&mut self, item: AnswerItem) -> ExecResult<AnswerControl> {
        let AnswerItem::Document(value) = item else {
            return Err(evidence_error(
                "query_v2_model_page_kind",
                "page re-match returned a row instead of a hydrated document",
            ));
        };
        let decoded = self.decode_solution(value)?;
        let mut bindings = BTreeMap::new();
        for thing in decoded {
            let binding = thing.binding;
            let iid = thing.node.iid.clone();
            if !self.expected_bindings.contains(&binding) || bindings.insert(binding, iid).is_some()
            {
                return Err(evidence_error(
                    "query_v2_model_page_binding",
                    "page re-match contains an unknown or duplicate binding",
                ));
            }
            for nested in thing.nested {
                self.graph.insert(nested)?;
            }
            self.graph.insert(thing.node)?;
        }
        if bindings.len() != self.expected_bindings.len() {
            return Err(evidence_error(
                "query_v2_model_page_binding",
                "page re-match omits a positive binding",
            ));
        }
        let root = bindings[&self.root].clone();
        if !self.selected_roots.contains(&root) {
            return Err(evidence_error(
                "query_v2_model_page_unexpected_root",
                "page re-match returned a root outside the selected root set",
            ));
        }
        self.seen_roots.insert(root);
        self.solutions.push(Solution { bindings });
        Ok(AnswerControl::Continue)
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_page(
    transaction: &mut ModelExecutionTarget<'_>,
    validated: &ValidatedQuery,
    hydration: &HydrationProjectionV2,
    output: &QueryModelOutputV2,
    root: BindingId,
    offset: u64,
    limit: u64,
    include_total: bool,
    selection: TypedRootScan,
    total_scan: Option<TypedRootScan>,
    mut rematch: TypedPageRematch,
    budget: &mut ExecutionBudget,
    reply_limits: RemoteReplyDecodeLimitsV2,
) -> ExecResult<RemoteOutcomeV2> {
    let total = match (include_total, total_scan) {
        (true, Some(scan)) => Some(execute_count(transaction, root, scan, budget).await?),
        (false, None) => None,
        _ => {
            return Err(validation_error(
                "query_v2_model_page_total_plan",
                "typed provider plan contradicts the requested page-total contract",
            ));
        }
    };
    let roots = if limit == 0 {
        Vec::new()
    } else {
        let mut consumer = RootConsumer::new(root);
        budget.begin_statement()?;
        let limits = budget.limits(selection.limit.unwrap_or(limit));
        let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
        let stats = budget
            .await_provider(transaction.query_root_typed_bounded(&selection, limits, &mut draining))
            .await;
        let stats = draining.complete(stats)?;
        budget.charge(stats)?;
        require_exhausted(stats)?;
        consumer.finish()
    };
    if u64::try_from(roots.len()).unwrap_or(u64::MAX) > reply_limits.max_items {
        return Err(resource_error(
            "query_v2_model_item_limit",
            "page root count exceeds the caller's result-item budget",
        ));
    }
    if roots.is_empty() {
        let graph = HydrationGraphV2::new(Vec::new()).map_err(PlanExecutionError::Validation)?;
        return Ok(RemoteOutcomeV2::HydratedPage {
            entries: Vec::new(),
            graph,
            limit,
            offset,
            root,
            total,
        });
    }
    if budget.remaining_items() == 0 {
        return Err(resource_error(
            "query_v2_model_solution_limit",
            "page root selection exhausted the aggregate provider-item budget before re-match",
        ));
    }
    rematch.root_concept_ids.clone_from(&roots);
    let authority = HydrationAuthority::new(hydration)?;
    let mut graph = GraphBuilder::new(reply_limits);
    let scan_limit = budget.remaining_items();
    let mut consumer = PageRematchConsumer::new(&authority, &mut graph, root, &roots);
    budget.begin_statement()?;
    let limits = budget.limits(scan_limit);
    let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
    let stats = budget
        .await_provider(transaction.rematch_page_typed_bounded(&rematch, limits, &mut draining))
        .await;
    let stats = draining.complete(stats)?;
    budget.charge(stats)?;
    require_exhausted(stats)?;
    if stats.processed_items >= scan_limit {
        return Err(resource_error(
            "query_v2_model_solution_limit",
            "provider solution ceiling was reached before page re-match completeness was proven",
        ));
    }
    let solutions = consumer.finish()?;
    for solution in &solutions {
        validate_solution_predicate(validated, solution, &graph)?;
    }
    charge_solution_semantics(&solutions, &graph, reply_limits)?;
    let mut grouped = BTreeMap::<String, Vec<&Solution>>::new();
    for solution in &solutions {
        grouped
            .entry(solution.bindings[&root.get()].clone())
            .or_default()
            .push(solution);
    }
    let mut entries = Vec::with_capacity(roots.len());
    for root_iid in &roots {
        let group = grouped.remove(root_iid).ok_or_else(|| {
            evidence_error(
                "query_v2_model_page_root_set",
                "selected page root has no same-snapshot re-match solution",
            )
        })?;
        let mut slots = Vec::with_capacity(output.slots().len());
        for slot in output.slots() {
            match slot {
                QueryModelOutputSlotV2::One { binding, declared } => {
                    slots.push(SlotDraft::Singular {
                        declared: declared.clone(),
                        iid: group[0].bindings[&binding.get()].clone(),
                    });
                }
                QueryModelOutputSlotV2::Collect {
                    binding,
                    declared,
                    distinct,
                    order,
                } => {
                    let mut iids = group
                        .iter()
                        .map(|solution| solution.bindings[&binding.get()].clone())
                        .collect::<Vec<_>>();
                    if *distinct {
                        let mut seen = BTreeSet::new();
                        iids.retain(|iid| seen.insert(iid.clone()));
                    }
                    try_sort_iids(&mut iids, order, &graph)?;
                    budget.charge_collection(iids.len())?;
                    slots.push(SlotDraft::Collection {
                        declared: declared.clone(),
                        iids,
                    });
                }
            }
        }
        entries.push(slots);
    }
    if let Some(total) = total {
        let expected = limit.min(total.saturating_sub(offset));
        let actual = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        if actual != expected {
            return Err(evidence_error(
                "query_v2_model_page_total_length",
                "provider page length is inconsistent with its same-snapshot total and window",
            ));
        }
    }
    let (graph, entries) = finish_hydrated_rows(graph, entries)?;
    Ok(RemoteOutcomeV2::HydratedPage {
        entries,
        graph,
        limit,
        offset,
        root,
        total,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_reduction(
    transaction: &mut ModelExecutionTarget<'_>,
    validated: &ValidatedQuery,
    hydration: &HydrationProjectionV2,
    root: BindingId,
    group: Option<&QueryReductionGroupV2>,
    reducers: &[QueryReductionTermV2],
    scan: TypedRootScan,
    mut rematch: Option<TypedPageRematch>,
    budget: &mut ExecutionBudget,
    reply_limits: RemoteReplyDecodeLimitsV2,
) -> ExecResult<RemoteOutcomeV2> {
    let scan_limit = budget.remaining_items();
    let (roots, stats) = execute_root_scan(transaction, root, scan, budget, scan_limit).await?;
    if stats.processed_items >= scan_limit {
        return Err(resource_error(
            "query_v2_model_solution_limit",
            "provider solution ceiling was reached before reduction completeness was proven",
        ));
    }
    let authority = HydrationAuthority::new(hydration)?;
    let mut graph = GraphBuilder::new(reply_limits);
    let solutions = if let Some(rematch) = rematch.as_mut() {
        if roots.is_empty() {
            Vec::new()
        } else {
            if budget.remaining_items() == 0 {
                return Err(resource_error(
                    "query_v2_model_solution_limit",
                    "reduction root scan exhausted the provider-item budget before re-match",
                ));
            }
            rematch.root_concept_ids.clone_from(&roots);
            let rematch_limit = budget.remaining_items();
            let mut consumer = PageRematchConsumer::new(&authority, &mut graph, root, &roots);
            budget.begin_statement()?;
            let limits = budget.limits(rematch_limit);
            let mut draining = ModelDrainingConsumer::new(&mut consumer, &limits);
            let stats = budget
                .await_provider(transaction.rematch_page_typed_bounded(
                    rematch,
                    limits,
                    &mut draining,
                ))
                .await;
            let stats = draining.complete(stats)?;
            budget.charge(stats)?;
            require_exhausted(stats)?;
            if stats.processed_items >= rematch_limit {
                return Err(resource_error(
                    "query_v2_model_solution_limit",
                    "provider solution ceiling was reached before reduction re-match completeness was proven",
                ));
            }
            consumer.finish()?
        }
    } else {
        Vec::new()
    };
    for solution in &solutions {
        validate_solution_predicate(validated, solution, &graph)?;
    }
    charge_solution_semantics(&solutions, &graph, reply_limits)?;

    let reduction_fields = reduction_fields(group, reducers);
    let evidence = solutions
        .iter()
        .map(|solution| {
            provider_reduction_solution(solution, &graph, &authority, &reduction_fields)
        })
        .collect::<ExecResult<Vec<_>>>()?;
    let (lowered_group, lowered_terms) = lower_reduction_contract(group, reducers)?;
    let match_root = MatchBindingId::new(root.get());
    let rows = reduce_rows(
        match_root,
        lowered_group.as_ref(),
        &lowered_terms,
        &roots,
        &evidence,
        usize::try_from(reply_limits.max_items).unwrap_or(usize::MAX),
        usize::try_from(budget.remaining_collection_members).unwrap_or(usize::MAX),
    )
    .map_err(PlanExecutionError::Provider)?;
    if u64::try_from(rows.len()).unwrap_or(u64::MAX) > reply_limits.max_items {
        return Err(resource_error(
            "query_v2_model_item_limit",
            "typed reduction row count exceeds the caller's result-item budget",
        ));
    }
    for row in &rows {
        let group_cells = row.field_groups().map_or_else(
            || usize::from(row.has_group_evidence()),
            <[AttributeValue]>::len,
        );
        budget.charge_collection(group_cells.saturating_add(row.values().len()))?;
    }

    let group_roots = rows
        .iter()
        .filter_map(ReductionRow::group)
        .map(|thing| thing.concept_id().as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let finished = graph.finish(&group_roots)?;
    let rows = rows
        .iter()
        .map(|row| encode_reduction_row(row, group, &authority, &finished))
        .collect::<ExecResult<Vec<_>>>()?;
    Ok(RemoteOutcomeV2::ModelReduction {
        graph: finished.graph,
        root,
        group: group.cloned(),
        reducers: reducers.to_vec(),
        rows,
    })
}

fn reduction_fields<'field>(
    group: Option<&'field QueryReductionGroupV2>,
    reducers: &'field [QueryReductionTermV2],
) -> Vec<&'field QueryFieldV2> {
    let mut fields = Vec::new();
    match group {
        Some(QueryReductionGroupV2::Field { field }) => fields.push(field),
        Some(QueryReductionGroupV2::Fields { fields: grouped }) => {
            for field in grouped {
                if !fields.contains(&field) {
                    fields.push(field);
                }
            }
        }
        None | Some(QueryReductionGroupV2::Binding { .. }) => {}
    }
    for term in reducers {
        if let Some(field) = term.input()
            && !fields.contains(&field)
        {
            fields.push(field);
        }
    }
    fields
}

fn provider_reduction_solution(
    solution: &Solution,
    graph: &GraphBuilder,
    authority: &HydrationAuthority<'_>,
    fields: &[&QueryFieldV2],
) -> ExecResult<ProviderSolutionEvidence> {
    let bindings = solution
        .bindings
        .iter()
        .map(|(binding, iid)| {
            let node = graph.node(iid)?;
            let declared = authority.binding(*binding)?.declared_descriptor();
            let attributes = fields
                .iter()
                .filter(|field| field.binding().get() == *binding)
                .filter_map(|field| {
                    node.attributes
                        .iter()
                        .find(|(attribute, _)| attribute == field.attribute())
                        .map(|(_, values)| (*field, values))
                })
                .map(|(field, values)| {
                    let values = values
                        .iter()
                        .map(released_attribute_value)
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| PlanExecutionError::Provider(error.into()))?;
                    Ok(HydratedAttribute::new(reduction_field_id(field), values))
                })
                .collect::<ExecResult<Vec<_>>>()?;
            let thing = HydratedThing::new(
                ConceptId::new(iid.clone()),
                descriptor_id(declared)?,
                descriptor_id(&node.concrete)?,
                match node.kind {
                    HydrationNodeKindV2::Entity => ThingKind::Entity,
                    HydrationNodeKindV2::Relation => ThingKind::Relation,
                },
                attributes,
                Vec::new(),
            );
            Ok(BoundConceptEvidence::new(
                MatchBindingId::new(*binding),
                thing,
            ))
        })
        .collect::<ExecResult<Vec<_>>>()?;
    Ok(ProviderSolutionEvidence::new(bindings))
}

fn descriptor_id(value: &TypeId) -> ExecResult<DescriptorId> {
    let prefix = match value.kind() {
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Attribute | TypeKind::Struct => {
            return Err(validation_error(
                "query_v2_model_reduction_descriptor",
                "typed reduction references a non-thing descriptor",
            ));
        }
    };
    Ok(DescriptorId::new(format!("{prefix}:{}", value.label())))
}

fn reduction_field_id(field: &QueryFieldV2) -> FieldId {
    let prefix = match field.descriptor().kind() {
        TypeKind::Entity => "entity",
        TypeKind::Relation => "relation",
        TypeKind::Attribute | TypeKind::Struct => "invalid",
    };
    FieldId::new(
        DescriptorId::new(format!("{prefix}:{}", field.descriptor().label())),
        field.attribute().label().as_str(),
    )
}

fn lower_reduction_contract(
    group: Option<&QueryReductionGroupV2>,
    reducers: &[QueryReductionTermV2],
) -> ExecResult<(Option<LoweredReduceGroup>, Vec<LoweredReduceTerm>)> {
    let group = group
        .map(|group| {
            Ok(match group {
                QueryReductionGroupV2::Binding { binding } => {
                    LoweredReduceGroup::Binding(MatchBindingId::new(binding.get()))
                }
                QueryReductionGroupV2::Field { field } => {
                    LoweredReduceGroup::Field(BoundFieldId::new(
                        MatchBindingId::new(field.binding().get()),
                        reduction_field_id(field),
                    ))
                }
                QueryReductionGroupV2::Fields { fields } => LoweredReduceGroup::Fields(
                    fields
                        .iter()
                        .map(|field| {
                            BoundFieldId::new(
                                MatchBindingId::new(field.binding().get()),
                                reduction_field_id(field),
                            )
                        })
                        .collect(),
                ),
            })
        })
        .transpose()?;
    let terms = reducers
        .iter()
        .map(|term| {
            let reduction = match term.reduction() {
                QueryReductionKindV2::Count => Reduction::Count,
                QueryReductionKindV2::Sum => Reduction::Sum,
                QueryReductionKindV2::Min => Reduction::Min,
                QueryReductionKindV2::Max => Reduction::Max,
                QueryReductionKindV2::Mean => Reduction::Mean,
                QueryReductionKindV2::Median => Reduction::Median,
                QueryReductionKindV2::Std => Reduction::Std,
            };
            let input = term.input().map(|field| LoweredReduceInput {
                binding: MatchBindingId::new(field.binding().get()),
                field: reduction_field_id(field),
                domain: match field.value_type() {
                    ValueTypeTag::Long => ReduceDomain::Long,
                    ValueTypeTag::Double => ReduceDomain::Double,
                    _ => unreachable!("validated reduction inputs are numeric"),
                },
            });
            LoweredReduceTerm { reduction, input }
        })
        .collect();
    Ok((group, terms))
}

fn encode_reduction_row(
    row: &ReductionRow,
    group: Option<&QueryReductionGroupV2>,
    authority: &HydrationAuthority<'_>,
    graph: &FinishedGraph,
) -> ExecResult<RemoteReductionRowV2> {
    let group_value = match group {
        None => None,
        Some(QueryReductionGroupV2::Binding { binding }) => {
            let thing = row.group().ok_or_else(|| {
                evidence_error(
                    "query_v2_model_reduction_group",
                    "canonical thing-grouped reduction row lost its group evidence",
                )
            })?;
            let declared = authority.binding(binding.get())?.declared_descriptor();
            Some(RemoteReductionGroupV2::thing(
                graph.reference(declared, thing.concept_id().as_str())?,
            ))
        }
        Some(QueryReductionGroupV2::Field { .. }) => Some(RemoteReductionGroupV2::field(
            adapt_value(row.field_group().ok_or_else(|| {
                evidence_error(
                    "query_v2_model_reduction_group",
                    "canonical field-grouped reduction row lost its scalar group evidence",
                )
            })?)
            .map_err(PlanExecutionError::Validation)?,
        )),
        Some(QueryReductionGroupV2::Fields { .. }) => Some(RemoteReductionGroupV2::fields(
            row.field_groups()
                .ok_or_else(|| {
                    evidence_error(
                        "query_v2_model_reduction_group",
                        "canonical tuple-grouped reduction row lost its group evidence",
                    )
                })?
                .iter()
                .map(adapt_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(PlanExecutionError::Validation)?,
        )),
    };
    let values = row
        .values()
        .iter()
        .map(|value| match value {
            ReducedValue::Count(value) => RemoteReducedValueV2::Count { value: *value },
            ReducedValue::Long(value) => RemoteReducedValueV2::Long { value: *value },
            ReducedValue::Double(value) => RemoteReducedValueV2::DoubleBits {
                value: value.map(f64::to_bits),
            },
        })
        .collect();
    Ok(RemoteReductionRowV2::new(group_value, values))
}

fn validate_solution_predicate(
    validated: &ValidatedQuery,
    solution: &Solution,
    graph: &GraphBuilder,
) -> ExecResult<()> {
    let Some(predicate) = validated
        .plan()
        .v2_compatibility()
        .and_then(|compatibility| compatibility.predicate())
    else {
        return Ok(());
    };
    if evaluate_pattern(predicate, solution, graph)? {
        Ok(())
    } else {
        Err(evidence_error(
            "query_v2_model_predicate_evidence",
            "hydrated provider solution does not satisfy the validated compatibility predicate",
        ))
    }
}

fn cardinality_error(actual: usize) -> PlanExecutionError {
    PlanExecutionError::Validation(
        failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_model_exactly_one",
            "exactly-one model fetch did not produce exactly one distinct selected tuple",
        )
        .with_detail("actual", i64::try_from(actual).unwrap_or(i64::MAX)),
    )
}

fn cancelled_error(code: &'static str, message: &'static str) -> PlanExecutionError {
    PlanExecutionError::Validation(failure(DiagnosticCategory::Cancelled, code, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::query_plan::{CompatibilityValueV2, HydrationFieldV2};
    use type_bridge_contract::value::{CanonicalValue, Cardinality, DecimalValue};

    #[test]
    fn live_binding_ordinals_accept_only_integral_in_range_numbers() {
        assert_eq!(json_u16(&serde_json::json!(3.0)), Some(3));
        assert_eq!(json_u16(&serde_json::json!({"value": 4.0})), Some(4));
        assert_eq!(json_u16(&serde_json::json!(3.5)), None);
        assert_eq!(json_u16(&serde_json::json!(-1.0)), None);
        assert_eq!(json_u16(&serde_json::json!(65536.0)), None);
    }

    #[test]
    fn graph_finish_orders_attribute_evidence_by_descriptor() {
        let first = AttributeId::new("attribute-a").expect("attribute");
        let second = AttributeId::new("attribute-z").expect("attribute");
        let iid = "0x1".to_owned();
        let mut graph = GraphBuilder::new(RemoteReplyDecodeLimitsV2 {
            max_bytes: 1 << 20,
            max_items: 10,
            max_collection_members: 10,
            max_graph_nodes: 10,
            max_attribute_values: 10,
            max_role_players: 10,
        });
        graph
            .insert(NodeDraft {
                iid: iid.clone(),
                concrete: TypeId::new(TypeKind::Entity, "person").expect("type"),
                kind: HydrationNodeKindV2::Entity,
                // A generated-model alias order may be the reverse of the
                // provider attribute-descriptor order.
                attributes: vec![(second.clone(), Vec::new()), (first.clone(), Vec::new())],
                roles: Vec::new(),
                full: true,
            })
            .expect("draft");

        let finished = graph
            .finish(&BTreeSet::from([iid]))
            .expect("canonical graph");
        let attributes = finished.graph.nodes()[0].attributes();
        assert_eq!(attributes[0].attribute(), &first);
        assert_eq!(attributes[1].attribute(), &second);
    }

    #[test]
    fn unordered_hydration_collapses_released_decimal_spellings_semantically() {
        let person = TypeId::new(TypeKind::Entity, "person").expect("type");
        let field = HydrationFieldV2::new(
            "balance",
            vec![person],
            AttributeId::new("balance").expect("attribute"),
            ValueTypeTag::Decimal,
            Cardinality::new(0, None).expect("cardinality"),
            false,
            false,
            false,
        );
        let canonical_one = CompatibilityValueV2::canonical(CanonicalValue::Decimal(
            DecimalValue::new("1").expect("decimal"),
        ));
        let canonical_two = CompatibilityValueV2::canonical(CanonicalValue::Decimal(
            DecimalValue::new("2").expect("decimal"),
        ));
        let released_one =
            CompatibilityValueV2::released_decimal("1.0dec").expect("released decimal");
        let mut values = vec![canonical_two.clone(), released_one, canonical_one.clone()];

        canonicalize_field_values(&field, &mut values).expect("canonical hydration values");

        assert_eq!(values, vec![canonical_one, canonical_two]);
    }
}
