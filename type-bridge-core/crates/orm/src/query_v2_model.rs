//! Same-snapshot execution of schema-validated V1 compatibility models.
//!
//! This is deliberately not a second semantic lowerer. The adapter-authored
//! compatibility tree is lowered once by `query_v2_compatibility`; this module
//! executes only that closed typed provider plan and turns complete provider
//! evidence into the contract-owned hydration graph.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use regex::Regex;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::{
    AttributeId, MAX_THING_IID_HEX_DIGITS, RoleId, TypeId, TypeKind, is_canonical_thing_iid,
};
use type_bridge_contract::limits::MAX_BINDINGS;
use type_bridge_contract::migration_assertion::BindingId;
use type_bridge_contract::query_plan::{
    CompatibilityValueV2, HydrationBindingV2, HydrationDescriptorV2, HydrationFieldV2,
    HydrationProjectionV2, HydrationRoleV2, ModelQueryV2, QueryComparatorV2, QueryFieldV2,
    QueryInvocation, QueryModelOutputSlotV2, QueryModelOutputV2, QueryPatternV2,
    QueryRowCardinalityV2, QueryStableOrderV2,
};
use type_bridge_contract::query_remote_v2::{
    HydratedRowV2, HydrationAttributeEvidenceV2, HydrationGraphV2, HydrationNodeIdV2,
    HydrationNodeKindV2, HydrationNodeV2, HydrationReferenceV2, HydrationRoleEvidenceV2,
    HydrationSlotV2, RemoteOutcomeV2, RemoteReplyDecodeLimitsV2, RemoteResultKindV2,
    validate_remote_outcome_v2,
};
use type_bridge_contract::value::ValueTypeTag;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedHydrationDescriptor, TypedHydrationField,
    TypedHydrationRole, TypedHydrationTarget, TypedPageRematch, TypedRootScan, TypedThingKind,
};
use type_bridge_query::ValidatedQuery;
use unicase::UniCase;

use crate::query_v2::{QueryV2ExecutionError, failure, preflight_invocation_transport};
use crate::query_v2_adapter::adapt_value;
use crate::query_v2_compatibility::{
    CompatibilityProviderPlan, lower_validated_compatibility_query,
};
use crate::session::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerStats,
    MAX_ERROR_DRAIN_BYTES, MAX_ERROR_DRAIN_ITEMS, QueryV2AnswerLimits,
};
use crate::session::context::TransactionContext;
use crate::session::transaction::Transaction;
use crate::value::AttributeValue;

type ExecResult<T> = Result<T, QueryV2ExecutionError>;
type HydrationBatchPlan = (Vec<TypedHydrateThings>, BTreeSet<(u16, String)>);
const MAX_MODEL_STATEMENTS: u8 = 3;
const MAX_MODEL_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const TUPLE_PROOF_ROWS: u64 = 2;
const TUPLE_PROOF_ROW_ENVELOPE_BYTES: u64 = 64;
const TUPLE_PROOF_BINDING_FIXED_BYTES: u64 = 192 + MAX_THING_IID_HEX_DIGITS as u64;

enum ModelExecutionTarget<'target> {
    Owned(&'target mut Transaction),
    Borrowed(&'target TransactionContext),
}

impl ModelExecutionTarget<'_> {
    async fn query_typed_bounded(
        &mut self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, crate::error::OrmError> {
        match self {
            Self::Owned(transaction) => {
                transaction
                    .query_typed_bounded(query, limits, consumer)
                    .await
            }
            Self::Borrowed(context) => context.query_typed_bounded(query, limits, consumer).await,
        }
    }

    async fn query_tuple_typed_bounded(
        &mut self,
        query: &TypedFetchRows,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, crate::error::OrmError> {
        match self {
            Self::Owned(transaction) => {
                transaction
                    .query_tuple_typed_bounded(query, limits, consumer)
                    .await
            }
            Self::Borrowed(context) => {
                context
                    .query_tuple_typed_bounded(query, limits, consumer)
                    .await
            }
        }
    }

    async fn hydrate_typed_bounded(
        &mut self,
        query: &TypedHydrateThings,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, crate::error::OrmError> {
        match self {
            Self::Owned(transaction) => {
                transaction
                    .hydrate_typed_bounded(query, limits, consumer)
                    .await
            }
            Self::Borrowed(context) => context.hydrate_typed_bounded(query, limits, consumer).await,
        }
    }

    async fn query_root_typed_bounded(
        &mut self,
        query: &TypedRootScan,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, crate::error::OrmError> {
        match self {
            Self::Owned(transaction) => {
                transaction
                    .query_root_typed_bounded(query, limits, consumer)
                    .await
            }
            Self::Borrowed(context) => {
                context
                    .query_root_typed_bounded(query, limits, consumer)
                    .await
            }
        }
    }

    async fn rematch_page_typed_bounded(
        &mut self,
        query: &TypedPageRematch,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, crate::error::OrmError> {
        match self {
            Self::Owned(transaction) => {
                transaction
                    .rematch_page_typed_bounded(query, limits, consumer)
                    .await
            }
            Self::Borrowed(context) => {
                context
                    .rematch_page_typed_bounded(query, limits, consumer)
                    .await
            }
        }
    }

    async fn supports_exactly_one_tuple_proof(&mut self) -> Result<bool, crate::error::OrmError> {
        match self {
            Self::Owned(transaction) => transaction.supports_exactly_one_tuple_proof(),
            Self::Borrowed(context) => context.supports_exactly_one_tuple_proof().await,
        }
    }
}

/// Execute one adapter-authored model plan inside the caller's existing
/// schema-fenced transaction.
pub(crate) async fn execute_validated_model_query(
    transaction: &mut Transaction,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: QueryV2AnswerLimits,
    reply_limits: RemoteReplyDecodeLimitsV2,
) -> ExecResult<RemoteOutcomeV2> {
    execute_validated_model_query_in_target(
        ModelExecutionTarget::Owned(transaction),
        validated,
        invocation,
        limits,
        reply_limits,
        MAX_MODEL_STATEMENTS,
    )
    .await
}

pub(crate) async fn execute_validated_model_query_with_statement_limit(
    transaction: &mut Transaction,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: QueryV2AnswerLimits,
    reply_limits: RemoteReplyDecodeLimitsV2,
    max_statements: u8,
) -> ExecResult<RemoteOutcomeV2> {
    execute_validated_model_query_in_target(
        ModelExecutionTarget::Owned(transaction),
        validated,
        invocation,
        limits,
        reply_limits,
        max_statements,
    )
    .await
}

pub(crate) async fn execute_validated_model_query_borrowed(
    context: &TransactionContext,
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    limits: QueryV2AnswerLimits,
    reply_limits: RemoteReplyDecodeLimitsV2,
    max_statements: u8,
) -> ExecResult<RemoteOutcomeV2> {
    execute_validated_model_query_in_target(
        ModelExecutionTarget::Borrowed(context),
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
    limits: QueryV2AnswerLimits,
    reply_limits: RemoteReplyDecodeLimitsV2,
    max_statements: u8,
) -> ExecResult<RemoteOutcomeV2> {
    preflight_invocation_transport(validated.plan(), invocation)
        .map_err(QueryV2ExecutionError::Validation)?;
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
    let lowered = lower_validated_compatibility_query(validated, invocation.operation())
        .map_err(QueryV2ExecutionError::Validation)?
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
        _ => {
            return Err(validation_error(
                "query_v2_model_provider_plan_mismatch",
                "typed provider plan contradicts the schema-validated model terminal",
            ));
        }
    };
    validate_remote_outcome_v2(&outcome, expected, reply_limits, validated.plan())
        .map_err(QueryV2ExecutionError::Validation)?;
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
        limits: QueryV2AnswerLimits,
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
            return Err(resource_error(
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
                result = &mut future => result.map_err(QueryV2ExecutionError::Provider),
                () = &mut cancellation => Err(resource_error(
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
                result = &mut future => result.map_err(QueryV2ExecutionError::Provider),
                () = &mut cancellation => Err(resource_error(
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
    first_error: Option<QueryV2ExecutionError>,
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

    fn reject(&mut self, error: QueryV2ExecutionError) -> AnswerControl {
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

#[derive(Clone, Debug)]
struct Solution {
    bindings: BTreeMap<u16, String>,
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
    if projected_bindings == 0 || projected_bindings > MAX_BINDINGS {
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

struct HydrationAuthority<'plan> {
    projection: &'plan HydrationProjectionV2,
    bindings: BTreeMap<u16, &'plan HydrationBindingV2>,
    descriptors: BTreeMap<TypeId, &'plan HydrationDescriptorV2>,
}

impl<'plan> HydrationAuthority<'plan> {
    fn new(projection: &'plan HydrationProjectionV2) -> ExecResult<Self> {
        let bindings = projection
            .bindings()
            .iter()
            .map(|binding| (binding.binding().get(), binding))
            .collect::<BTreeMap<_, _>>();
        let descriptors = projection
            .descriptors()
            .iter()
            .map(|descriptor| (descriptor.descriptor().clone(), descriptor))
            .collect::<BTreeMap<_, _>>();
        if bindings.len() != projection.bindings().len()
            || descriptors.len() != projection.descriptors().len()
        {
            return Err(validation_error(
                "query_v2_model_hydration_authority",
                "validated hydration authority contains duplicate identities",
            ));
        }
        Ok(Self {
            projection,
            bindings,
            descriptors,
        })
    }

    fn binding(&self, binding: u16) -> ExecResult<&'plan HydrationBindingV2> {
        self.bindings.get(&binding).copied().ok_or_else(|| {
            validation_error(
                "query_v2_model_hydration_binding",
                "provider hydration references an unknown binding",
            )
        })
    }

    fn descriptor(&self, descriptor: &TypeId) -> ExecResult<&'plan HydrationDescriptorV2> {
        self.descriptors.get(descriptor).copied().ok_or_else(|| {
            validation_error(
                "query_v2_model_hydration_descriptor",
                "provider hydration references a descriptor outside the projection",
            )
        })
    }

    fn descriptor_by_label(&self, label: &str) -> ExecResult<&'plan HydrationDescriptorV2> {
        let mut matches = self
            .projection
            .descriptors()
            .iter()
            .filter(|descriptor| descriptor.descriptor().label().as_str() == label);
        let descriptor = matches.next().ok_or_else(|| {
            validation_error(
                "query_v2_model_hydration_descriptor",
                "provider hydration returned an unknown concrete type",
            )
        })?;
        if matches.next().is_some() {
            return Err(validation_error(
                "query_v2_model_hydration_descriptor",
                "provider hydration label is ambiguous across thing kinds",
            ));
        }
        Ok(descriptor)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PlayerDraft {
    declared: TypeId,
    iid: String,
}

#[derive(Clone, Debug, PartialEq)]
struct RoleDraft {
    role: RoleId,
    reference_roles: Vec<RoleId>,
    players: Vec<PlayerDraft>,
}

#[derive(Clone, Debug, PartialEq)]
struct NodeDraft {
    iid: String,
    concrete: TypeId,
    kind: HydrationNodeKindV2,
    attributes: Vec<(AttributeId, Vec<CompatibilityValueV2>)>,
    roles: Vec<RoleDraft>,
    full: bool,
}

struct GraphBuilder {
    nodes: BTreeMap<String, NodeDraft>,
    max_nodes: u64,
    max_attribute_values: u64,
    max_role_players: u64,
    attribute_values: u64,
    role_players: u64,
}

impl GraphBuilder {
    fn new(limits: RemoteReplyDecodeLimitsV2) -> Self {
        Self {
            nodes: BTreeMap::new(),
            max_nodes: limits.max_graph_nodes,
            max_attribute_values: limits.max_attribute_values,
            max_role_players: limits.max_role_players,
            attribute_values: 0,
            role_players: 0,
        }
    }

    fn insert(&mut self, node: NodeDraft) -> ExecResult<()> {
        if !is_canonical_thing_iid(&node.iid) {
            return Err(evidence_error(
                "query_v2_model_hydration_iid",
                "hydration returned a malformed provider IID",
            ));
        }
        if let Some(previous) = self.nodes.get(&node.iid) {
            if previous.concrete != node.concrete
                || previous.kind != node.kind
                || previous.attributes != node.attributes
            {
                return Err(evidence_error(
                    "query_v2_model_hydration_conflict",
                    "one provider IID carries contradictory concrete hydration evidence",
                ));
            }
            if previous.full && node.full && previous.roles != node.roles {
                return Err(evidence_error(
                    "query_v2_model_hydration_conflict",
                    "one provider IID carries contradictory relation-role evidence",
                ));
            }
            if !previous.full && node.full {
                self.charge_role_players(&node.roles)?;
                if let Some(previous) = self.nodes.get_mut(&node.iid) {
                    previous.roles = node.roles;
                    previous.full = true;
                } else {
                    return Err(validation_error(
                        "query_v2_model_hydration_state",
                        "hydration merge lost an existing provider identity",
                    ));
                }
            }
            return Ok(());
        }
        let next_nodes = u64::try_from(self.nodes.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if next_nodes > self.max_nodes {
            return Err(resource_error(
                "query_v2_model_graph_limit",
                "hydration construction exceeded the caller's graph-node budget",
            ));
        }
        let values = node
            .attributes
            .iter()
            .try_fold(0_u64, |total, (_, values)| {
                total
                    .checked_add(u64::try_from(values.len()).unwrap_or(u64::MAX))
                    .ok_or_else(|| {
                        resource_error(
                            "query_v2_model_attribute_limit",
                            "hydration attribute-value counter overflowed",
                        )
                    })
            })?;
        self.attribute_values = self
            .attribute_values
            .checked_add(values)
            .filter(|total| *total <= self.max_attribute_values)
            .ok_or_else(|| {
                resource_error(
                    "query_v2_model_attribute_limit",
                    "hydration construction exceeded the caller's attribute-value budget",
                )
            })?;
        if node.full {
            self.charge_role_players(&node.roles)?;
        }
        self.nodes.insert(node.iid.clone(), node);
        Ok(())
    }

    fn charge_role_players(&mut self, roles: &[RoleDraft]) -> ExecResult<()> {
        let players = roles.iter().try_fold(0_u64, |total, role| {
            total
                .checked_add(u64::try_from(role.players.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| {
                    resource_error(
                        "query_v2_model_role_player_limit",
                        "hydration role-player counter overflowed",
                    )
                })
        })?;
        self.role_players = self
            .role_players
            .checked_add(players)
            .filter(|total| *total <= self.max_role_players)
            .ok_or_else(|| {
                resource_error(
                    "query_v2_model_role_player_limit",
                    "hydration construction exceeded the caller's role-player budget",
                )
            })?;
        Ok(())
    }

    fn node(&self, iid: &str) -> ExecResult<&NodeDraft> {
        self.nodes.get(iid).ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_missing",
                "selected provider identity lacks complete hydration evidence",
            )
        })
    }

    fn finish(self, roots: &BTreeSet<String>) -> ExecResult<FinishedGraph> {
        let mut retained = BTreeSet::new();
        let mut pending = roots.iter().cloned().collect::<Vec<_>>();
        while let Some(iid) = pending.pop() {
            if !retained.insert(iid.clone()) {
                continue;
            }
            let node = self.nodes.get(&iid).ok_or_else(|| {
                evidence_error(
                    "query_v2_model_hydration_missing",
                    "output references a provider identity absent from hydration evidence",
                )
            })?;
            if node.full {
                for role in &node.roles {
                    pending.extend(role.players.iter().map(|player| player.iid.clone()));
                }
            }
        }
        let ids = retained
            .iter()
            .enumerate()
            .map(|(index, iid)| {
                let index = u32::try_from(index).map_err(|_| {
                    resource_error(
                        "query_v2_model_graph_limit",
                        "hydration graph ordinal exceeds the contract range",
                    )
                })?;
                Ok((iid.clone(), HydrationNodeIdV2::new(index)))
            })
            .collect::<ExecResult<BTreeMap<_, _>>>()?;
        let mut nodes = Vec::with_capacity(retained.len());
        for iid in retained {
            let draft = &self.nodes[&iid];
            let mut attributes = draft
                .attributes
                .iter()
                .map(|(attribute, values)| {
                    HydrationAttributeEvidenceV2::new(attribute.clone(), values.clone())
                })
                .collect::<Vec<_>>();
            // Registry field aliases determine model-member order, while the
            // remote graph contract canonicalizes evidence by provider
            // descriptor identity. Keep those independent order domains from
            // leaking into one another.
            attributes.sort_by(|left, right| left.attribute().cmp(right.attribute()));
            let mut roles = if draft.full {
                draft
                    .roles
                    .iter()
                    .map(|role| {
                        let players = role
                            .players
                            .iter()
                            .map(|player| {
                                ids.get(&player.iid)
                                    .copied()
                                    .map(|node| {
                                        HydrationReferenceV2::new(player.declared.clone(), node)
                                    })
                                    .ok_or_else(|| {
                                        evidence_error(
                                            "query_v2_model_hydration_missing",
                                            "relation role references missing player hydration",
                                        )
                                    })
                            })
                            .collect::<ExecResult<Vec<_>>>()?;
                        Ok(HydrationRoleEvidenceV2::new(role.role.clone(), players))
                    })
                    .collect::<ExecResult<Vec<_>>>()?
            } else {
                Vec::new()
            };
            roles.sort_by(|left, right| left.role().cmp(right.role()));
            nodes.push(HydrationNodeV2::new(
                ids[&iid],
                iid,
                draft.concrete.clone(),
                draft.kind,
                attributes,
                roles,
            ));
        }
        let graph = HydrationGraphV2::new(nodes).map_err(QueryV2ExecutionError::Validation)?;
        Ok(FinishedGraph { graph, ids })
    }
}

struct FinishedGraph {
    graph: HydrationGraphV2,
    ids: BTreeMap<String, HydrationNodeIdV2>,
}

impl FinishedGraph {
    fn reference(&self, declared: &TypeId, iid: &str) -> ExecResult<HydrationReferenceV2> {
        self.ids
            .get(iid)
            .copied()
            .map(|node| HydrationReferenceV2::new(declared.clone(), node))
            .ok_or_else(|| {
                evidence_error(
                    "query_v2_model_hydration_missing",
                    "output references a provider identity pruned from its hydration graph",
                )
            })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum WireThingKind {
    Entity,
    Relation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HydratedThingWire {
    binding: u16,
    concept_id: String,
    concrete_type: String,
    kind: WireThingKind,
    #[serde(default)]
    attributes: Vec<HydratedAttributeWire>,
    #[serde(default)]
    roles: Vec<HydratedRoleWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HydratedAttributeWire {
    field: String,
    value_type: String,
    values: Vec<JsonValue>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HydratedRoleWire {
    role: String,
    players: Vec<HydratedPlayerWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HydratedPlayerWire {
    concept_id: String,
    declared_type: String,
    concrete_type: String,
    kind: WireThingKind,
    #[serde(default)]
    attributes: Vec<HydratedAttributeWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RematchSolutionWire {
    bindings: Vec<HydratedThingWire>,
    #[serde(default)]
    satisfied_role_edges: Vec<u16>,
}

struct DecodedThing {
    binding: u16,
    node: NodeDraft,
    nested: Vec<NodeDraft>,
}

fn decode_hydrated_document(
    value: JsonValue,
    expected_binding: Option<u16>,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<DecodedThing> {
    if let Ok(wire) = serde_json::from_value::<HydratedThingWire>(value.clone()) {
        if expected_binding.is_some_and(|expected| expected != wire.binding) {
            return Err(evidence_error(
                "query_v2_model_hydration_binding",
                "hydration document contains the wrong binding ordinal",
            ));
        }
        return decode_exact_thing(wire, authority);
    }
    let object = value.as_object().ok_or_else(|| {
        evidence_error(
            "query_v2_model_hydration_document",
            "TypeDB hydration document is not an object",
        )
    })?;
    let binding = expected_binding
        .or_else(|| object.get("binding").and_then(json_u16))
        .ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_binding",
                "TypeDB hydration document omits its binding ordinal",
            )
        })?;
    let iid = object
        .get("concept_id")
        .and_then(json_string)
        .ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_iid",
                "TypeDB hydration document omits its provider IID",
            )
        })?
        .to_owned();
    let concrete_label = object
        .get("concrete_type")
        .and_then(json_string)
        .ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_descriptor",
                "TypeDB hydration document omits its concrete type",
            )
        })?;
    let projected = authority.descriptor_by_label(concrete_label)?;
    validate_binding_concrete(binding, projected.descriptor(), authority)?;
    let attributes = decode_wildcard_attributes(projected, object.get("attributes"))?;
    let (roles, nested) = if projected.descriptor().kind() == TypeKind::Relation {
        decode_wildcard_roles(projected, object.get("roles"), authority)?
    } else {
        if object
            .get("roles")
            .is_some_and(|value| value.as_array().is_none_or(|roles| !roles.is_empty()))
        {
            return Err(evidence_error(
                "query_v2_model_hydration_roles",
                "hydrated entity unexpectedly carries relation roles",
            ));
        }
        (Vec::new(), Vec::new())
    };
    Ok(DecodedThing {
        binding,
        node: NodeDraft {
            iid,
            concrete: projected.descriptor().clone(),
            kind: node_kind(projected.descriptor().kind())?,
            attributes,
            roles,
            full: true,
        },
        nested,
    })
}

fn decode_exact_thing(
    wire: HydratedThingWire,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<DecodedThing> {
    let projected = authority.descriptor_by_label(&wire.concrete_type)?;
    validate_binding_concrete(wire.binding, projected.descriptor(), authority)?;
    validate_wire_kind(wire.kind, projected.descriptor().kind())?;
    let attributes = decode_exact_attributes(projected, wire.attributes)?;
    let (roles, nested) = if projected.descriptor().kind() == TypeKind::Relation {
        decode_exact_roles(projected, wire.roles, authority)?
    } else if wire.roles.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        return Err(evidence_error(
            "query_v2_model_hydration_roles",
            "hydrated entity unexpectedly carries relation roles",
        ));
    };
    Ok(DecodedThing {
        binding: wire.binding,
        node: NodeDraft {
            iid: wire.concept_id,
            concrete: projected.descriptor().clone(),
            kind: node_kind(projected.descriptor().kind())?,
            attributes,
            roles,
            full: true,
        },
        nested,
    })
}

fn validate_binding_concrete(
    binding: u16,
    concrete: &TypeId,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<()> {
    if authority
        .binding(binding)?
        .concrete_descriptors()
        .contains(concrete)
    {
        Ok(())
    } else {
        Err(evidence_error(
            "query_v2_model_hydration_descriptor",
            "hydrated concrete type is outside the binding's validated subtype closure",
        ))
    }
}

fn decode_wildcard_attributes(
    descriptor: &HydrationDescriptorV2,
    value: Option<&JsonValue>,
) -> ExecResult<Vec<(AttributeId, Vec<CompatibilityValueV2>)>> {
    let object = match value {
        None => None,
        Some(value) => Some(value.as_object().ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_attributes",
                "TypeDB wildcard hydration attributes are not an object",
            )
        })?),
    };
    let mut result = Vec::with_capacity(descriptor.fields().len());
    for field in descriptor.fields() {
        let by_attribute = object.and_then(|object| object.get(field.attribute().label().as_str()));
        let by_alias = object.and_then(|object| object.get(field.alias()));
        if by_attribute.is_some()
            && by_alias.is_some()
            && field.alias() != field.attribute().label().as_str()
        {
            return Err(evidence_error(
                "query_v2_model_hydration_attributes",
                "TypeDB wildcard hydration repeats one projected attribute under two names",
            ));
        }
        let raw = by_attribute.or(by_alias);
        result.push((
            field.attribute().clone(),
            decode_attribute_values(field, raw)?,
        ));
    }
    if let Some(object) = object {
        for name in object.keys() {
            let known = descriptor
                .fields()
                .iter()
                .any(|field| field.alias() == name || field.attribute().label().as_str() == name);
            if !known {
                return Err(evidence_error(
                    "query_v2_model_hydration_attributes",
                    "TypeDB wildcard hydration returned an undeclared attribute",
                ));
            }
        }
    }
    Ok(result)
}

fn decode_attribute_values(
    field: &HydrationFieldV2,
    raw: Option<&JsonValue>,
) -> ExecResult<Vec<CompatibilityValueV2>> {
    let mut values = match raw {
        None => Vec::new(),
        Some(raw) if raw.is_null() => Vec::new(),
        Some(raw) => {
            let raw_values = raw
                .as_array()
                .map_or_else(|| vec![raw], |values| values.iter().collect());
            raw_values
                .into_iter()
                .filter(|value| !value.is_null())
                .map(|value| compatibility_value_from_json(value, field.value_type()))
                .collect::<ExecResult<Vec<_>>>()?
        }
    };
    canonicalize_field_values(field, &mut values)?;
    Ok(values)
}

fn decode_exact_attributes(
    descriptor: &HydrationDescriptorV2,
    attributes: Vec<HydratedAttributeWire>,
) -> ExecResult<Vec<(AttributeId, Vec<CompatibilityValueV2>)>> {
    let mut by_attribute = BTreeMap::new();
    for attribute in attributes {
        let field = descriptor
            .fields()
            .iter()
            .find(|field| {
                field.alias() == attribute.field
                    || field.attribute().label().as_str() == attribute.field
            })
            .ok_or_else(|| {
                evidence_error(
                    "query_v2_model_hydration_attributes",
                    "typed hydration returned an undeclared attribute",
                )
            })?;
        if attribute.value_type != value_type_label(field.value_type()) {
            return Err(evidence_error(
                "query_v2_model_hydration_value_type",
                "typed hydration attribute declares the wrong scalar domain",
            ));
        }
        let mut values = attribute
            .values
            .iter()
            .map(|value| compatibility_value_from_json(value, field.value_type()))
            .collect::<ExecResult<Vec<_>>>()?;
        canonicalize_field_values(field, &mut values)?;
        if by_attribute
            .insert(field.attribute().clone(), values)
            .is_some()
        {
            return Err(evidence_error(
                "query_v2_model_hydration_attributes",
                "typed hydration returns one attribute more than once",
            ));
        }
    }
    Ok(descriptor
        .fields()
        .iter()
        .map(|field| {
            (
                field.attribute().clone(),
                by_attribute.remove(field.attribute()).unwrap_or_default(),
            )
        })
        .collect())
}

fn canonicalize_field_values(
    field: &HydrationFieldV2,
    values: &mut Vec<CompatibilityValueV2>,
) -> ExecResult<()> {
    if field.ordered() {
        return Ok(());
    }

    // Compatibility values have a representation identity for stable wire
    // fingerprints and a semantic identity for released V1 scalar behavior.
    // Sort by the latter, using the former only to choose one deterministic
    // spelling before collapsing semantic duplicates.
    for index in 1..values.len() {
        let mut current = index;
        while current > 0 {
            let semantic = values[current - 1]
                .semantic_cmp_same_domain(&values[current])
                .ok_or_else(|| {
                    evidence_error(
                        "query_v2_model_hydration_value_order",
                        "unordered hydration values are not comparable in their validated domain",
                    )
                })?;
            let canonical = semantic.then_with(|| values[current - 1].cmp(&values[current]));
            if canonical != Ordering::Greater {
                break;
            }
            values.swap(current - 1, current);
            current -= 1;
        }
    }
    values.dedup_by(|left, right| left.semantic_cmp_same_domain(right) == Some(Ordering::Equal));
    Ok(())
}

fn decode_wildcard_roles(
    descriptor: &HydrationDescriptorV2,
    value: Option<&JsonValue>,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<(Vec<RoleDraft>, Vec<NodeDraft>)> {
    let documents = match value {
        None => &[][..],
        Some(value) => value.as_array().ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_roles",
                "TypeDB nested role hydration is not a list",
            )
        })?,
    };
    let mut players = BTreeMap::<RoleId, Vec<PlayerDraft>>::new();
    let mut nested = Vec::new();
    for document in documents {
        let document = document.as_object().ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_roles",
                "TypeDB nested role hydration member is not an object",
            )
        })?;
        let role_label = document.get("role").and_then(json_string).ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_roles",
                "TypeDB nested role hydration omits the role label",
            )
        })?;
        let role = projected_role(descriptor, role_label)?;
        let iid = document
            .get("player_concept_id")
            .and_then(json_string)
            .ok_or_else(|| {
                evidence_error(
                    "query_v2_model_hydration_role_player",
                    "TypeDB nested role hydration omits the player IID",
                )
            })?
            .to_owned();
        let concrete_label = document
            .get("player_concrete_type")
            .and_then(json_string)
            .ok_or_else(|| {
                evidence_error(
                    "query_v2_model_hydration_role_player",
                    "TypeDB nested role hydration omits the player concrete type",
                )
            })?;
        let projected = authority.descriptor_by_label(concrete_label)?;
        let declared = declared_player_type(role, projected.descriptor())?;
        let attributes = decode_wildcard_attributes(projected, document.get("attributes"))?;
        nested.push(NodeDraft {
            iid: iid.clone(),
            concrete: projected.descriptor().clone(),
            kind: node_kind(projected.descriptor().kind())?,
            attributes,
            roles: Vec::new(),
            full: false,
        });
        players
            .entry(role.role().clone())
            .or_default()
            .push(PlayerDraft { declared, iid });
    }
    complete_roles(descriptor, players, nested)
}

fn decode_exact_roles(
    descriptor: &HydrationDescriptorV2,
    roles: Vec<HydratedRoleWire>,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<(Vec<RoleDraft>, Vec<NodeDraft>)> {
    let mut grouped = BTreeMap::<RoleId, Vec<PlayerDraft>>::new();
    let mut nested = Vec::new();
    for wire_role in roles {
        let role = projected_role(descriptor, &wire_role.role)?;
        if grouped.contains_key(role.role()) {
            return Err(evidence_error(
                "query_v2_model_hydration_roles",
                "typed hydration returns one relation role more than once",
            ));
        }
        let mut role_players = Vec::new();
        for player in wire_role.players {
            let projected = authority.descriptor_by_label(&player.concrete_type)?;
            validate_wire_kind(player.kind, projected.descriptor().kind())?;
            let declared = TypeId::new(projected.descriptor().kind(), player.declared_type)
                .map_err(QueryV2ExecutionError::Validation)?;
            let compatible = role.players().iter().any(|authority| {
                authority.declared_descriptor() == &declared
                    && authority
                        .concrete_descriptors()
                        .contains(projected.descriptor())
            });
            if !compatible {
                return Err(evidence_error(
                    "query_v2_model_hydration_role_player",
                    "typed hydration role player violates its declared-to-concrete authority",
                ));
            }
            let attributes = decode_exact_attributes(projected, player.attributes)?;
            nested.push(NodeDraft {
                iid: player.concept_id.clone(),
                concrete: projected.descriptor().clone(),
                kind: node_kind(projected.descriptor().kind())?,
                attributes,
                roles: Vec::new(),
                full: false,
            });
            role_players.push(PlayerDraft {
                declared,
                iid: player.concept_id,
            });
        }
        grouped.insert(role.role().clone(), role_players);
    }
    complete_roles(descriptor, grouped, nested)
}

fn complete_roles(
    descriptor: &HydrationDescriptorV2,
    mut grouped: BTreeMap<RoleId, Vec<PlayerDraft>>,
    nested: Vec<NodeDraft>,
) -> ExecResult<(Vec<RoleDraft>, Vec<NodeDraft>)> {
    let mut roles = Vec::with_capacity(descriptor.roles().len());
    for projection in descriptor.roles() {
        let mut players = grouped.remove(projection.role()).unwrap_or_default();
        if !projection.ordered() {
            players.sort_by(|left, right| {
                (&left.iid, &left.declared).cmp(&(&right.iid, &right.declared))
            });
        }
        roles.push(RoleDraft {
            role: projection.role().clone(),
            reference_roles: projection.reference_roles().to_vec(),
            players,
        });
    }
    if !grouped.is_empty() {
        return Err(evidence_error(
            "query_v2_model_hydration_roles",
            "hydration returned a role outside the concrete projection",
        ));
    }
    Ok((roles, nested))
}

fn projected_role<'a>(
    descriptor: &'a HydrationDescriptorV2,
    label: &str,
) -> ExecResult<&'a HydrationRoleV2> {
    let (owner, role_label) = label
        .rsplit_once(':')
        .map_or((None, label), |(owner, role)| (Some(owner), role));
    let mut matches = descriptor.roles().iter().filter(|role| {
        role.role().label().as_str() == role_label
            && owner.is_none_or(|owner| {
                role.role().declaring_relation().as_str() == owner
                    || role.reference_roles().iter().any(|reference| {
                        reference.declaring_relation().as_str() == owner
                            && reference.label().as_str() == role_label
                    })
            })
    });
    let role = matches.next().ok_or_else(|| {
        evidence_error(
            "query_v2_model_hydration_roles",
            "hydration returned an undeclared relation role",
        )
    })?;
    if matches.next().is_some() {
        return Err(evidence_error(
            "query_v2_model_hydration_roles",
            "hydration role label is ambiguous under the concrete relation",
        ));
    }
    Ok(role)
}

fn declared_player_type(role: &HydrationRoleV2, concrete: &TypeId) -> ExecResult<TypeId> {
    role.players()
        .iter()
        .find(|player| player.concrete_descriptors().contains(concrete))
        .map(|player| player.declared_descriptor().clone())
        .ok_or_else(|| {
            evidence_error(
                "query_v2_model_hydration_role_player",
                "hydrated role player is outside the role's concrete player closure",
            )
        })
}

fn compatibility_value_from_json(
    value: &JsonValue,
    expected: ValueTypeTag,
) -> ExecResult<CompatibilityValueV2> {
    let value = AttributeValue::from_json(value, value_type_label(expected)).ok_or_else(|| {
        evidence_error(
            "query_v2_model_hydration_value_type",
            "hydration attribute value does not match its validated scalar domain",
        )
    })?;
    adapt_value(&value).map_err(QueryV2ExecutionError::Validation)
}

const fn value_type_label(value_type: ValueTypeTag) -> &'static str {
    match value_type {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "long",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime-tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    }
}

fn validate_wire_kind(kind: WireThingKind, expected: TypeKind) -> ExecResult<()> {
    if matches!(
        (kind, expected),
        (WireThingKind::Entity, TypeKind::Entity) | (WireThingKind::Relation, TypeKind::Relation)
    ) {
        Ok(())
    } else {
        Err(evidence_error(
            "query_v2_model_hydration_kind",
            "hydration thing kind contradicts its concrete descriptor",
        ))
    }
}

fn node_kind(kind: TypeKind) -> ExecResult<HydrationNodeKindV2> {
    match kind {
        TypeKind::Entity => Ok(HydrationNodeKindV2::Entity),
        TypeKind::Relation => Ok(HydrationNodeKindV2::Relation),
        TypeKind::Attribute | TypeKind::Struct => Err(validation_error(
            "query_v2_model_hydration_kind",
            "model hydration projection contains a non-thing descriptor",
        )),
    }
}

fn json_string(value: &JsonValue) -> Option<&str> {
    bounded_unwrap_value(value)?.as_str()
}

fn json_u16(value: &JsonValue) -> Option<u16> {
    let value = bounded_unwrap_value(value)?;
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| {
            value.as_f64().and_then(|value| {
                (value.is_finite()
                    && value.fract() == 0.0
                    && value >= 0.0
                    && value <= f64::from(u16::MAX))
                .then_some(value as u16)
            })
        })
}

fn bounded_unwrap_value(mut value: &JsonValue) -> Option<&JsonValue> {
    const MAX_WRAPPER_DEPTH: usize = 16;
    for _ in 0..=MAX_WRAPPER_DEPTH {
        let Some(next) = value.as_object().and_then(|object| object.get("value")) else {
            return Some(value);
        };
        value = next;
    }
    None
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

#[derive(Clone)]
enum SlotDraft {
    Singular { declared: TypeId, iid: String },
    Collection { declared: TypeId, iids: Vec<String> },
}

fn finish_hydrated_rows(
    graph: GraphBuilder,
    drafts: Vec<Vec<SlotDraft>>,
) -> ExecResult<(HydrationGraphV2, Vec<HydratedRowV2>)> {
    let roots = drafts
        .iter()
        .flat_map(|row| row.iter())
        .flat_map(|slot| match slot {
            SlotDraft::Singular { iid, .. } => std::slice::from_ref(iid).iter(),
            SlotDraft::Collection { iids, .. } => iids.iter(),
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let finished = graph.finish(&roots)?;
    let rows = drafts
        .into_iter()
        .map(|row| {
            let slots = row
                .into_iter()
                .map(|slot| match slot {
                    SlotDraft::Singular { declared, iid } => Ok(HydrationSlotV2::Singular {
                        value: finished.reference(&declared, &iid)?,
                    }),
                    SlotDraft::Collection { declared, iids } => Ok(HydrationSlotV2::Collection {
                        values: iids
                            .into_iter()
                            .map(|iid| finished.reference(&declared, &iid))
                            .collect::<ExecResult<Vec<_>>>()?,
                    }),
                })
                .collect::<ExecResult<Vec<_>>>()?;
            Ok(HydratedRowV2::new(slots))
        })
        .collect::<ExecResult<Vec<_>>>()?;
    Ok((finished.graph, rows))
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
        let graph = HydrationGraphV2::new(Vec::new()).map_err(QueryV2ExecutionError::Validation)?;
        return Ok(RemoteOutcomeV2::HydratedRows {
            graph,
            rows: Vec::new(),
        });
    }
    let provider_terminal = provider_projection_is_public(&statement);
    let needs_tuple_proof =
        cardinality.is_exactly_one() && !provider_terminal && tuple_proof.is_some();
    let retain = if cardinality.is_exactly_one() {
        2
    } else {
        offset.saturating_add(limit)
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
            .await_provider(transaction.supports_exactly_one_tuple_proof())
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
        let graph = HydrationGraphV2::new(Vec::new()).map_err(QueryV2ExecutionError::Validation)?;
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

fn try_sort_iids(
    values: &mut [String],
    order: &QueryStableOrderV2,
    graph: &GraphBuilder,
) -> ExecResult<()> {
    for index in 1..values.len() {
        let mut current = index;
        while current > 0
            && compare_iids_by_order(&values[current - 1], &values[current], order, graph)?
                == Ordering::Greater
        {
            values.swap(current - 1, current);
            current -= 1;
        }
    }
    Ok(())
}

fn compare_iids_by_order(
    left: &str,
    right: &str,
    order: &QueryStableOrderV2,
    graph: &GraphBuilder,
) -> ExecResult<Ordering> {
    for term in order.terms() {
        let left = order_field_value(graph.node(left)?, term.field())?;
        let right = order_field_value(graph.node(right)?, term.field())?;
        let ordering = compare_optional_field_values(left, right, term.missing(), term.field())?;
        let ordering = if term.direction()
            == type_bridge_contract::query_plan::QueryOrderDirectionV2::Descending
        {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(left.cmp(right))
}

fn order_field_value<'a>(
    node: &'a NodeDraft,
    field: &QueryFieldV2,
) -> ExecResult<Option<&'a CompatibilityValueV2>> {
    let values = node
        .attributes
        .iter()
        .find(|(attribute, _)| attribute == field.attribute())
        .map(|(_, values)| values.as_slice())
        .unwrap_or_default();
    if values.len() > 1 {
        return Err(field_evidence_error(
            "query_v2_model_order_non_scalar",
            "stable order field carries more than one hydrated scalar value",
            field,
        ));
    }
    Ok(values.first())
}

fn compare_optional_values(
    left: Option<&CompatibilityValueV2>,
    right: Option<&CompatibilityValueV2>,
    missing: type_bridge_contract::query_plan::QueryMissingOrderV2,
) -> ExecResult<Ordering> {
    match (left, right) {
        (None, None) => Ok(Ordering::Equal),
        (None, Some(_)) => match missing {
            type_bridge_contract::query_plan::QueryMissingOrderV2::First => Ok(Ordering::Less),
            type_bridge_contract::query_plan::QueryMissingOrderV2::Last => Ok(Ordering::Greater),
            type_bridge_contract::query_plan::QueryMissingOrderV2::Reject => Err(evidence_error(
                "query_v2_model_order_missing",
                "stable order rejects missing hydrated values",
            )),
        },
        (Some(_), None) => match missing {
            type_bridge_contract::query_plan::QueryMissingOrderV2::First => Ok(Ordering::Greater),
            type_bridge_contract::query_plan::QueryMissingOrderV2::Last => Ok(Ordering::Less),
            type_bridge_contract::query_plan::QueryMissingOrderV2::Reject => Err(evidence_error(
                "query_v2_model_order_missing",
                "stable order rejects missing hydrated values",
            )),
        },
        (Some(left), Some(right)) => left.semantic_cmp_same_domain(right).ok_or_else(|| {
            evidence_error(
                "query_v2_model_order_value_type",
                "stable order values are not comparable in their validated domain",
            )
        }),
    }
}

fn validate_solution_order(
    solutions: &[Solution],
    order: &QueryStableOrderV2,
    graph: &GraphBuilder,
) -> ExecResult<()> {
    for adjacent in solutions.windows(2) {
        if compare_solutions_by_order(&adjacent[0], &adjacent[1], order, graph)?
            == Ordering::Greater
        {
            return Err(evidence_error(
                "query_v2_model_unstable_provider_order",
                "provider solutions violate the validator-derived stable total order",
            ));
        }
    }
    Ok(())
}

fn compare_solutions_by_order(
    left: &Solution,
    right: &Solution,
    order: &QueryStableOrderV2,
    graph: &GraphBuilder,
) -> ExecResult<Ordering> {
    for term in order.terms() {
        let left = solution_field_values(left, graph, term.field())?;
        let right = solution_field_values(right, graph, term.field())?;
        if left.len() > 1 || right.len() > 1 {
            return Err(field_evidence_error(
                "query_v2_model_order_non_scalar",
                "stable order field carries more than one hydrated scalar value",
                term.field(),
            ));
        }
        let ordering = compare_optional_field_values(
            left.first(),
            right.first(),
            term.missing(),
            term.field(),
        )?;
        let ordering = if term.direction()
            == type_bridge_contract::query_plan::QueryOrderDirectionV2::Descending
        {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    for binding in order.identity_tiebreakers() {
        let left = left.bindings.get(&binding.get()).ok_or_else(|| {
            evidence_error(
                "query_v2_model_solution_binding",
                "stable-order identity tie breaker references a missing binding",
            )
        })?;
        let right = right.bindings.get(&binding.get()).ok_or_else(|| {
            evidence_error(
                "query_v2_model_solution_binding",
                "stable-order identity tie breaker references a missing binding",
            )
        })?;
        let ordering = left.cmp(right);
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn charge_solution_semantics(
    solutions: &[Solution],
    graph: &GraphBuilder,
    limits: RemoteReplyDecodeLimitsV2,
) -> ExecResult<()> {
    let mut hydrated_things = 0_u64;
    let mut attribute_values = 0_u64;
    for solution in solutions {
        for iid in solution.bindings.values() {
            let node = graph.node(iid)?;
            hydrated_things = hydrated_things.checked_add(1).ok_or_else(|| {
                resource_error(
                    "query_v2_model_graph_limit",
                    "hydrated thing multiplicity counter overflowed",
                )
            })?;
            attribute_values = attribute_values
                .checked_add(node_attribute_value_count(node)?)
                .ok_or_else(|| {
                    resource_error(
                        "query_v2_model_attribute_limit",
                        "hydrated attribute-value multiplicity counter overflowed",
                    )
                })?;
            for role in &node.roles {
                for player in &role.players {
                    hydrated_things = hydrated_things.checked_add(1).ok_or_else(|| {
                        resource_error(
                            "query_v2_model_graph_limit",
                            "hydrated thing multiplicity counter overflowed",
                        )
                    })?;
                    attribute_values = attribute_values
                        .checked_add(node_attribute_value_count(graph.node(&player.iid)?)?)
                        .ok_or_else(|| {
                            resource_error(
                                "query_v2_model_attribute_limit",
                                "hydrated attribute-value multiplicity counter overflowed",
                            )
                        })?;
                }
            }
        }
    }
    if hydrated_things > limits.max_graph_nodes {
        return Err(resource_error(
            "query_v2_model_graph_limit",
            "provider hydration exceeded the hydrated-thing ceiling",
        ));
    }
    if attribute_values > limits.max_attribute_values {
        return Err(resource_error(
            "query_v2_model_attribute_limit",
            "provider hydration exceeded the attribute-value ceiling",
        ));
    }
    Ok(())
}

fn node_attribute_value_count(node: &NodeDraft) -> ExecResult<u64> {
    node.attributes
        .iter()
        .try_fold(0_u64, |count, (_, values)| {
            count
                .checked_add(u64::try_from(values.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| {
                    resource_error(
                        "query_v2_model_attribute_limit",
                        "hydrated attribute-value counter overflowed",
                    )
                })
        })
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

fn evaluate_pattern(
    pattern: &QueryPatternV2,
    solution: &Solution,
    graph: &GraphBuilder,
) -> ExecResult<bool> {
    match pattern {
        QueryPatternV2::FieldValue {
            field,
            comparator,
            value,
        } => {
            for candidate in solution_field_values(solution, graph, field)? {
                if compare_predicate_values(*comparator, candidate, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        QueryPatternV2::FieldComparison {
            left,
            comparator,
            right,
        } => {
            let left = solution_field_values(solution, graph, left)?;
            let right = solution_field_values(solution, graph, right)?;
            for left in left {
                for right in right {
                    if compare_predicate_values(*comparator, left, right)? {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        QueryPatternV2::FieldPresence { field, present } => {
            Ok(solution_field_values(solution, graph, field)?.is_empty() != *present)
        }
        QueryPatternV2::BindingIid { binding, iid } => Ok(solution
            .bindings
            .get(&binding.get())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(iid))),
        QueryPatternV2::RoleEdge {
            relation,
            role,
            player,
            ..
        } => {
            let relation_iid = solution.bindings.get(&relation.get()).ok_or_else(|| {
                evidence_error(
                    "query_v2_model_solution_binding",
                    "role edge references a missing relation binding",
                )
            })?;
            let player_iid = solution.bindings.get(&player.get()).ok_or_else(|| {
                evidence_error(
                    "query_v2_model_solution_binding",
                    "role edge references a missing player binding",
                )
            })?;
            let relation = graph.node(relation_iid)?;
            Ok(relation.roles.iter().any(|candidate| {
                (&candidate.role == role || candidate.reference_roles.contains(role))
                    && candidate
                        .players
                        .iter()
                        .any(|candidate| candidate.iid == *player_iid)
            }))
        }
        QueryPatternV2::Reachable {
            source,
            target,
            max_depth,
            ..
        } => {
            if *max_depth == 0 {
                Ok(solution.bindings.get(&source.get()) == solution.bindings.get(&target.get()))
            } else {
                // Positive path witnesses are intentionally existential and
                // projected away by the typed compiler. The provider boundary
                // is trusted only for that path proof; both public endpoints
                // and every materialized node remain independently checked.
                Ok(true)
            }
        }
        QueryPatternV2::And { patterns } => {
            for child in patterns {
                if !evaluate_pattern(child, solution, graph)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        QueryPatternV2::Or { patterns } => {
            for child in patterns {
                if evaluate_pattern(child, solution, graph)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        QueryPatternV2::Not { pattern } => Ok(!evaluate_pattern(pattern, solution, graph)?),
    }
}

fn solution_field_values<'a>(
    solution: &Solution,
    graph: &'a GraphBuilder,
    field: &QueryFieldV2,
) -> ExecResult<&'a [CompatibilityValueV2]> {
    let iid = solution
        .bindings
        .get(&field.binding().get())
        .ok_or_else(|| {
            evidence_error(
                "query_v2_model_solution_binding",
                "field predicate references a missing binding",
            )
        })?;
    Ok(graph
        .node(iid)?
        .attributes
        .iter()
        .find(|(attribute, _)| attribute == field.attribute())
        .map(|(_, values)| values.as_slice())
        .unwrap_or_default())
}

fn compare_predicate_values(
    comparator: QueryComparatorV2,
    left: &CompatibilityValueV2,
    right: &CompatibilityValueV2,
) -> ExecResult<bool> {
    match comparator {
        QueryComparatorV2::Equal | QueryComparatorV2::NotEqual => {
            let equal = left
                .semantic_cmp_same_domain(right)
                .map(|ordering| ordering == Ordering::Equal)
                .ok_or_else(|| {
                    evidence_error(
                        "query_v2_model_predicate_equal_type",
                        "predicate values do not share their validated scalar domain",
                    )
                })?;
            Ok(if comparator == QueryComparatorV2::Equal {
                equal
            } else {
                !equal
            })
        }
        QueryComparatorV2::Less
        | QueryComparatorV2::LessOrEqual
        | QueryComparatorV2::Greater
        | QueryComparatorV2::GreaterOrEqual => {
            let ordering = left.semantic_cmp_same_domain(right).ok_or_else(|| {
                evidence_error(
                    "query_v2_model_predicate_order_type",
                    "predicate values are not order-compatible",
                )
            })?;
            Ok(match comparator {
                QueryComparatorV2::Less => ordering == Ordering::Less,
                QueryComparatorV2::LessOrEqual => ordering != Ordering::Greater,
                QueryComparatorV2::Greater => ordering == Ordering::Greater,
                QueryComparatorV2::GreaterOrEqual => ordering != Ordering::Less,
                _ => false,
            })
        }
        QueryComparatorV2::Contains
        | QueryComparatorV2::StartsWith
        | QueryComparatorV2::EndsWith
        | QueryComparatorV2::Regex => {
            let left = compatibility_string(left)?;
            let right = compatibility_string(right)?;
            match comparator {
                QueryComparatorV2::Contains => Ok(UniCase::new(left.as_str())
                    .to_folded_case()
                    .contains(&UniCase::new(right.as_str()).to_folded_case())),
                QueryComparatorV2::StartsWith => Ok(left.starts_with(&right)),
                QueryComparatorV2::EndsWith => Ok(left.ends_with(&right)),
                QueryComparatorV2::Regex => Regex::new(&right)
                    .map(|regex| regex.is_match(&left))
                    .map_err(|_| {
                        validation_error(
                            "query_v2_model_regex",
                            "validated compatibility predicate contains an invalid regular expression",
                        )
                    }),
                _ => Ok(false),
            }
        }
    }
}

fn compatibility_string(value: &CompatibilityValueV2) -> ExecResult<String> {
    if value.value_type() != ValueTypeTag::String {
        return Err(evidence_error(
            "query_v2_model_predicate_string_type",
            "string predicate operator received a non-string value",
        ));
    }
    if let Some(type_bridge_contract::value::CanonicalValue::String(value)) =
        value.canonical_value()
    {
        return Ok(value.as_str().to_owned());
    }
    value.released_text().ok_or_else(|| {
        evidence_error(
            "query_v2_model_predicate_string_type",
            "string predicate value has no canonical or released representation",
        )
    })
}

fn cardinality_error(actual: usize) -> QueryV2ExecutionError {
    QueryV2ExecutionError::Validation(
        failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_model_exactly_one",
            "exactly-one model fetch did not produce exactly one distinct selected tuple",
        )
        .with_detail("actual", i64::try_from(actual).unwrap_or(i64::MAX)),
    )
}

fn validation_error(code: &'static str, message: &'static str) -> QueryV2ExecutionError {
    QueryV2ExecutionError::Validation(failure(DiagnosticCategory::InvalidContract, code, message))
}

fn evidence_error(code: &'static str, message: &'static str) -> QueryV2ExecutionError {
    QueryV2ExecutionError::Validation(evidence_failure(code, message))
}

fn field_evidence_error(
    code: &'static str,
    message: &'static str,
    field: &QueryFieldV2,
) -> QueryV2ExecutionError {
    let owner_kind = match field.descriptor().kind() {
        type_bridge_contract::id::TypeKind::Entity => "entity",
        type_bridge_contract::id::TypeKind::Relation => "relation",
        type_bridge_contract::id::TypeKind::Attribute => "attribute",
        type_bridge_contract::id::TypeKind::Struct => "struct",
    };
    QueryV2ExecutionError::Validation(
        evidence_failure(code, message)
            .with_detail(
                "field_owner",
                format!("{owner_kind}:{}", field.descriptor().label().as_str()),
            )
            .with_detail("field_name", field.attribute().label().as_str().to_owned()),
    )
}

fn compare_optional_field_values(
    left: Option<&CompatibilityValueV2>,
    right: Option<&CompatibilityValueV2>,
    missing: type_bridge_contract::query_plan::QueryMissingOrderV2,
    field: &QueryFieldV2,
) -> ExecResult<Ordering> {
    compare_optional_values(left, right, missing).map_err(|error| match error {
        QueryV2ExecutionError::Validation(diagnostic)
            if diagnostic.code().as_str() == "query_v2_model_order_missing" =>
        {
            field_evidence_error(
                "query_v2_model_order_missing",
                "stable order rejects missing hydrated values",
                field,
            )
        }
        QueryV2ExecutionError::Validation(diagnostic)
            if diagnostic.code().as_str() == "query_v2_model_order_value_type" =>
        {
            field_evidence_error(
                "query_v2_model_order_value_type",
                "stable order values are not comparable in their validated domain",
                field,
            )
        }
        error => error,
    })
}

fn resource_error(code: &'static str, message: &'static str) -> QueryV2ExecutionError {
    QueryV2ExecutionError::Validation(failure(DiagnosticCategory::ResourceLimit, code, message))
}

fn evidence_failure(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::Integrity, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::value::{CanonicalValue, Cardinality, DecimalValue};

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
