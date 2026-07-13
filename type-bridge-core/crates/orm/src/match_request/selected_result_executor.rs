//! End-to-end selected-row execution over bounded typed provider statements.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedHydrationDescriptor, TypedHydrationField,
    TypedHydrationRole, TypedHydrationTarget, TypedPageRematch, TypedRootScan, TypedThingKind,
};

use super::capability::CapabilitySet;
use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::{BindingId, DescriptorId, FieldId, RoleEdgeId, RoleId};
use super::lowering::{LoweredMatchExecution, lower_match_execution};
use super::model::{
    FetchShape, FetchSlot, MatchExpr, MatchOperation, RowCardinality, ThingKind, Window,
};
use super::result::{
    BoundConceptEvidence, ConceptId, HydratedAttribute, HydratedRole, HydratedRolePlayer,
    HydratedThing, ProviderResultEvidence, ProviderSolutionEvidence, ValidatedMatchResult,
};
use super::result_validation::{ResultValidationLimits, validate_provider_result_with_limits};
use super::validation::ValidatedMatchRequest;
use crate::descriptor::{TypeDescriptor, TypeDescriptorRef};
use crate::error::OrmError;
use crate::registry::DescriptorRegistry;
use crate::session::backend::{
    AnswerCancellation, AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, TxType,
};
use crate::session::{Database, Transaction, TransactionContext};
use crate::value::AttributeValue;

const MAX_PROCESSED_ITEMS: u64 = 100_000;
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSACTION_DURATION: Duration = Duration::from_secs(30);
const MAX_STATEMENTS: u8 = 3;

/// Caller policy clamped to the selected executor's hard ceilings.
#[derive(Debug, Clone)]
pub struct MatchExecutionLimits {
    max_items: u64,
    max_bytes: u64,
    timeout: Duration,
    cancellation: AnswerCancellation,
    max_hydrated_things: u64,
    max_attribute_values: u64,
    max_collected_concepts: u64,
    max_statements: u8,
}

impl MatchExecutionLimits {
    /// Construct an execution policy that may only tighten hard ceilings.
    pub fn tightened(
        max_items: u64,
        max_bytes: u64,
        timeout: Duration,
        cancellation: AnswerCancellation,
    ) -> Self {
        let max_items = max_items.min(MAX_PROCESSED_ITEMS);
        Self {
            max_items,
            max_bytes: max_bytes.min(MAX_RESPONSE_BYTES),
            timeout: timeout.min(MAX_TRANSACTION_DURATION),
            cancellation,
            max_hydrated_things: max_items,
            max_attribute_values: max_items,
            max_collected_concepts: max_items,
            max_statements: MAX_STATEMENTS,
        }
    }

    /// Further tighten the hydrated-thing ceiling, including nested role players.
    pub fn with_max_hydrated_things(mut self, max_hydrated_things: u64) -> Self {
        self.max_hydrated_things = max_hydrated_things.min(self.max_hydrated_things);
        self
    }

    /// Further tighten the hydrated attribute-value ceiling.
    pub fn with_max_attribute_values(mut self, max_attribute_values: u64) -> Self {
        self.max_attribute_values = max_attribute_values.min(self.max_attribute_values);
        self
    }

    /// Further tighten the page collection-concept ceiling.
    pub fn with_max_collected_concepts(mut self, max_collected_concepts: u64) -> Self {
        self.max_collected_concepts = max_collected_concepts.min(self.max_collected_concepts);
        self
    }

    /// Further tighten the number of provider statements in one execution.
    pub fn with_max_statements(mut self, max_statements: u8) -> Self {
        self.max_statements = max_statements.min(self.max_statements);
        self
    }

    fn validation_limits(&self) -> ResultValidationLimits {
        ResultValidationLimits::new(
            usize::try_from(self.max_items).unwrap_or(usize::MAX),
            usize::try_from(self.max_items).unwrap_or(usize::MAX),
            usize::try_from(self.max_hydrated_things).unwrap_or(usize::MAX),
            usize::try_from(self.max_attribute_values).unwrap_or(usize::MAX),
            usize::try_from(self.max_collected_concepts).unwrap_or(usize::MAX),
            usize::try_from(self.max_bytes).unwrap_or(usize::MAX),
        )
    }

    fn semantic_limits(&self) -> SemanticItemLimits {
        SemanticItemLimits {
            hydrated_things: self.max_hydrated_things,
            attribute_values: self.max_attribute_values,
            collected_concepts: self.max_collected_concepts,
        }
    }
}

struct ExecutionBudget {
    remaining_items: u64,
    remaining_bytes: u64,
    deadline: Option<Instant>,
    cancellation: AnswerCancellation,
    statements: u8,
    max_statements: u8,
}

impl ExecutionBudget {
    fn new(limits: &MatchExecutionLimits, deadline: Option<Instant>) -> Self {
        Self {
            remaining_items: limits.max_items,
            remaining_bytes: limits.max_bytes,
            deadline,
            cancellation: limits.cancellation.clone(),
            statements: 0,
            max_statements: limits.max_statements,
        }
    }

    fn begin_statement(&mut self) -> Result<BoundedAnswerLimits, OrmError> {
        self.statements = self.statements.checked_add(1).ok_or_else(|| {
            resource_error(
                "statement_count_limit",
                "match execution statement counter overflowed",
            )
        })?;
        if self.statements > self.max_statements {
            return Err(resource_error(
                "statement_count_limit",
                "match execution exceeded its statement ceiling",
            ));
        }
        Ok(BoundedAnswerLimits {
            max_items: self.remaining_items,
            max_bytes: self.remaining_bytes,
            deadline: self.deadline,
            cancellation: self.cancellation.clone(),
        })
    }

    fn charge(&mut self, stats: crate::session::backend::BoundedAnswerStats) {
        self.remaining_items = self.remaining_items.saturating_sub(stats.processed_items);
        self.remaining_bytes = self.remaining_bytes.saturating_sub(stats.response_bytes);
    }

    fn check_before_await(&self) -> Result<(), OrmError> {
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
        future: impl Future<Output = Result<T, OrmError>>,
    ) -> Result<T, OrmError> {
        self.check_before_await()?;
        let result = self.race_provider(future).await;
        self.check_before_await()?;
        result
    }

    async fn await_cleanup<T>(
        &self,
        future: impl Future<Output = Result<T, OrmError>>,
    ) -> Result<T, OrmError> {
        // Cleanup must be polled once even when execution consumed the budget.
        // The result-first biased race invokes close, then cancellation or the
        // original deadline bounds a close future that remains pending.
        self.race_provider(future).await
    }

    async fn race_provider<T>(
        &self,
        future: impl Future<Output = Result<T, OrmError>>,
    ) -> Result<T, OrmError> {
        tokio::pin!(future);
        let cancellation = self.cancellation.cancelled();
        tokio::pin!(cancellation);

        if let Some(deadline) = self.deadline {
            let deadline = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
            tokio::pin!(deadline);
            tokio::select! {
                biased;
                result = &mut future => result,
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
                result = &mut future => result,
                () = &mut cancellation => Err(resource_error(
                    "provider_cancelled",
                    "provider answer processing was cancelled",
                )),
            }
        }
    }
}

impl Default for MatchExecutionLimits {
    fn default() -> Self {
        Self::tightened(
            MAX_PROCESSED_ITEMS,
            MAX_RESPONSE_BYTES,
            MAX_TRANSACTION_DURATION,
            AnswerCancellation::default(),
        )
    }
}

/// Internal selected-row executor used by owned and borrowed public entry seams.
pub(crate) struct SelectedResultExecutor<'a> {
    registry: &'a DescriptorRegistry,
    available_capabilities: CapabilitySet,
    limits: MatchExecutionLimits,
}

impl<'a> SelectedResultExecutor<'a> {
    pub(crate) fn new(
        registry: &'a DescriptorRegistry,
        available_capabilities: CapabilitySet,
        limits: MatchExecutionLimits,
    ) -> Self {
        Self {
            registry,
            available_capabilities,
            limits,
        }
    }

    pub(crate) async fn execute_owned(
        &self,
        database: &Database,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let statement = self.preflight(validated)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(self.limits.timeout)
            .map(tokio::time::Instant::into_std);
        let mut budget = ExecutionBudget::new(&self.limits, deadline);
        let mut transaction = budget
            .await_provider(async {
                database
                    .read_transaction()
                    .await
                    .map_err(provider_transaction_open_error)
            })
            .await?;
        let execution = match self
            .collect_from_transaction(&mut transaction, validated, statement, &mut budget)
            .await
        {
            Ok(evidence) => self.validate(validated, evidence, &budget),
            Err(error) => Err(error),
        };
        let close = budget
            .await_cleanup(async {
                transaction
                    .close()
                    .await
                    .map_err(provider_transaction_close_error)
            })
            .await;
        match (execution, close) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(result), Ok(())) => Ok(result),
        }
    }

    pub(crate) async fn execute_borrowed(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let statement = self.preflight(validated)?;
        if context.tx_type() != TxType::Read {
            return Err(MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "borrowed_target_not_read_only",
                "selected-row execution requires a borrowed read transaction",
            )
            .at(MatchErrorPathSegment::Operation)
            .into());
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(self.limits.timeout)
            .map(tokio::time::Instant::into_std);
        let mut budget = ExecutionBudget::new(&self.limits, deadline);
        let evidence = self
            .collect_from_context(context, validated, statement, &mut budget)
            .await?;
        self.validate(validated, evidence, &budget)
    }

    fn preflight(
        &self,
        validated: &ValidatedMatchRequest,
    ) -> Result<LoweredMatchExecution, OrmError> {
        validated.recheck_schema(self.registry)?;
        validated.require_capabilities(&self.available_capabilities)?;
        Ok(lower_match_execution(self.registry, validated)?)
    }

    async fn collect_from_transaction(
        &self,
        transaction: &mut Transaction,
        validated: &ValidatedMatchRequest,
        plan: LoweredMatchExecution,
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        match plan {
            LoweredMatchExecution::FetchRows(statement) => {
                self.collect_rows_transaction(transaction, validated, statement, budget)
                    .await
            }
            LoweredMatchExecution::CountBy { root, scan } => {
                let value = self
                    .scan_roots_transaction(transaction, scan, root, RootScanPurpose::Count, budget)
                    .await?
                    .len() as u64;
                Ok(ProviderResultEvidence::count(
                    validated.request_token(),
                    validated.shape_id().clone(),
                    root,
                    value,
                ))
            }
            LoweredMatchExecution::ExistsBy { root, scan } => {
                let value = !self
                    .scan_roots_transaction(
                        transaction,
                        scan,
                        root,
                        RootScanPurpose::Exists,
                        budget,
                    )
                    .await?
                    .is_empty();
                Ok(ProviderResultEvidence::exists(
                    validated.request_token(),
                    validated.shape_id().clone(),
                    root,
                    value,
                ))
            }
            LoweredMatchExecution::PageBy {
                root,
                total,
                selection,
                rematch,
            } => {
                self.collect_page_transaction(
                    transaction,
                    validated,
                    root,
                    total,
                    *selection,
                    rematch,
                    budget,
                )
                .await
            }
        }
    }

    async fn collect_from_context(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
        plan: LoweredMatchExecution,
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        match plan {
            LoweredMatchExecution::FetchRows(statement) => {
                self.collect_rows_context(context, validated, statement, budget)
                    .await
            }
            LoweredMatchExecution::CountBy { root, scan } => {
                let value = self
                    .scan_roots_context(context, scan, root, RootScanPurpose::Count, budget)
                    .await?
                    .len() as u64;
                Ok(ProviderResultEvidence::count(
                    validated.request_token(),
                    validated.shape_id().clone(),
                    root,
                    value,
                ))
            }
            LoweredMatchExecution::ExistsBy { root, scan } => {
                let value = !self
                    .scan_roots_context(context, scan, root, RootScanPurpose::Exists, budget)
                    .await?
                    .is_empty();
                Ok(ProviderResultEvidence::exists(
                    validated.request_token(),
                    validated.shape_id().clone(),
                    root,
                    value,
                ))
            }
            LoweredMatchExecution::PageBy {
                root,
                total,
                selection,
                rematch,
            } => {
                self.collect_page_context(
                    context, validated, root, total, *selection, rematch, budget,
                )
                .await
            }
        }
    }

    async fn collect_rows_transaction(
        &self,
        transaction: &mut Transaction,
        validated: &ValidatedMatchRequest,
        mut statement: TypedFetchRows,
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        statement.offset = 0;
        statement.limit = self.limits.max_items.max(1);
        let mut solutions = SolutionConsumer::new(validated)?;
        let limits = budget.begin_statement()?;
        let scan_limit = limits.max_items;
        let stats = budget
            .await_provider(async {
                transaction
                    .query_typed_bounded(&statement, limits, &mut solutions)
                    .await
                    .map_err(provider_statement_error)
            })
            .await?;
        budget.charge(stats);
        require_solution_scan_proof(stats, scan_limit)?;
        let solutions = solutions.finish();
        if solutions.is_empty() {
            return Ok(rows_evidence(validated, Vec::new()));
        }
        let batches = hydration_batches(self.registry, validated, &solutions)?;
        let mut hydration = HydrationConsumer::new(
            self.registry,
            validated,
            &solutions,
            self.limits.semantic_limits(),
        )?;
        for batch in batches {
            let limits = budget.begin_statement()?;
            let stats = budget
                .await_provider(async {
                    transaction
                        .hydrate_typed_bounded(&batch, limits, &mut hydration)
                        .await
                        .map_err(provider_statement_error)
                })
                .await?;
            budget.charge(stats);
        }
        rows_evidence_from_hydration(validated, solutions, hydration.finish()?)
    }

    async fn collect_rows_context(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
        mut statement: TypedFetchRows,
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        statement.offset = 0;
        statement.limit = self.limits.max_items.max(1);
        let mut solutions = SolutionConsumer::new(validated)?;
        let limits = budget.begin_statement()?;
        let scan_limit = limits.max_items;
        let stats = budget
            .await_provider(async {
                context
                    .query_typed_bounded(&statement, limits, &mut solutions)
                    .await
                    .map_err(provider_statement_error)
            })
            .await?;
        budget.charge(stats);
        require_solution_scan_proof(stats, scan_limit)?;
        let solutions = solutions.finish();
        if solutions.is_empty() {
            return Ok(rows_evidence(validated, Vec::new()));
        }
        let batches = hydration_batches(self.registry, validated, &solutions)?;
        let mut hydration = HydrationConsumer::new(
            self.registry,
            validated,
            &solutions,
            self.limits.semantic_limits(),
        )?;
        for batch in batches {
            let limits = budget.begin_statement()?;
            let stats = budget
                .await_provider(async {
                    context
                        .hydrate_typed_bounded(&batch, limits, &mut hydration)
                        .await
                        .map_err(provider_statement_error)
                })
                .await?;
            budget.charge(stats);
        }
        rows_evidence_from_hydration(validated, solutions, hydration.finish()?)
    }

    async fn scan_roots_transaction(
        &self,
        transaction: &mut Transaction,
        mut scan: TypedRootScan,
        root: BindingId,
        purpose: RootScanPurpose,
        budget: &mut ExecutionBudget,
    ) -> Result<Vec<String>, OrmError> {
        let limits = budget.begin_statement()?;
        let scan_limit = limits.max_items;
        if purpose == RootScanPurpose::Count {
            scan.limit = Some(scan_limit.max(1));
        }
        let mut roots = RootConsumer::new(root, purpose);
        let stats = budget
            .await_provider(async {
                transaction
                    .query_root_typed_bounded(&scan, limits, &mut roots)
                    .await
                    .map_err(provider_statement_error)
            })
            .await?;
        budget.charge(stats);
        require_solution_scan_proof(stats, scan_limit)?;
        Ok(roots.finish())
    }

    async fn scan_roots_context(
        &self,
        context: &TransactionContext,
        mut scan: TypedRootScan,
        root: BindingId,
        purpose: RootScanPurpose,
        budget: &mut ExecutionBudget,
    ) -> Result<Vec<String>, OrmError> {
        let limits = budget.begin_statement()?;
        let scan_limit = limits.max_items;
        if purpose == RootScanPurpose::Count {
            scan.limit = Some(scan_limit.max(1));
        }
        let mut roots = RootConsumer::new(root, purpose);
        let stats = budget
            .await_provider(async {
                context
                    .query_root_typed_bounded(&scan, limits, &mut roots)
                    .await
                    .map_err(provider_statement_error)
            })
            .await?;
        budget.charge(stats);
        require_solution_scan_proof(stats, scan_limit)?;
        Ok(roots.finish())
    }

    #[allow(clippy::too_many_arguments)]
    async fn collect_page_transaction(
        &self,
        transaction: &mut Transaction,
        validated: &ValidatedMatchRequest,
        root: BindingId,
        total_scan: Option<TypedRootScan>,
        selection: TypedRootScan,
        mut rematch: TypedPageRematch,
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        let total = if let Some(scan) = total_scan {
            Some(
                self.scan_roots_transaction(transaction, scan, root, RootScanPurpose::Count, budget)
                    .await?
                    .len() as u64,
            )
        } else {
            None
        };
        let window = page_window(validated)?;
        let roots = self
            .scan_roots_transaction(
                transaction,
                selection,
                root,
                RootScanPurpose::Page(window.limit),
                budget,
            )
            .await?;
        if roots.is_empty() {
            return Ok(page_evidence(
                validated,
                root,
                &roots,
                Vec::new(),
                window,
                total,
            ));
        }
        rematch.root_concept_ids.clone_from(&roots);
        let mut consumer = PageRematchConsumer::new(
            self.registry,
            validated,
            root,
            &roots,
            self.limits.semantic_limits(),
        )?;
        let limits = budget.begin_statement()?;
        let scan_limit = limits.max_items;
        let stats = budget
            .await_provider(async {
                transaction
                    .rematch_page_typed_bounded(&rematch, limits, &mut consumer)
                    .await
                    .map_err(provider_statement_error)
            })
            .await?;
        budget.charge(stats);
        require_solution_scan_proof(stats, scan_limit)?;
        let solutions = consumer.finish()?;
        Ok(page_evidence(
            validated, root, &roots, solutions, window, total,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn collect_page_context(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
        root: BindingId,
        total_scan: Option<TypedRootScan>,
        selection: TypedRootScan,
        mut rematch: TypedPageRematch,
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        let total = if let Some(scan) = total_scan {
            Some(
                self.scan_roots_context(context, scan, root, RootScanPurpose::Count, budget)
                    .await?
                    .len() as u64,
            )
        } else {
            None
        };
        let window = page_window(validated)?;
        let roots = self
            .scan_roots_context(
                context,
                selection,
                root,
                RootScanPurpose::Page(window.limit),
                budget,
            )
            .await?;
        if roots.is_empty() {
            return Ok(page_evidence(
                validated,
                root,
                &roots,
                Vec::new(),
                window,
                total,
            ));
        }
        rematch.root_concept_ids.clone_from(&roots);
        let mut consumer = PageRematchConsumer::new(
            self.registry,
            validated,
            root,
            &roots,
            self.limits.semantic_limits(),
        )?;
        let limits = budget.begin_statement()?;
        let scan_limit = limits.max_items;
        let stats = budget
            .await_provider(async {
                context
                    .rematch_page_typed_bounded(&rematch, limits, &mut consumer)
                    .await
                    .map_err(provider_statement_error)
            })
            .await?;
        budget.charge(stats);
        require_solution_scan_proof(stats, scan_limit)?;
        let solutions = consumer.finish()?;
        Ok(page_evidence(
            validated, root, &roots, solutions, window, total,
        ))
    }

    fn validate(
        &self,
        validated: &ValidatedMatchRequest,
        evidence: ProviderResultEvidence,
        budget: &ExecutionBudget,
    ) -> Result<ValidatedMatchResult, OrmError> {
        budget.check_before_await()?;
        let result = validate_provider_result_with_limits(
            self.registry,
            validated,
            evidence,
            self.limits.validation_limits(),
        );
        budget.check_before_await()?;
        Ok(result?)
    }
}

fn require_solution_scan_proof(
    stats: crate::session::backend::BoundedAnswerStats,
    max_items: u64,
) -> Result<(), OrmError> {
    if !stats.stopped_early && stats.processed_items >= max_items {
        return Err(MatchError::new(
            MatchErrorCategory::ResourceLimit,
            "solution_scan_limit",
            "provider solution ceiling was reached before result completeness was proven",
        )
        .at(MatchErrorPathSegment::ProviderEvidence)
        .with_detail("limit", max_items)
        .into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct SemanticItemLimits {
    hydrated_things: u64,
    attribute_values: u64,
    collected_concepts: u64,
}

struct SemanticBudget {
    limits: SemanticItemLimits,
    hydrated_things: u64,
    attribute_values: u64,
    collected_concepts: u64,
}

impl SemanticBudget {
    fn new(limits: SemanticItemLimits) -> Self {
        Self {
            limits,
            hydrated_things: 0,
            attribute_values: 0,
            collected_concepts: 0,
        }
    }

    fn charge_thing(&mut self, thing: &HydratedThing, multiplicity: u64) -> Result<(), OrmError> {
        let (hydrated_things, attribute_values) = semantic_counts(thing)?;
        let hydrated_things = hydrated_things.checked_mul(multiplicity).ok_or_else(|| {
            resource_error(
                "hydrated_thing_limit",
                "hydrated thing multiplicity counter overflowed",
            )
        })?;
        let attribute_values = attribute_values.checked_mul(multiplicity).ok_or_else(|| {
            resource_error(
                "hydrated_attribute_value_limit",
                "hydrated attribute value multiplicity counter overflowed",
            )
        })?;
        self.charge(hydrated_things, attribute_values, 0)
    }

    fn charge_solution(
        &mut self,
        bindings: &BTreeMap<BindingId, HydratedThing>,
        collected_concepts: u64,
    ) -> Result<(), OrmError> {
        let mut hydrated_things = 0_u64;
        let mut attribute_values = 0_u64;
        for thing in bindings.values() {
            let (thing_count, value_count) = semantic_counts(thing)?;
            hydrated_things = hydrated_things.checked_add(thing_count).ok_or_else(|| {
                resource_error("hydrated_thing_limit", "hydrated thing counter overflowed")
            })?;
            attribute_values = attribute_values.checked_add(value_count).ok_or_else(|| {
                resource_error(
                    "hydrated_attribute_value_limit",
                    "hydrated attribute value counter overflowed",
                )
            })?;
        }
        self.charge(hydrated_things, attribute_values, collected_concepts)
    }

    fn charge(
        &mut self,
        hydrated_things: u64,
        attribute_values: u64,
        collected_concepts: u64,
    ) -> Result<(), OrmError> {
        let next_hydrated = self
            .hydrated_things
            .checked_add(hydrated_things)
            .ok_or_else(|| {
                resource_error("hydrated_thing_limit", "hydrated thing counter overflowed")
            })?;
        let next_attributes = self
            .attribute_values
            .checked_add(attribute_values)
            .ok_or_else(|| {
                resource_error(
                    "hydrated_attribute_value_limit",
                    "hydrated attribute value counter overflowed",
                )
            })?;
        let next_collected = self
            .collected_concepts
            .checked_add(collected_concepts)
            .ok_or_else(|| {
                resource_error(
                    "collected_concept_limit",
                    "collected concept counter overflowed",
                )
            })?;
        if next_hydrated > self.limits.hydrated_things {
            return Err(resource_error(
                "hydrated_thing_limit",
                "provider hydration exceeded the hydrated-thing ceiling",
            ));
        }
        if next_attributes > self.limits.attribute_values {
            return Err(resource_error(
                "hydrated_attribute_value_limit",
                "provider hydration exceeded the attribute-value ceiling",
            ));
        }
        if next_collected > self.limits.collected_concepts {
            return Err(resource_error(
                "collected_concept_limit",
                "page re-match exceeded the collected-concept ceiling",
            ));
        }
        self.hydrated_things = next_hydrated;
        self.attribute_values = next_attributes;
        self.collected_concepts = next_collected;
        Ok(())
    }
}

fn semantic_counts(thing: &HydratedThing) -> Result<(u64, u64), OrmError> {
    let mut hydrated_things = 1_u64;
    let mut attribute_values = count_attribute_values(thing.attributes())?;
    for role in thing.roles() {
        for player in role.players() {
            hydrated_things = hydrated_things.checked_add(1).ok_or_else(|| {
                resource_error("hydrated_thing_limit", "hydrated thing counter overflowed")
            })?;
            attribute_values = attribute_values
                .checked_add(count_attribute_values(player.attributes())?)
                .ok_or_else(|| {
                    resource_error(
                        "hydrated_attribute_value_limit",
                        "hydrated attribute value counter overflowed",
                    )
                })?;
        }
    }
    Ok((hydrated_things, attribute_values))
}

fn count_attribute_values(attributes: &[HydratedAttribute]) -> Result<u64, OrmError> {
    attributes.iter().try_fold(0_u64, |count, attribute| {
        let values = u64::try_from(attribute.values().len()).map_err(|_| {
            resource_error(
                "hydrated_attribute_value_limit",
                "hydrated attribute value count exceeds the counter range",
            )
        })?;
        count.checked_add(values).ok_or_else(|| {
            resource_error(
                "hydrated_attribute_value_limit",
                "hydrated attribute value counter overflowed",
            )
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootScanPurpose {
    Count,
    Exists,
    Page(u64),
}

struct RootConsumer {
    root: BindingId,
    purpose: RootScanPurpose,
    seen: BTreeSet<String>,
    roots: Vec<String>,
}

impl RootConsumer {
    fn new(root: BindingId, purpose: RootScanPurpose) -> Self {
        Self {
            root,
            purpose,
            seen: BTreeSet::new(),
            roots: Vec::new(),
        }
    }

    fn finish(self) -> Vec<String> {
        self.roots
    }
}

impl AnswerConsumer for RootConsumer {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        let AnswerItem::Row(value) = item else {
            return Err(decode_error(
                "root_answer_kind",
                "distinct-root statement returned a document instead of a row",
            ));
        };
        let wire: SolutionWire = serde_json::from_value(value).map_err(|error| {
            decode_error_owned(
                "malformed_root_row",
                format!("invalid distinct-root row: {error}"),
            )
        })?;
        if !wire.satisfied_role_edges.is_empty() || wire.bindings.len() != 1 {
            return Err(decode_error(
                "malformed_root_row",
                "distinct-root row must contain exactly the root binding",
            ));
        }
        let assignment = &wire.bindings[0];
        if BindingId::new(assignment.binding) != self.root {
            return Err(decode_error(
                "root_binding_mismatch",
                "distinct-root row contains the wrong binding",
            ));
        }
        validate_provider_iid(&assignment.concept_id)?;
        if self.seen.insert(assignment.concept_id.clone()) {
            self.roots.push(assignment.concept_id.clone());
        }
        let complete = match self.purpose {
            RootScanPurpose::Count => false,
            RootScanPurpose::Exists => !self.roots.is_empty(),
            RootScanPurpose::Page(limit) => self.roots.len() as u64 >= limit,
        };
        Ok(if complete {
            AnswerControl::Stop
        } else {
            AnswerControl::Continue
        })
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

#[derive(Debug)]
struct UnhydratedSolution {
    bindings: BTreeMap<BindingId, String>,
    satisfied_role_edges: Vec<RoleEdgeId>,
}

struct SolutionConsumer {
    expected: BTreeSet<BindingId>,
    selected: Vec<BindingId>,
    stop_after: u64,
    seen: BTreeSet<Vec<String>>,
    solutions: Vec<UnhydratedSolution>,
}

impl SolutionConsumer {
    fn new(validated: &ValidatedMatchRequest) -> Result<Self, OrmError> {
        let MatchOperation::FetchRows {
            output,
            window,
            cardinality,
            ..
        } = &validated.request().operation
        else {
            return Err(decode_error(
                "unsupported_selected_operation",
                "selected solution decoder supports only FetchRows",
            ));
        };
        let selected = output_slots(output)
            .map(|slot| match slot {
                FetchSlot::One { binding } => Ok(*binding),
                FetchSlot::Collect { .. } => Err(decode_error(
                    "unsupported_collection_slot",
                    "selected solution decoder does not support collection slots",
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let stop_after = match cardinality {
            RowCardinality::ExactlyOne => 2,
            RowCardinality::BoundedMany => window.offset.saturating_add(window.limit),
        };
        Ok(Self {
            expected: validated
                .request()
                .plan
                .bindings
                .iter()
                .map(|binding| binding.id)
                .collect(),
            selected,
            stop_after,
            seen: BTreeSet::new(),
            solutions: Vec::new(),
        })
    }

    fn finish(self) -> Vec<UnhydratedSolution> {
        self.solutions
    }
}

impl AnswerConsumer for SolutionConsumer {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        let AnswerItem::Row(value) = item else {
            return Err(decode_error(
                "solution_answer_kind",
                "selected solution statement returned a document instead of a row",
            ));
        };
        let wire: SolutionWire = serde_json::from_value(value).map_err(|error| {
            decode_error_owned(
                "malformed_solution_row",
                format!("invalid solution row: {error}"),
            )
        })?;
        let mut bindings = BTreeMap::new();
        for assignment in wire.bindings {
            let binding = BindingId::new(assignment.binding);
            if !self.expected.contains(&binding) {
                return Err(decode_error(
                    "unknown_provider_binding",
                    "provider solution contains an unknown binding",
                ));
            }
            validate_provider_iid(&assignment.concept_id)?;
            if bindings.insert(binding, assignment.concept_id).is_some() {
                return Err(decode_error(
                    "duplicate_provider_binding",
                    "provider solution assigns one binding more than once",
                ));
            }
        }
        if bindings.len() != self.expected.len() {
            return Err(decode_error(
                "missing_provider_binding",
                "provider solution omits a positive binding",
            ));
        }
        let identity = self
            .selected
            .iter()
            .map(|binding| bindings[binding].clone())
            .collect::<Vec<_>>();
        if self.seen.insert(identity) {
            self.solutions.push(UnhydratedSolution {
                bindings,
                satisfied_role_edges: wire
                    .satisfied_role_edges
                    .into_iter()
                    .map(RoleEdgeId::new)
                    .collect(),
            });
        }
        Ok(if self.solutions.len() as u64 >= self.stop_after {
            AnswerControl::Stop
        } else {
            AnswerControl::Continue
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HydratedThingWire {
    binding: u16,
    concept_id: String,
    concrete_type: String,
    kind: ThingKind,
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
    values: Vec<Value>,
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
    kind: ThingKind,
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

struct HydrationConsumer<'a> {
    registry: &'a DescriptorRegistry,
    declared: BTreeMap<BindingId, DescriptorId>,
    expected: BTreeSet<(BindingId, String)>,
    multiplicity: BTreeMap<(BindingId, String), u64>,
    semantic_budget: SemanticBudget,
    hydrated: BTreeMap<(BindingId, String), HydratedThing>,
}

impl<'a> HydrationConsumer<'a> {
    fn decoder(
        registry: &'a DescriptorRegistry,
        validated: &ValidatedMatchRequest,
        semantic_limits: SemanticItemLimits,
    ) -> Self {
        Self {
            registry,
            declared: validated
                .request()
                .plan
                .bindings
                .iter()
                .map(|binding| (binding.id, binding.descriptor.clone()))
                .collect(),
            expected: BTreeSet::new(),
            multiplicity: BTreeMap::new(),
            semantic_budget: SemanticBudget::new(semantic_limits),
            hydrated: BTreeMap::new(),
        }
    }

    fn new(
        registry: &'a DescriptorRegistry,
        validated: &ValidatedMatchRequest,
        solutions: &[UnhydratedSolution],
        semantic_limits: SemanticItemLimits,
    ) -> Result<Self, OrmError> {
        let mut decoder = Self::decoder(registry, validated, semantic_limits);
        // Hydration is de-duplicated by binding/IID, but validated evidence
        // materializes that thing once for every retained solution assignment.
        for solution in solutions {
            for (binding, concept) in &solution.bindings {
                let multiplicity = decoder
                    .multiplicity
                    .entry((*binding, concept.clone()))
                    .or_default();
                *multiplicity = multiplicity.checked_add(1).ok_or_else(|| {
                    resource_error(
                        "hydrated_thing_limit",
                        "hydration multiplicity counter overflowed",
                    )
                })?;
            }
        }
        decoder.expected = decoder.multiplicity.keys().cloned().collect();
        Ok(decoder)
    }

    fn finish(self) -> Result<BTreeMap<(BindingId, String), HydratedThing>, OrmError> {
        if self.hydrated.len() != self.expected.len() {
            return Err(decode_error(
                "missing_hydrated_concept",
                "batched hydration omitted a requested binding/IID pair",
            ));
        }
        Ok(self.hydrated)
    }

    fn decode_thing(&self, wire: HydratedThingWire) -> Result<HydratedThing, OrmError> {
        let binding = BindingId::new(wire.binding);
        let declared = self.declared.get(&binding).cloned().ok_or_else(|| {
            decode_error(
                "unknown_hydrated_binding",
                "hydration returned an undeclared binding",
            )
        })?;
        let concrete = self
            .registry
            .descriptor_id(&wire.concrete_type)
            .ok_or_else(|| {
                decode_error(
                    "unknown_hydrated_descriptor",
                    "hydration returned an unregistered concrete type",
                )
            })?;
        let attributes = decode_attributes(concrete.clone(), wire.attributes)?;
        let roles = wire
            .roles
            .into_iter()
            .map(|role| decode_role(self.registry, concrete.clone(), role))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(HydratedThing::new(
            ConceptId::new(wire.concept_id),
            declared,
            concrete,
            wire.kind,
            attributes,
            roles,
        ))
    }

    fn decode_document(
        &self,
        value: Value,
    ) -> Result<((BindingId, String), HydratedThing), OrmError> {
        if let Ok(wire) = serde_json::from_value::<HydratedThingWire>(value.clone()) {
            let key = (BindingId::new(wire.binding), wire.concept_id.clone());
            let thing = self.decode_thing(wire)?;
            return Ok((key, thing));
        }

        let document = value.as_object().ok_or_else(|| {
            decode_error(
                "malformed_hydration_document",
                "TypeDB hydration document is not an object",
            )
        })?;
        let binding = document
            .get("binding")
            .and_then(json_u16)
            .map(BindingId::new)
            .ok_or_else(|| {
                decode_error(
                    "malformed_hydration_document",
                    "TypeDB hydration document has no binding ordinal",
                )
            })?;
        let concept_id = document
            .get("concept_id")
            .and_then(json_string)
            .ok_or_else(|| {
                decode_error(
                    "malformed_hydration_document",
                    "TypeDB hydration document has no concept IID",
                )
            })?
            .to_owned();
        let concrete_type = document
            .get("concrete_type")
            .and_then(json_string)
            .ok_or_else(|| {
                decode_error(
                    "malformed_hydration_document",
                    "TypeDB hydration document has no concrete type label",
                )
            })?;
        let declared = self.declared.get(&binding).cloned().ok_or_else(|| {
            decode_error(
                "unknown_hydrated_binding",
                "hydration returned an undeclared binding",
            )
        })?;
        let concrete = self.registry.descriptor_id(concrete_type).ok_or_else(|| {
            decode_error(
                "unknown_hydrated_descriptor",
                "hydration returned an unregistered concrete type",
            )
        })?;
        let descriptor = self.registry.get(concrete_type).ok_or_else(|| {
            decode_error(
                "unknown_hydrated_descriptor",
                "hydration returned an unregistered concrete type",
            )
        })?;
        let attributes =
            decode_wildcard_attributes(&concrete, &descriptor, document.get("attributes"))?;
        let (kind, roles) = match descriptor {
            TypeDescriptorRef::Entity(_) => (ThingKind::Entity, Vec::new()),
            TypeDescriptorRef::Relation(relation) => (
                ThingKind::Relation,
                decode_live_roles(
                    self.registry,
                    &concrete,
                    &relation.roles,
                    document.get("roles"),
                )?,
            ),
        };
        let thing = HydratedThing::new(
            ConceptId::new(concept_id.clone()),
            declared,
            concrete,
            kind,
            attributes,
            roles,
        );
        Ok(((binding, concept_id), thing))
    }
}

impl AnswerConsumer for HydrationConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        let AnswerItem::Document(value) = item else {
            return Err(decode_error(
                "hydration_answer_kind",
                "batched hydration returned a row instead of a document",
            ));
        };
        let (key, thing) = self.decode_document(value)?;
        if !self.expected.contains(&key) {
            return Err(decode_error(
                "unexpected_hydrated_concept",
                "batched hydration returned an unrequested binding/IID pair",
            ));
        }
        if self.hydrated.contains_key(&key) {
            return Err(decode_error(
                "duplicate_hydrated_concept",
                "batched hydration returned one binding/IID pair more than once",
            ));
        }
        let multiplicity = self.multiplicity[&key];
        self.semantic_budget.charge_thing(&thing, multiplicity)?;
        self.hydrated.insert(key, thing);
        Ok(AnswerControl::Continue)
    }
}

type CollectedIdentity = (String, BindingId, String);

struct PageRematchConsumer<'a> {
    validated: &'a ValidatedMatchRequest,
    root: BindingId,
    selected_roots: Vec<String>,
    selected_set: BTreeSet<String>,
    seen_roots: BTreeSet<String>,
    expected_bindings: BTreeSet<BindingId>,
    collections: Vec<(BindingId, bool)>,
    collected_distinct: BTreeSet<CollectedIdentity>,
    semantic_budget: SemanticBudget,
    decoder: HydrationConsumer<'a>,
    solutions: Vec<(String, ProviderSolutionEvidence)>,
}

impl<'a> PageRematchConsumer<'a> {
    fn new(
        registry: &'a DescriptorRegistry,
        validated: &'a ValidatedMatchRequest,
        root: BindingId,
        selected_roots: &[String],
        semantic_limits: SemanticItemLimits,
    ) -> Result<Self, OrmError> {
        let selected_set = selected_roots.iter().cloned().collect::<BTreeSet<_>>();
        if selected_set.len() != selected_roots.len() {
            return Err(decode_error(
                "duplicate_selected_root",
                "root selection returned one identity more than once",
            ));
        }
        let MatchOperation::PageBy { output, .. } = &validated.request().operation else {
            return Err(decode_error(
                "page_operation_mismatch",
                "page re-match consumer requires a PageBy operation",
            ));
        };
        let collections = output_slots(output)
            .filter_map(|slot| match slot {
                FetchSlot::One { .. } => None,
                FetchSlot::Collect {
                    binding, distinct, ..
                } => Some((*binding, *distinct)),
            })
            .collect();
        Ok(Self {
            validated,
            root,
            selected_roots: selected_roots.to_vec(),
            selected_set,
            seen_roots: BTreeSet::new(),
            expected_bindings: validated
                .request()
                .plan
                .bindings
                .iter()
                .map(|binding| binding.id)
                .collect(),
            collections,
            collected_distinct: BTreeSet::new(),
            semantic_budget: SemanticBudget::new(semantic_limits),
            decoder: HydrationConsumer::decoder(registry, validated, semantic_limits),
            solutions: Vec::new(),
        })
    }

    fn collection_charge(
        &self,
        root_id: &str,
        bindings: &BTreeMap<BindingId, HydratedThing>,
    ) -> Result<(u64, Vec<CollectedIdentity>), OrmError> {
        let mut count = 0_u64;
        let mut newly_distinct = Vec::new();
        for (binding, distinct) in &self.collections {
            let concept_id = bindings
                .get(binding)
                .expect("complete page binding assignment")
                .concept_id()
                .as_str()
                .to_owned();
            if *distinct {
                // Distinctness is local to one root and one collection slot;
                // the same concept under another root is another output item.
                let key = (root_id.to_owned(), *binding, concept_id);
                if self.collected_distinct.contains(&key) || newly_distinct.contains(&key) {
                    continue;
                }
                newly_distinct.push(key);
            }
            count = count.checked_add(1).ok_or_else(|| {
                resource_error(
                    "collected_concept_limit",
                    "collected concept counter overflowed",
                )
            })?;
        }
        Ok((count, newly_distinct))
    }

    fn decode_solution(
        &self,
        value: Value,
    ) -> Result<(BTreeMap<BindingId, HydratedThing>, Vec<RoleEdgeId>), OrmError> {
        if let Ok(wire) = serde_json::from_value::<RematchSolutionWire>(value.clone()) {
            let mut bindings = BTreeMap::new();
            for thing in wire.bindings {
                validate_provider_iid(&thing.concept_id)?;
                let binding = BindingId::new(thing.binding);
                if !self.expected_bindings.contains(&binding) {
                    return Err(decode_error(
                        "unknown_provider_binding",
                        "page re-match contains an unknown binding",
                    ));
                }
                if bindings
                    .insert(binding, self.decoder.decode_thing(thing)?)
                    .is_some()
                {
                    return Err(decode_error(
                        "duplicate_provider_binding",
                        "page re-match assigns one binding more than once",
                    ));
                }
            }
            if bindings.len() != self.expected_bindings.len() {
                return Err(decode_error(
                    "missing_provider_binding",
                    "page re-match omits a positive binding",
                ));
            }
            return Ok((
                bindings,
                wire.satisfied_role_edges
                    .into_iter()
                    .map(RoleEdgeId::new)
                    .collect(),
            ));
        }

        let document = value.as_object().ok_or_else(|| {
            decode_error(
                "malformed_page_rematch_document",
                "page re-match document is not an object",
            )
        })?;
        if document.len() != self.expected_bindings.len() {
            return Err(decode_error(
                "malformed_page_rematch_document",
                "page re-match document has missing or extra binding members",
            ));
        }
        let mut bindings = BTreeMap::new();
        for binding in &self.expected_bindings {
            let key = format!("b{}", binding.get());
            let nested = document.get(&key).ok_or_else(|| {
                decode_error(
                    "missing_provider_binding",
                    "page re-match document omits a positive binding",
                )
            })?;
            let mut nested = nested.as_object().cloned().ok_or_else(|| {
                decode_error(
                    "malformed_page_rematch_binding",
                    "page re-match binding member is not an object",
                )
            })?;
            nested.insert("binding".into(), serde_json::json!(binding.get()));
            let ((actual_binding, concept_id), thing) =
                self.decoder.decode_document(Value::Object(nested))?;
            if actual_binding != *binding {
                return Err(decode_error(
                    "provider_binding_mismatch",
                    "page re-match binding member has the wrong ordinal",
                ));
            }
            validate_provider_iid(&concept_id)?;
            bindings.insert(*binding, thing);
        }
        Ok((bindings, Vec::new()))
    }

    fn finish(self) -> Result<Vec<ProviderSolutionEvidence>, OrmError> {
        if self.seen_roots != self.selected_set {
            return Err(decode_error(
                "selected_root_set_mismatch",
                "page re-match root set does not exactly equal root selection",
            ));
        }
        let mut grouped: BTreeMap<String, Vec<ProviderSolutionEvidence>> = BTreeMap::new();
        for (root, solution) in self.solutions {
            grouped.entry(root).or_default().push(solution);
        }
        let mut ordered = Vec::new();
        for root in self.selected_roots {
            ordered.extend(grouped.remove(&root).unwrap_or_default());
        }
        Ok(ordered)
    }
}

impl AnswerConsumer for PageRematchConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        let AnswerItem::Document(value) = item else {
            return Err(decode_error(
                "page_rematch_answer_kind",
                "page re-match returned a row instead of a hydrated document",
            ));
        };
        let (bindings, claimed_edges) = self.decode_solution(value)?;
        let root_id = bindings
            .get(&self.root)
            .expect("complete page binding assignment")
            .concept_id()
            .as_str()
            .to_owned();
        if !self.selected_set.contains(&root_id) {
            return Err(decode_error(
                "unexpected_hydrated_root",
                "page re-match returned a root outside the selected identity set",
            ));
        }
        let (collected_concepts, newly_distinct) = self.collection_charge(&root_id, &bindings)?;
        self.semantic_budget
            .charge_solution(&bindings, collected_concepts)?;
        self.collected_distinct.extend(newly_distinct);
        self.seen_roots.insert(root_id.clone());
        let evidence = provider_solution_from_hydrated(self.validated, bindings, claimed_edges)?;
        self.solutions.push((root_id, evidence));
        Ok(AnswerControl::Continue)
    }
}

fn decode_wildcard_attributes(
    owner: &DescriptorId,
    descriptor: &TypeDescriptorRef,
    value: Option<&Value>,
) -> Result<Vec<HydratedAttribute>, OrmError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let object = value.as_object().ok_or_else(|| {
        decode_error(
            "malformed_hydrated_attributes",
            "TypeDB wildcard hydration attributes are not an object",
        )
    })?;
    let mut attributes = Vec::new();
    for (name, raw) in object {
        let field = match descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(name),
            TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(name),
        }
        .ok_or_else(|| {
            decode_error(
                "unknown_hydrated_attribute",
                "TypeDB wildcard hydration returned an undeclared attribute",
            )
        })?;
        let raw_values = raw
            .as_array()
            .map_or_else(|| vec![raw], |values| values.iter().collect());
        let values = raw_values
            .into_iter()
            .filter(|value| !value.is_null())
            .map(|value| {
                AttributeValue::from_json(value, field.value_type.as_str()).ok_or_else(|| {
                    decode_error(
                        "hydrated_attribute_value_type",
                        "TypeDB wildcard attribute value has the wrong value type",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        attributes.push(HydratedAttribute::new(
            FieldId::new(owner.clone(), field.field_name.clone()),
            values,
        ));
    }
    attributes.sort_by(|left, right| left.field().cmp(right.field()));
    Ok(attributes)
}

fn decode_live_roles(
    registry: &DescriptorRegistry,
    owner: &DescriptorId,
    descriptors: &[crate::descriptor::RoleDescriptor],
    value: Option<&Value>,
) -> Result<Vec<HydratedRole>, OrmError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let documents = value.as_array().ok_or_else(|| {
        decode_error(
            "malformed_hydrated_roles",
            "TypeDB nested role hydration is not a list",
        )
    })?;
    let mut roles: BTreeMap<String, Vec<HydratedRolePlayer>> = BTreeMap::new();
    for document in documents {
        let document = document.as_object().ok_or_else(|| {
            decode_error(
                "malformed_hydrated_roles",
                "TypeDB nested role hydration member is not an object",
            )
        })?;
        let role_label = document.get("role").and_then(json_string).ok_or_else(|| {
            decode_error(
                "malformed_hydrated_roles",
                "TypeDB nested role hydration omitted the role label",
            )
        })?;
        let role_name = role_label.rsplit(':').next().unwrap_or(role_label);
        let role = descriptors
            .iter()
            .find(|role| role.role_name == role_name)
            .ok_or_else(|| {
                decode_error(
                    "unknown_hydrated_role",
                    "TypeDB nested hydration returned an undeclared relation role",
                )
            })?;
        let concept_id = document
            .get("player_concept_id")
            .and_then(json_string)
            .ok_or_else(|| {
                decode_error(
                    "malformed_hydrated_role_player",
                    "TypeDB nested hydration omitted the player IID",
                )
            })?;
        let concrete_type = document
            .get("player_concrete_type")
            .and_then(json_string)
            .ok_or_else(|| {
                decode_error(
                    "malformed_hydrated_role_player",
                    "TypeDB nested hydration omitted the player concrete type",
                )
            })?;
        let concrete = registry.descriptor_id(concrete_type).ok_or_else(|| {
            decode_error(
                "unknown_role_player_descriptor",
                "TypeDB nested hydration returned an unregistered player type",
            )
        })?;
        let player_descriptor = registry.get(concrete_type).ok_or_else(|| {
            decode_error(
                "unknown_role_player_descriptor",
                "TypeDB nested hydration returned an unregistered player type",
            )
        })?;
        let kind = match &player_descriptor {
            TypeDescriptorRef::Entity(_) => ThingKind::Entity,
            TypeDescriptorRef::Relation(_) => ThingKind::Relation,
        };
        let attributes =
            decode_wildcard_attributes(&concrete, &player_descriptor, document.get("attributes"))?;
        let declared_type = role
            .player_type_names
            .iter()
            .find(|declared| is_registered_type_or_subtype(registry, concrete_type, declared))
            .map(String::as_str)
            .unwrap_or(concrete_type);
        let declared = registry.descriptor_id(declared_type).ok_or_else(|| {
            decode_error(
                "unknown_role_player_descriptor",
                "TypeDB nested hydration player has no compatible declared type",
            )
        })?;
        roles
            .entry(role_name.to_owned())
            .or_default()
            .push(HydratedRolePlayer::new(
                ConceptId::new(concept_id),
                declared,
                concrete,
                kind,
                attributes,
            ));
    }
    Ok(roles
        .into_iter()
        .map(|(role, players)| HydratedRole::new(RoleId::new(owner.clone(), role), players))
        .collect())
}

fn is_registered_type_or_subtype(
    registry: &DescriptorRegistry,
    actual: &str,
    expected: &str,
) -> bool {
    let mut current = Some(actual.to_owned());
    let mut visited = BTreeSet::new();
    while let Some(name) = current {
        if name == expected {
            return true;
        }
        if !visited.insert(name.clone()) {
            return false;
        }
        current = registry.get(&name).and_then(|descriptor| match descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.parent_type.clone(),
            TypeDescriptorRef::Relation(descriptor) => descriptor.parent_type.clone(),
        });
    }
    false
}

fn json_string(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("value").and_then(json_string))
}

fn json_u16(value: &Value) -> Option<u16> {
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
        .or_else(|| value.get("value").and_then(json_u16))
}

fn decode_attributes(
    owner: DescriptorId,
    attributes: Vec<HydratedAttributeWire>,
) -> Result<Vec<HydratedAttribute>, OrmError> {
    attributes
        .into_iter()
        .map(|attribute| {
            let values = attribute
                .values
                .iter()
                .map(|value| {
                    AttributeValue::from_json(value, &attribute.value_type).ok_or_else(|| {
                        decode_error(
                            "hydrated_attribute_value_type",
                            "hydrated attribute value does not match its value type",
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HydratedAttribute::new(
                FieldId::new(owner.clone(), attribute.field),
                values,
            ))
        })
        .collect()
}

fn decode_role(
    registry: &DescriptorRegistry,
    owner: DescriptorId,
    role: HydratedRoleWire,
) -> Result<HydratedRole, OrmError> {
    let players = role
        .players
        .into_iter()
        .map(|player| {
            let declared = registry
                .descriptor_id(&player.declared_type)
                .ok_or_else(|| {
                    decode_error(
                        "unknown_role_player_descriptor",
                        "hydrated role player has an unregistered declared type",
                    )
                })?;
            let concrete = registry
                .descriptor_id(&player.concrete_type)
                .ok_or_else(|| {
                    decode_error(
                        "unknown_role_player_descriptor",
                        "hydrated role player has an unregistered concrete type",
                    )
                })?;
            let attributes = decode_attributes(concrete.clone(), player.attributes)?;
            Ok(HydratedRolePlayer::new(
                ConceptId::new(player.concept_id),
                declared,
                concrete,
                player.kind,
                attributes,
            ))
        })
        .collect::<Result<Vec<_>, OrmError>>()?;
    Ok(HydratedRole::new(RoleId::new(owner, role.role), players))
}

fn rows_evidence(
    validated: &ValidatedMatchRequest,
    solutions: Vec<ProviderSolutionEvidence>,
) -> ProviderResultEvidence {
    ProviderResultEvidence::rows_unwindowed(
        validated.request_token(),
        validated.shape_id().clone(),
        solutions,
    )
}

fn page_evidence(
    validated: &ValidatedMatchRequest,
    root: BindingId,
    selected_roots: &[String],
    solutions: Vec<ProviderSolutionEvidence>,
    window: Window,
    total: Option<u64>,
) -> ProviderResultEvidence {
    ProviderResultEvidence::page_selected(
        validated.request_token(),
        validated.shape_id().clone(),
        root,
        selected_roots
            .iter()
            .map(|root| ConceptId::new(root.clone()))
            .collect(),
        solutions,
        window,
        total,
    )
}

fn page_window(validated: &ValidatedMatchRequest) -> Result<Window, OrmError> {
    let MatchOperation::PageBy { window, .. } = validated.request().operation else {
        return Err(decode_error(
            "page_operation_mismatch",
            "typed page plan does not belong to a PageBy operation",
        ));
    };
    Ok(window)
}

fn provider_solution_from_hydrated(
    validated: &ValidatedMatchRequest,
    bindings: BTreeMap<BindingId, HydratedThing>,
    mut satisfied_role_edges: Vec<RoleEdgeId>,
) -> Result<ProviderSolutionEvidence, OrmError> {
    let concept_bindings = bindings
        .iter()
        .map(|(binding, thing)| (*binding, thing.concept_id().as_str().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let hydrated = bindings
        .iter()
        .map(|(binding, thing)| {
            (
                (*binding, thing.concept_id().as_str().to_owned()),
                thing.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let role_edges = validated
        .request()
        .plan
        .predicate
        .as_ref()
        .map(|predicate| {
            let mut edges = Vec::new();
            collect_role_edges(predicate, &mut edges);
            edges
        })
        .unwrap_or_default();
    for edge in &role_edges {
        if !satisfied_role_edges.contains(&edge.id)
            && hydrated_role_edge(&hydrated, &concept_bindings, edge)
        {
            satisfied_role_edges.push(edge.id);
        }
    }
    Ok(ProviderSolutionEvidence::new(
        bindings
            .into_iter()
            .map(|(binding, thing)| BoundConceptEvidence::new(binding, thing))
            .collect(),
        satisfied_role_edges,
    ))
}

fn rows_evidence_from_hydration(
    validated: &ValidatedMatchRequest,
    solutions: Vec<UnhydratedSolution>,
    hydrated: BTreeMap<(BindingId, String), HydratedThing>,
) -> Result<ProviderResultEvidence, OrmError> {
    let role_edges = validated
        .request()
        .plan
        .predicate
        .as_ref()
        .map(|predicate| {
            let mut edges = Vec::new();
            collect_role_edges(predicate, &mut edges);
            edges
        })
        .unwrap_or_default();
    let solutions = solutions
        .into_iter()
        .map(|solution| {
            let mut satisfied_role_edges = solution.satisfied_role_edges;
            for edge in &role_edges {
                if !satisfied_role_edges.contains(&edge.id)
                    && hydrated_role_edge(&hydrated, &solution.bindings, edge)
                {
                    satisfied_role_edges.push(edge.id);
                }
            }
            let bindings = solution
                .bindings
                .into_iter()
                .map(|(binding, concept_id)| {
                    hydrated
                        .get(&(binding, concept_id))
                        .cloned()
                        .map(|thing| BoundConceptEvidence::new(binding, thing))
                        .ok_or_else(|| {
                            decode_error(
                                "missing_hydrated_concept",
                                "solution references a concept absent from batched hydration",
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ProviderSolutionEvidence::new(
                bindings,
                satisfied_role_edges,
            ))
        })
        .collect::<Result<Vec<_>, OrmError>>()?;
    Ok(rows_evidence(validated, solutions))
}

struct RoleEdgeContract<'a> {
    id: RoleEdgeId,
    relation: BindingId,
    role_name: &'a str,
    player: BindingId,
}

fn collect_role_edges<'a>(expression: &'a MatchExpr, edges: &mut Vec<RoleEdgeContract<'a>>) {
    match expression {
        MatchExpr::RoleEdge {
            id,
            relation,
            role,
            player,
        } => edges.push(RoleEdgeContract {
            id: *id,
            relation: *relation,
            role_name: &role.name,
            player: *player,
        }),
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for expression in expressions {
                collect_role_edges(expression, edges);
            }
        }
        MatchExpr::Not { expression } => collect_role_edges(expression, edges),
        MatchExpr::FieldValue { .. } | MatchExpr::FieldComparison { .. } => {}
    }
}

fn hydrated_role_edge(
    hydrated: &BTreeMap<(BindingId, String), HydratedThing>,
    bindings: &BTreeMap<BindingId, String>,
    edge: &RoleEdgeContract<'_>,
) -> bool {
    let Some(relation_id) = bindings.get(&edge.relation) else {
        return false;
    };
    let Some(player_id) = bindings.get(&edge.player) else {
        return false;
    };
    hydrated
        .get(&(edge.relation, relation_id.clone()))
        .is_some_and(|relation| {
            relation.roles().iter().any(|role| {
                role.role().name == edge.role_name
                    && role
                        .players()
                        .iter()
                        .any(|player| player.concept_id().as_str() == player_id)
            })
        })
}

fn hydration_batches(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    solutions: &[UnhydratedSolution],
) -> Result<Vec<TypedHydrateThings>, OrmError> {
    let snapshot = registry.snapshot();
    let mut entity_targets = Vec::new();
    let mut relation_targets = Vec::new();
    for binding in &validated.request().plan.bindings {
        let declared_type = descriptor_name(&binding.descriptor)?;
        let concept_ids = solutions
            .iter()
            .map(|solution| solution.bindings[&binding.id].clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let concrete_descriptors = snapshot
            .iter()
            .filter(|descriptor| {
                descriptor_kind(descriptor) == binding.thing_kind
                    && is_type_or_subtype(&snapshot, descriptor.type_name(), declared_type)
            })
            .map(typed_hydration_descriptor)
            .collect::<Vec<_>>();
        let target = TypedHydrationTarget {
            binding: binding.id.get(),
            declared_type: declared_type.to_owned(),
            kind: typed_kind(binding.thing_kind),
            concept_ids,
            concrete_descriptors,
        };
        match binding.thing_kind {
            ThingKind::Entity => entity_targets.push(target),
            ThingKind::Relation => relation_targets.push(target),
        }
    }
    Ok([entity_targets, relation_targets]
        .into_iter()
        .filter(|targets| !targets.is_empty())
        .map(|targets| TypedHydrateThings { targets })
        .collect())
}

fn typed_hydration_descriptor(descriptor: &TypeDescriptor) -> TypedHydrationDescriptor {
    let (kind, fields, roles) = match descriptor {
        TypeDescriptor::Entity(descriptor) => (
            TypedThingKind::Entity,
            &descriptor.owned_attributes,
            &[][..],
        ),
        TypeDescriptor::Relation(descriptor) => (
            TypedThingKind::Relation,
            &descriptor.owned_attributes,
            descriptor.roles.as_slice(),
        ),
    };
    TypedHydrationDescriptor {
        type_name: descriptor.type_name().to_owned(),
        kind,
        fields: fields
            .iter()
            .map(|field| TypedHydrationField {
                field_name: field.field_name.clone(),
                attribute_type: field.attr_name.clone(),
                value_type: field.value_type.as_str().to_owned(),
            })
            .collect(),
        roles: roles
            .iter()
            .map(|role| TypedHydrationRole {
                role_name: role.role_name.clone(),
                player_types: role.player_type_names.clone(),
            })
            .collect(),
    }
}

fn descriptor_kind(descriptor: &TypeDescriptor) -> ThingKind {
    match descriptor {
        TypeDescriptor::Entity(_) => ThingKind::Entity,
        TypeDescriptor::Relation(_) => ThingKind::Relation,
    }
}

fn is_type_or_subtype(snapshot: &[TypeDescriptor], actual: &str, expected: &str) -> bool {
    let mut current = Some(actual);
    let mut visited = BTreeSet::new();
    while let Some(name) = current {
        if name == expected {
            return true;
        }
        if !visited.insert(name) {
            return false;
        }
        current = snapshot
            .iter()
            .find(|descriptor| descriptor.type_name() == name)
            .and_then(|descriptor| match descriptor {
                TypeDescriptor::Entity(descriptor) => descriptor.parent_type.as_deref(),
                TypeDescriptor::Relation(descriptor) => descriptor.parent_type.as_deref(),
            });
    }
    false
}

fn descriptor_name(descriptor: &DescriptorId) -> Result<&str, OrmError> {
    descriptor
        .as_str()
        .split_once(':')
        .map(|(_, name)| name)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            decode_error(
                "malformed_descriptor_identity",
                "validated binding has a malformed descriptor identity",
            )
        })
}

fn typed_kind(kind: ThingKind) -> TypedThingKind {
    match kind {
        ThingKind::Entity => TypedThingKind::Entity,
        ThingKind::Relation => TypedThingKind::Relation,
    }
}

fn output_slots(output: &FetchShape) -> Box<dyn Iterator<Item = &FetchSlot> + '_> {
    match output {
        FetchShape::Positional { slots } => Box::new(slots.iter()),
        FetchShape::Named { slots } => Box::new(slots.iter().map(|slot| &slot.slot)),
    }
}

fn decode_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResultDecode, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

fn decode_error_owned(code: &'static str, message: String) -> OrmError {
    MatchError::new(MatchErrorCategory::ResultDecode, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

fn resource_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

fn provider_transaction_open_error(_error: OrmError) -> OrmError {
    canonical_provider_error(
        "provider_transaction_open_failed",
        "provider transaction could not be opened for typed match execution",
    )
}

fn provider_statement_error(error: OrmError) -> OrmError {
    if let OrmError::Match(error) = error
        && let Some(error) = sanitized_provider_statement_error(error)
    {
        return error;
    }
    canonical_provider_error(
        "provider_statement_failed",
        "provider statement failed before complete typed match evidence was produced",
    )
}

fn provider_transaction_close_error(_error: OrmError) -> OrmError {
    canonical_provider_error(
        "provider_transaction_close_failed",
        "provider transaction could not be closed after typed match execution",
    )
}

fn canonical_provider_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::Provider, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

fn sanitized_provider_statement_error(error: MatchError) -> Option<OrmError> {
    const RESOURCE_CODES: &[&str] = &[
        "answer_byte_counter_overflow",
        "collected_concept_limit",
        "hydrated_attribute_value_limit",
        "hydrated_thing_limit",
        "processed_item_counter_overflow",
        "processed_item_limit",
        "provider_cancelled",
        "response_byte_limit",
        "transaction_deadline_exceeded",
    ];
    const RESULT_DECODE_CODES: &[&str] = &[
        "duplicate_hydrated_concept",
        "duplicate_provider_binding",
        "duplicate_selected_root",
        "hydrated_attribute_value_type",
        "hydration_answer_kind",
        "malformed_descriptor_identity",
        "malformed_hydrated_attributes",
        "malformed_hydrated_role_player",
        "malformed_hydrated_roles",
        "malformed_hydration_document",
        "malformed_page_rematch_binding",
        "malformed_page_rematch_document",
        "malformed_provider_concept_id",
        "malformed_root_row",
        "malformed_solution_row",
        "missing_hydrated_concept",
        "missing_provider_binding",
        "missing_provider_concept_id",
        "page_operation_mismatch",
        "page_rematch_answer_kind",
        "provider_binding_mismatch",
        "root_answer_kind",
        "root_binding_mismatch",
        "selected_root_set_mismatch",
        "solution_answer_kind",
        "unexpected_hydrated_concept",
        "unexpected_hydrated_root",
        "unknown_hydrated_attribute",
        "unknown_hydrated_binding",
        "unknown_hydrated_descriptor",
        "unknown_provider_binding",
        "unknown_role_player_descriptor",
        "unknown_hydrated_role",
        "unsupported_collection_slot",
        "unsupported_selected_operation",
    ];

    let code = error.code().as_str();
    let (category, message) = match error.category() {
        MatchErrorCategory::ResourceLimit if RESOURCE_CODES.contains(&code) => (
            MatchErrorCategory::ResourceLimit,
            "provider resource limits prevented complete typed match evidence",
        ),
        MatchErrorCategory::ResultDecode if RESULT_DECODE_CODES.contains(&code) => (
            MatchErrorCategory::ResultDecode,
            "provider evidence failed canonical typed match decoding",
        ),
        _ => return None,
    };
    Some(
        MatchError::new(category, code.to_owned(), message)
            .at(MatchErrorPathSegment::ProviderEvidence)
            .into(),
    )
}

fn validate_provider_iid(value: &str) -> Result<(), OrmError> {
    let valid = value.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty()
            && digits.len() <= 256
            && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    });
    if valid {
        Ok(())
    } else {
        Err(decode_error(
            "malformed_provider_concept_id",
            "provider concept IID must be bounded canonical hexadecimal",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::MatchErrorPath;
    use crate::attribute::ValueType;
    use crate::descriptor::{
        EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
    };
    use crate::entity::Annotation;
    use crate::match_request::ids::BoundFieldId;
    use crate::match_request::model::{
        BindingPair, MatchBinding, MatchExpr, MatchMode, MatchOrder, MatchPlan, MatchRequest,
        MissingOrder, SortDirection, Window,
    };
    use crate::match_request::validation::validate_match_request;
    use crate::session::backend::{
        BoundedAnswerReader, BoundedAnswerStats, BoxFuture, DriverBackend, QueryResult,
        TransactionOps,
    };
    use tokio::sync::Notify;

    type RecordedAnswers = Arc<Mutex<VecDeque<Result<Vec<AnswerItem>, String>>>>;

    #[test]
    fn executor_and_lowerer_source_contain_no_raw_or_dynamic_escape_hatch() {
        let executor = include_str!("selected_result_executor.rs");
        let lowerer = include_str!("lowering.rs");
        for forbidden in [
            concat!("Pattern", "::Raw"),
            concat!("Statement", "::Raw"),
            concat!("Dynamic", "Expr"),
        ] {
            assert!(!executor.contains(forbidden));
            assert!(!lowerer.contains(forbidden));
        }
    }

    #[test]
    fn execution_limit_builders_only_tighten_every_semantic_and_statement_ceiling() {
        let limits = MatchExecutionLimits::tightened(
            7,
            4096,
            Duration::from_secs(1),
            AnswerCancellation::default(),
        );
        let validation = limits.validation_limits();
        assert_eq!(
            (
                validation.solutions,
                validation.result_identities,
                validation.hydrated_things,
                validation.attribute_values,
                validation.collected_concepts,
            ),
            (7, 7, 7, 7, 7)
        );

        let limits = limits
            .with_max_hydrated_things(3)
            .with_max_hydrated_things(6)
            .with_max_attribute_values(4)
            .with_max_attribute_values(7)
            .with_max_collected_concepts(2)
            .with_max_collected_concepts(5)
            .with_max_statements(1)
            .with_max_statements(MAX_STATEMENTS);
        assert_eq!(
            (
                limits.max_hydrated_things,
                limits.max_attribute_values,
                limits.max_collected_concepts,
                limits.max_statements,
            ),
            (3, 4, 2, 1)
        );
    }

    #[tokio::test]
    async fn execution_budget_rechecks_cancellation_after_a_ready_provider_error() {
        let cancellation = AnswerCancellation::default();
        let trigger = cancellation.clone();
        let limits =
            MatchExecutionLimits::tightened(10, 4096, Duration::from_secs(1), cancellation);
        let budget = ExecutionBudget::new(&limits, None);

        let error = budget
            .await_provider(async move {
                trigger.cancel();
                Err::<(), _>(OrmError::QueryExecution("decoder failed".into()))
            })
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "provider_cancelled");
    }

    #[derive(Default)]
    struct Events {
        opens: usize,
        closes: usize,
        commits: usize,
        rollbacks: usize,
        solution_answers: usize,
        root_answers: usize,
        hydration_answers: usize,
        rematch_answers: usize,
        solution_statements: Vec<TypedFetchRows>,
        root_statements: Vec<TypedRootScan>,
        rematch_statements: Vec<TypedPageRematch>,
        hydration_statements: Vec<TypedHydrateThings>,
    }

    struct RecordingBackend {
        events: Arc<Mutex<Events>>,
        capabilities: CapabilitySet,
        solutions: RecordedAnswers,
        hydrations: RecordedAnswers,
        close_failure: Option<String>,
    }

    impl DriverBackend for RecordingBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            self.capabilities.clone()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            let events = Arc::clone(&self.events);
            let solutions = Arc::clone(&self.solutions);
            let hydrations = Arc::clone(&self.hydrations);
            let close_failure = self.close_failure.clone();
            Box::pin(async move {
                events.lock().unwrap().opens += 1;
                Ok(Box::new(RecordingTransaction {
                    events,
                    solutions,
                    hydrations,
                    close_failure,
                }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct RecordingTransaction {
        events: Arc<Mutex<Events>>,
        solutions: RecordedAnswers,
        hydrations: RecordedAnswers,
        close_failure: Option<String>,
    }

    impl TransactionOps for RecordingTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("selected executor used legacy string query") })
        }

        fn query_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let events = Arc::clone(&self.events);
            let response = self
                .solutions
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                events.lock().unwrap().solution_statements.push(query);
                feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Solution,
                )
            })
        }

        fn hydrate_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedHydrateThings,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let events = Arc::clone(&self.events);
            let response = self
                .hydrations
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                events.lock().unwrap().hydration_statements.push(query);
                feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Hydration,
                )
            })
        }

        fn query_root_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedRootScan,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let events = Arc::clone(&self.events);
            let response = self
                .solutions
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                events.lock().unwrap().root_statements.push(query);
                feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Root,
                )
            })
        }

        fn rematch_page_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedPageRematch,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let events = Arc::clone(&self.events);
            let response = self
                .hydrations
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                events.lock().unwrap().rematch_statements.push(query);
                feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Rematch,
                )
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.events.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.events.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.events.lock().unwrap().closes += 1;
            let failure = self.close_failure.take();
            Box::pin(async move {
                match failure {
                    Some(message) => Err(OrmError::Transaction(message)),
                    None => Ok(()),
                }
            })
        }
    }

    fn feed(
        response: Result<Vec<AnswerItem>, String>,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
    ) -> Result<BoundedAnswerStats, OrmError> {
        let items = response.map_err(OrmError::QueryExecution)?;
        let mut reader = BoundedAnswerReader::new(limits);
        reader.check_before_read()?;
        for item in items {
            if reader.accept(item, consumer)? == AnswerControl::Stop {
                break;
            }
        }
        Ok(reader.stats())
    }

    #[derive(Clone, Copy)]
    enum RecordedAnswerKind {
        Solution,
        Root,
        Hydration,
        Rematch,
    }

    fn feed_recorded(
        response: Result<Vec<AnswerItem>, String>,
        limits: BoundedAnswerLimits,
        consumer: &mut dyn AnswerConsumer,
        events: &Arc<Mutex<Events>>,
        kind: RecordedAnswerKind,
    ) -> Result<BoundedAnswerStats, OrmError> {
        let items = response.map_err(OrmError::QueryExecution)?;
        let mut reader = BoundedAnswerReader::new(limits);
        reader.check_before_read()?;
        for item in items {
            {
                let mut events = events.lock().unwrap();
                let answers = match kind {
                    RecordedAnswerKind::Solution => &mut events.solution_answers,
                    RecordedAnswerKind::Root => &mut events.root_answers,
                    RecordedAnswerKind::Hydration => &mut events.hydration_answers,
                    RecordedAnswerKind::Rematch => &mut events.rematch_answers,
                };
                *answers += 1;
            }
            if reader.accept(item, consumer)? == AnswerControl::Stop {
                break;
            }
        }
        Ok(reader.stats())
    }

    #[derive(Default)]
    struct LegacyCounters {
        opens: AtomicUsize,
        statements: AtomicUsize,
        closes: AtomicUsize,
    }

    struct LegacyOnlyBackend {
        counters: Arc<LegacyCounters>,
    }

    impl DriverBackend for LegacyOnlyBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.counters.opens.fetch_add(1, AtomicOrdering::SeqCst);
            let counters = Arc::clone(&self.counters);
            Box::pin(async move {
                Ok(Box::new(LegacyOnlyTransaction { counters }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct LegacyOnlyTransaction {
        counters: Arc<LegacyCounters>,
    }

    impl TransactionOps for LegacyOnlyTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            self.counters
                .statements
                .fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async { Ok(QueryResult::Rows(Vec::new())) })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.counters.closes.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    struct OpenFailureBackend {
        opens: Arc<AtomicUsize>,
    }

    impl DriverBackend for OpenFailureBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.opens.fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async {
                Err(OrmError::Connection(
                    "credential=top-secret provider detail".into(),
                ))
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    #[derive(Default)]
    struct SnapshotEvents {
        opens: AtomicUsize,
        root_statements: AtomicUsize,
        rematch_statements: AtomicUsize,
        closes: AtomicUsize,
    }

    struct SnapshotBarrierState {
        live_name: Mutex<String>,
        selection_complete: Notify,
        resume_rematch: Notify,
        events: SnapshotEvents,
    }

    struct SnapshotBarrierBackend {
        state: Arc<SnapshotBarrierState>,
    }

    impl DriverBackend for SnapshotBarrierBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.state.events.opens.fetch_add(1, AtomicOrdering::SeqCst);
            let snapshot_name = self.state.live_name.lock().unwrap().clone();
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                Ok(Box::new(SnapshotBarrierTransaction {
                    state,
                    snapshot_name,
                }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct SnapshotBarrierTransaction {
        state: Arc<SnapshotBarrierState>,
        snapshot_name: String,
    }

    impl TransactionOps for SnapshotBarrierTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("snapshot-barrier test used a legacy string query") })
        }

        fn query_root_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedRootScan,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state
                    .events
                    .root_statements
                    .fetch_add(1, AtomicOrdering::SeqCst);
                let stats = feed(Ok(vec![solution(&[(0, "0x01")], &[])]), limits, consumer)?;
                state.selection_complete.notify_one();
                Ok(stats)
            })
        }

        fn rematch_page_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedPageRematch,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let state = Arc::clone(&self.state);
            let snapshot_name = self.snapshot_name.clone();
            Box::pin(async move {
                state
                    .events
                    .rematch_statements
                    .fetch_add(1, AtomicOrdering::SeqCst);
                assert_eq!(query.root_concept_ids, ["0x01"]);
                state.resume_rematch.notified().await;
                feed(
                    Ok(vec![person_rematch("0x01", &snapshot_name)]),
                    limits,
                    consumer,
                )
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state
                .events
                .closes
                .fetch_add(1, AtomicOrdering::SeqCst);
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum PendingProviderPhase {
        Open,
        Statement,
        Close,
    }

    struct PendingProviderState {
        phase: PendingProviderPhase,
        entered: Notify,
        opens: AtomicUsize,
        statements: AtomicUsize,
        closes: AtomicUsize,
        pending_statement: AtomicBool,
    }

    impl PendingProviderState {
        fn new(phase: PendingProviderPhase) -> Arc<Self> {
            Arc::new(Self {
                phase,
                entered: Notify::new(),
                opens: AtomicUsize::new(0),
                statements: AtomicUsize::new(0),
                closes: AtomicUsize::new(0),
                pending_statement: AtomicBool::new(phase == PendingProviderPhase::Statement),
            })
        }
    }

    struct PendingProviderBackend {
        state: Arc<PendingProviderState>,
    }

    impl DriverBackend for PendingProviderBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            CapabilitySet::all()
        }

        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state.opens.fetch_add(1, AtomicOrdering::SeqCst);
                if state.phase == PendingProviderPhase::Open {
                    state.entered.notify_one();
                    std::future::pending::<()>().await;
                }
                Ok(Box::new(PendingProviderTransaction { state }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }
    }

    struct PendingProviderTransaction {
        state: Arc<PendingProviderState>,
    }

    impl TransactionOps for PendingProviderTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("pending-provider test used a legacy string query") })
        }

        fn query_root_typed_bounded<'a>(
            &'a mut self,
            _query: &'a TypedRootScan,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                state.statements.fetch_add(1, AtomicOrdering::SeqCst);
                if state.pending_statement.swap(false, AtomicOrdering::SeqCst) {
                    state.entered.notify_one();
                    std::future::pending::<()>().await;
                }
                feed(Ok(Vec::new()), limits, consumer)
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.closes.fetch_add(1, AtomicOrdering::SeqCst);
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                if state.phase == PendingProviderPhase::Close {
                    state.entered.notify_one();
                    std::future::pending::<()>().await;
                }
                Ok(())
            })
        }
    }

    fn pending_provider_database(
        phase: PendingProviderPhase,
    ) -> (Database, Arc<PendingProviderState>) {
        let state = PendingProviderState::new(phase);
        let database = Database::with_backend(
            Box::new(PendingProviderBackend {
                state: Arc::clone(&state),
            }),
            "test",
        );
        (database, state)
    }

    fn key(field_name: &str) -> OwnedAttributeDescriptor {
        OwnedAttributeDescriptor {
            field_name: field_name.into(),
            attr_name: field_name.into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Key],
            is_optional: false,
            is_ordered: false,
            doc: None,
            meta: Default::default(),
        }
    }

    fn person_registry() -> DescriptorRegistry {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    fn person_request(
        registry: &DescriptorRegistry,
        cardinality: RowCardinality,
        limit: u64,
    ) -> MatchRequest {
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: BindingId::new(0),
                    descriptor: registry.descriptor_id("person").unwrap(),
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
                window: Window { offset: 0, limit },
                cardinality,
            },
        )
    }

    fn person_root_request(
        registry: &DescriptorRegistry,
        operation: impl FnOnce(BindingId) -> MatchOperation,
    ) -> MatchRequest {
        let root = BindingId::new(0);
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: root,
                    descriptor: registry.descriptor_id("person").unwrap(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            operation(root),
        )
    }

    fn person_page_request(registry: &DescriptorRegistry, include_total: bool) -> MatchRequest {
        let root = BindingId::new(0);
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: root,
                    descriptor: registry.descriptor_id("person").unwrap(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::PageBy {
                root,
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: root }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 1,
                    limit: 2,
                },
                include_total,
            },
        )
    }

    fn person_company_page_request(
        registry: &DescriptorRegistry,
        distinct: bool,
        limit: u64,
    ) -> MatchRequest {
        let root = BindingId::new(0);
        let company = BindingId::new(1);
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: root,
                        descriptor: registry.descriptor_id("person").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: company,
                        descriptor: registry.descriptor_id("company").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: None,
                allowed_cross_joins: BTreeSet::from([BindingPair::new(root, company)]),
            },
            MatchOperation::PageBy {
                root,
                output: FetchShape::Positional {
                    slots: vec![
                        FetchSlot::One { binding: root },
                        FetchSlot::Collect {
                            binding: company,
                            distinct,
                            order: Vec::new(),
                        },
                    ],
                },
                order: Vec::new(),
                window: Window { offset: 0, limit },
                include_total: false,
            },
        )
    }

    fn solution(bindings: &[(u16, &str)], edges: &[u16]) -> AnswerItem {
        AnswerItem::Row(serde_json::json!({
            "bindings": bindings.iter().map(|(binding, concept_id)| serde_json::json!({
                "binding": binding,
                "concept_id": concept_id,
            })).collect::<Vec<_>>(),
            "satisfied_role_edges": edges,
        }))
    }

    fn person_hydration(binding: u16, concept_id: &str, name: &str) -> AnswerItem {
        AnswerItem::Document(serde_json::json!({
            "binding": binding,
            "concept_id": concept_id,
            "concrete_type": "person",
            "kind": "entity",
            "attributes": [{"field": "name", "value_type": "string", "values": [name]}],
            "roles": [],
        }))
    }

    fn person_rematch(concept_id: &str, name: &str) -> AnswerItem {
        AnswerItem::Document(serde_json::json!({
            "bindings": [{
                "binding": 0,
                "concept_id": concept_id,
                "concrete_type": "person",
                "kind": "entity",
                "attributes": [{"field": "name", "value_type": "string", "values": [name]}],
                "roles": [],
            }],
            "satisfied_role_edges": [],
        }))
    }

    fn entity_wire(binding: u16, type_name: &str, concept_id: &str, name: &str) -> Value {
        serde_json::json!({
            "binding": binding,
            "concept_id": concept_id,
            "concrete_type": type_name,
            "kind": "entity",
            "attributes": [{"field": "name", "value_type": "string", "values": [name]}],
            "roles": [],
        })
    }

    fn rematch_entities(bindings: Vec<Value>) -> AnswerItem {
        AnswerItem::Document(serde_json::json!({
            "bindings": bindings,
            "satisfied_role_edges": [],
        }))
    }

    fn database(
        capabilities: CapabilitySet,
        solutions: Vec<Result<Vec<AnswerItem>, String>>,
        hydrations: Vec<Result<Vec<AnswerItem>, String>>,
    ) -> (Database, Arc<Mutex<Events>>) {
        database_with_close_failure(capabilities, solutions, hydrations, None)
    }

    fn database_with_close_failure(
        capabilities: CapabilitySet,
        solutions: Vec<Result<Vec<AnswerItem>, String>>,
        hydrations: Vec<Result<Vec<AnswerItem>, String>>,
        close_failure: Option<String>,
    ) -> (Database, Arc<Mutex<Events>>) {
        let events = Arc::new(Mutex::new(Events::default()));
        let backend = RecordingBackend {
            events: Arc::clone(&events),
            capabilities,
            solutions: Arc::new(Mutex::new(solutions.into())),
            hydrations: Arc::new(Mutex::new(hydrations.into())),
            close_failure,
        };
        (Database::with_backend(Box::new(backend), "test"), events)
    }

    fn match_code(error: &OrmError) -> &str {
        match_error(error).code().as_str()
    }

    fn match_error(error: &OrmError) -> &MatchError {
        let OrmError::Match(error) = error else {
            panic!("expected match error, got {error}")
        };
        error
    }

    #[test]
    fn provider_boundary_sanitizes_structured_backend_errors() {
        let forged_decode = MatchError::new(
            MatchErrorCategory::ResultDecode,
            "malformed_solution_row",
            "credential=top-secret",
        )
        .with_path(MatchErrorPath::from_segments([
            MatchErrorPathSegment::OutputName("credential=top-secret".into()),
        ]))
        .with_detail("credential", "top-secret");
        let sanitized = provider_statement_error(OrmError::Match(forged_decode.clone()));
        let sanitized = match_error(&sanitized);
        assert_eq!(sanitized.category(), MatchErrorCategory::ResultDecode);
        assert_eq!(sanitized.code().as_str(), "malformed_solution_row");
        assert_eq!(
            sanitized.path().segments(),
            &[MatchErrorPathSegment::ProviderEvidence]
        );
        assert!(sanitized.details().is_empty());
        assert!(!sanitized.to_string().contains("top-secret"));

        let forged_unknown = MatchError::new(
            MatchErrorCategory::ResultDecode,
            "credential_top_secret",
            "credential=top-secret",
        );
        let generic = provider_statement_error(OrmError::Match(forged_unknown));
        let generic = match_error(&generic);
        assert_eq!(generic.category(), MatchErrorCategory::Provider);
        assert_eq!(generic.code().as_str(), "provider_statement_failed");
        assert!(!generic.to_string().contains("top-secret"));

        for stage_error in [
            provider_transaction_open_error(OrmError::Match(forged_decode.clone())),
            provider_transaction_close_error(OrmError::Match(forged_decode)),
        ] {
            let stage_error = match_error(&stage_error);
            assert_eq!(stage_error.category(), MatchErrorCategory::Provider);
            assert!(stage_error.details().is_empty());
            assert!(!stage_error.to_string().contains("top-secret"));
        }
    }

    #[test]
    fn live_binding_ordinals_accept_only_integral_in_range_numbers() {
        assert_eq!(json_u16(&serde_json::json!(3.0)), Some(3));
        assert_eq!(json_u16(&serde_json::json!({"value": 4.0})), Some(4));
        assert_eq!(json_u16(&serde_json::json!(3.5)), None);
        assert_eq!(json_u16(&serde_json::json!(-1.0)), None);
        assert_eq!(json_u16(&serde_json::json!(65536.0)), None);
    }

    #[tokio::test]
    async fn legacy_only_backend_is_fail_closed_before_open_or_any_statement() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let counters = Arc::new(LegacyCounters::default());
        let database = Database::with_backend(
            Box::new(LegacyOnlyBackend {
                counters: Arc::clone(&counters),
            }),
            "test",
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();

        assert_eq!(match_code(&error), "missing_provider_capability");
        assert_eq!(counters.opens.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(counters.statements.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(counters.closes.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test]
    async fn provider_open_failure_is_canonical_and_redacts_backend_details() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let opens = Arc::new(AtomicUsize::new(0));
        let database = Database::with_backend(
            Box::new(OpenFailureBackend {
                opens: Arc::clone(&opens),
            }),
            "test",
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        let canonical = match_error(&error);

        assert_eq!(canonical.category(), MatchErrorCategory::Provider);
        assert_eq!(
            canonical.code().as_str(),
            "provider_transaction_open_failed"
        );
        assert_eq!(
            canonical.path().segments(),
            &[MatchErrorPathSegment::ProviderEvidence]
        );
        assert!(!canonical.message().contains("top-secret"));
        assert!(!error.to_string().contains("top-secret"));
        assert_eq!(opens.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn owned_close_failure_returns_no_result_and_is_canonical() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, events) = database_with_close_failure(
            CapabilitySet::all(),
            vec![Ok(vec![])],
            vec![],
            Some("close credential=top-secret".into()),
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        let canonical = match_error(&error);

        assert_eq!(canonical.category(), MatchErrorCategory::Provider);
        assert_eq!(
            canonical.code().as_str(),
            "provider_transaction_close_failed"
        );
        assert_eq!(
            canonical.path().segments(),
            &[MatchErrorPathSegment::ProviderEvidence]
        );
        assert!(!error.to_string().contains("top-secret"));
        let events = events.lock().unwrap();
        assert_eq!(events.opens, 1);
        assert_eq!(events.root_statements.len(), 1);
        assert_eq!(events.closes, 1);
    }

    #[tokio::test]
    async fn owned_validation_failure_wins_over_close_failure_after_exactly_one_close() {
        let registry = person_registry();
        let validated =
            validate_match_request(&registry, person_page_request(&registry, true)).unwrap();
        let (database, events) = database_with_close_failure(
            CapabilitySet::all(),
            vec![
                Ok(vec![solution(&[(0, "0x01")], &[])]),
                Ok(vec![solution(&[(0, "0x02")], &[])]),
            ],
            vec![Ok(vec![person_rematch("0x02", "Bob")])],
            Some("close credential=top-secret".into()),
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();

        assert_eq!(match_code(&error), "page_total_length_mismatch");
        assert!(!error.to_string().contains("top-secret"));
        let events = events.lock().unwrap();
        assert_eq!(events.opens, 1);
        assert_eq!(events.root_statements.len(), 2);
        assert_eq!(events.rematch_statements.len(), 1);
        assert_eq!(events.closes, 1);
    }

    #[tokio::test]
    async fn page_rematch_waits_at_a_real_barrier_and_uses_the_open_transaction_snapshot() {
        let registry = Arc::new(person_registry());
        let validated = Arc::new(
            validate_match_request(&registry, person_page_request(&registry, false)).unwrap(),
        );
        let state = Arc::new(SnapshotBarrierState {
            live_name: Mutex::new("Alice".into()),
            selection_complete: Notify::new(),
            resume_rematch: Notify::new(),
            events: SnapshotEvents::default(),
        });
        let database = Arc::new(Database::with_backend(
            Box::new(SnapshotBarrierBackend {
                state: Arc::clone(&state),
            }),
            "test",
        ));
        let task = {
            let database = Arc::clone(&database);
            let registry = Arc::clone(&registry);
            let validated = Arc::clone(&validated);
            tokio::spawn(async move { database.execute_match(&registry, &validated).await })
        };

        tokio::time::timeout(Duration::from_secs(5), state.selection_complete.notified())
            .await
            .expect("root selection did not reach the inter-stage barrier");
        *state.live_name.lock().unwrap() = "Bob".into();
        state.resume_rematch.notify_one();

        let result = task.await.unwrap().unwrap();
        let super::super::result::MatchResult::Page { entries, .. } = result.result() else {
            panic!("expected page result")
        };
        let super::super::result::SlotValue::One(thing) = &entries[0].slots()[0] else {
            panic!("expected singular root slot")
        };
        assert_eq!(
            thing.attributes()[0].values(),
            &[AttributeValue::String("Alice".into())]
        );
        assert_eq!(*state.live_name.lock().unwrap(), "Bob");
        assert_eq!(state.events.opens.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.events.root_statements.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(
            state.events.rematch_statements.load(AtomicOrdering::SeqCst),
            1
        );
        assert_eq!(state.events.closes.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn count_and_exists_use_distinct_root_streams_and_exists_stops_early() {
        let registry = person_registry();
        let count = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let exists = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::ExistsBy { root }),
        )
        .unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![
                Ok(vec![
                    solution(&[(0, "0x01")], &[]),
                    solution(&[(0, "0x01")], &[]),
                    solution(&[(0, "0x02")], &[]),
                ]),
                Ok(vec![
                    solution(&[(0, "0x03")], &[]),
                    AnswerItem::Row(serde_json::json!({"malformed": true})),
                ]),
            ],
            vec![],
        );

        let count = database.execute_match(&registry, &count).await.unwrap();
        assert!(matches!(
            count.result(),
            super::super::result::MatchResult::Count { value: 2, .. }
        ));
        let exists = database.execute_match(&registry, &exists).await.unwrap();
        assert!(matches!(
            exists.result(),
            super::super::result::MatchResult::Exists { value: true, .. }
        ));
        let events = events.lock().unwrap();
        assert_eq!(events.root_statements.len(), 2);
        assert!(events.root_statements[0].order.is_empty());
        assert_eq!(events.root_statements[0].offset, None);
        assert_eq!(events.root_statements[0].limit, Some(MAX_PROCESSED_ITEMS));
        assert!(events.root_statements[1].order.is_empty());
        assert_eq!(events.root_statements[1].offset, None);
        assert_eq!(events.root_statements[1].limit, Some(1));
        assert!(events.solution_statements.is_empty());
        assert!(events.hydration_statements.is_empty());
        assert_eq!((events.opens, events.closes), (2, 2));
    }

    #[tokio::test]
    async fn page_selects_distinct_roots_then_one_exact_rematch_and_omits_optional_total() {
        let registry = person_registry();
        for include_total in [false, true] {
            let validated =
                validate_match_request(&registry, person_page_request(&registry, include_total))
                    .unwrap();
            let mut roots = Vec::new();
            if include_total {
                roots.push(Ok(vec![
                    solution(&[(0, "0x01")], &[]),
                    solution(&[(0, "0x02")], &[]),
                    solution(&[(0, "0x03")], &[]),
                ]));
            }
            roots.push(Ok(vec![
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
            ]));
            let (database, events) = database(
                CapabilitySet::all(),
                roots,
                vec![Ok(vec![
                    person_rematch("0x03", "Carol"),
                    person_rematch("0x02", "Bob"),
                ])],
            );

            let result = database.execute_match(&registry, &validated).await.unwrap();
            let super::super::result::MatchResult::Page { entries, total, .. } = result.result()
            else {
                panic!("expected page")
            };
            assert_eq!(*total, include_total.then_some(3));
            let ids = entries
                .iter()
                .map(|row| match &row.slots()[0] {
                    super::super::result::SlotValue::One(thing) => thing.concept_id().as_str(),
                    super::super::result::SlotValue::Many(_) => panic!("expected root"),
                })
                .collect::<Vec<_>>();
            assert_eq!(ids, vec!["0x02", "0x03"]);
            let events = events.lock().unwrap();
            assert_eq!(
                events.root_statements.len(),
                if include_total { 2 } else { 1 }
            );
            assert_eq!(events.rematch_statements.len(), 1);
            assert!(events.hydration_statements.is_empty());
            assert_eq!(
                events.root_statements.len() + events.rematch_statements.len(),
                if include_total { 3 } else { 2 }
            );
            let selection = events.root_statements.last().unwrap();
            assert_eq!(selection.offset, Some(1));
            assert_eq!(selection.limit, Some(2));
            assert!(!selection.order.is_empty());
            if include_total {
                let total = &events.root_statements[0];
                assert!(total.order.is_empty());
                assert_eq!(total.offset, None);
                assert_eq!(total.limit, Some(MAX_PROCESSED_ITEMS));
            }
            assert_eq!(
                events.rematch_statements[0].root_concept_ids,
                vec!["0x02", "0x03"]
            );
            assert_eq!((events.opens, events.closes), (1, 1));
        }
    }

    #[tokio::test]
    async fn shuffled_two_collection_cross_product_is_grouped_and_sorted_per_binding() {
        let registry = person_registry();
        for type_name in ["company", "project"] {
            registry
                .register_entity(EntityDescriptor {
                    type_name: type_name.into(),
                    is_abstract: false,
                    parent_type: None,
                    owned_attributes: vec![key("name")],
                    doc: None,
                    meta: Default::default(),
                })
                .unwrap();
        }
        let root = BindingId::new(0);
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: root,
                        descriptor: registry.descriptor_id("person").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: BindingId::new(1),
                        descriptor: registry.descriptor_id("company").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: BindingId::new(2),
                        descriptor: registry.descriptor_id("project").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: None,
                allowed_cross_joins: BTreeSet::from([
                    BindingPair::new(root, BindingId::new(1)),
                    BindingPair::new(root, BindingId::new(2)),
                ]),
            },
            MatchOperation::PageBy {
                root,
                output: FetchShape::Positional {
                    slots: vec![
                        FetchSlot::One { binding: root },
                        FetchSlot::Collect {
                            binding: BindingId::new(1),
                            distinct: false,
                            order: Vec::new(),
                        },
                        FetchSlot::Collect {
                            binding: BindingId::new(2),
                            distinct: true,
                            order: Vec::new(),
                        },
                    ],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                include_total: false,
            },
        );
        let validated = validate_match_request(&registry, request).unwrap();
        let make = |company: (&str, &str), project: (&str, &str)| {
            rematch_entities(vec![
                entity_wire(0, "person", "0x01", "Alice"),
                entity_wire(1, "company", company.0, company.1),
                entity_wire(2, "project", project.0, project.1),
            ])
        };
        let shuffled = vec![
            make(("0x12", "Zulu"), ("0x22", "Two")),
            make(("0x11", "Acme"), ("0x21", "One")),
            make(("0x12", "Zulu"), ("0x21", "One")),
            make(("0x11", "Acme"), ("0x22", "Two")),
        ];
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(shuffled)],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Page { entries, .. } = result.result() else {
            panic!("expected page")
        };
        let super::super::result::SlotValue::Many(companies) = &entries[0].slots()[1] else {
            panic!("expected company collection")
        };
        let super::super::result::SlotValue::Many(projects) = &entries[0].slots()[2] else {
            panic!("expected project collection")
        };
        assert_eq!(
            companies
                .iter()
                .map(|thing| thing.concept_id().as_str())
                .collect::<Vec<_>>(),
            vec!["0x11", "0x11", "0x12", "0x12"]
        );
        assert_eq!(
            projects
                .iter()
                .map(|thing| thing.concept_id().as_str())
                .collect::<Vec<_>>(),
            vec!["0x21", "0x22"]
        );
        let events = events.lock().unwrap();
        assert_eq!(events.rematch_statements.len(), 1);
        assert_eq!(
            events.rematch_statements[0]
                .collection_orders
                .iter()
                .map(|order| order.binding)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn root_scan_caps_never_become_false_count_page_or_exists_results() {
        let registry = person_registry();
        let count = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (count_db, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x01")], &[]),
            ])],
            vec![],
        );
        let error = count_db
            .execute_match_with_limits(
                &registry,
                &count,
                MatchExecutionLimits::tightened(
                    3,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "solution_scan_limit");
        assert_eq!(events.lock().unwrap().closes, 1);

        let exists = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::ExistsBy { root }),
        )
        .unwrap();
        let (empty, _) = database(CapabilitySet::all(), vec![Ok(vec![])], vec![]);
        let result = empty.execute_match(&registry, &exists).await.unwrap();
        assert!(matches!(
            result.result(),
            super::super::result::MatchResult::Exists { value: false, .. }
        ));

        let page =
            validate_match_request(&registry, person_page_request(&registry, false)).unwrap();
        let (short, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x01")], &[]),
            ])],
            vec![],
        );
        let error = short
            .execute_match_with_limits(
                &registry,
                &page,
                MatchExecutionLimits::tightened(
                    2,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "solution_scan_limit");
        assert!(events.lock().unwrap().rematch_statements.is_empty());
        assert_eq!(events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn page_rejects_missing_extra_bad_total_and_cross_stage_budget_exhaustion() {
        let registry = person_registry();
        let page =
            validate_match_request(&registry, person_page_request(&registry, false)).unwrap();
        let cases = [
            (
                vec![solution(&[(0, "0x01")], &[]), solution(&[(0, "0x02")], &[])],
                vec![person_rematch("0x01", "Alice")],
                "selected_root_set_mismatch",
            ),
            (
                vec![solution(&[(0, "0x01")], &[])],
                vec![person_rematch("0x02", "Bob")],
                "unexpected_hydrated_root",
            ),
        ];
        for (roots, rematch, expected) in cases {
            let (database, events) =
                database(CapabilitySet::all(), vec![Ok(roots)], vec![Ok(rematch)]);
            let error = database.execute_match(&registry, &page).await.unwrap_err();
            assert_eq!(match_code(&error), expected);
            assert_eq!(events.lock().unwrap().closes, 1);
        }

        let with_total =
            validate_match_request(&registry, person_page_request(&registry, true)).unwrap();
        let (total_db, events) = database(
            CapabilitySet::all(),
            vec![
                Ok(vec![solution(&[(0, "0x01")], &[])]),
                Ok(vec![solution(&[(0, "0x02")], &[])]),
            ],
            vec![Ok(vec![person_rematch("0x02", "Bob")])],
        );
        let error = total_db
            .execute_match(&registry, &with_total)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "page_total_length_mismatch");
        assert_eq!(events.lock().unwrap().closes, 1);

        let (budgeted, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_rematch("0x01", "Alice")])],
        );
        let error = budgeted
            .execute_match_with_limits(
                &registry,
                &page,
                MatchExecutionLimits::tightened(
                    2,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "solution_scan_limit");
        assert_eq!(events.lock().unwrap().rematch_statements.len(), 1);
        assert_eq!(events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn owned_page_closes_atomically_when_any_provider_stage_fails() {
        let registry = person_registry();
        let page = validate_match_request(&registry, person_page_request(&registry, true)).unwrap();
        let cases = [
            (vec![Err("total failed: top-secret".into())], vec![], 1, 0),
            (
                vec![
                    Ok(vec![solution(&[(0, "0x01")], &[])]),
                    Err("selection failed: top-secret".into()),
                ],
                vec![],
                2,
                0,
            ),
            (
                vec![
                    Ok(vec![solution(&[(0, "0x01")], &[])]),
                    Ok(vec![solution(&[(0, "0x01")], &[])]),
                ],
                vec![Err("rematch failed: top-secret".into())],
                2,
                1,
            ),
        ];

        for (roots, rematch, root_statements, rematch_statements) in cases {
            let (database, events) = database(CapabilitySet::all(), roots, rematch);
            let error = database.execute_match(&registry, &page).await.unwrap_err();
            let canonical = match_error(&error);
            assert_eq!(canonical.category(), MatchErrorCategory::Provider);
            assert_eq!(canonical.code().as_str(), "provider_statement_failed");
            assert_eq!(
                canonical.path().segments(),
                &[MatchErrorPathSegment::ProviderEvidence]
            );
            assert!(!error.to_string().contains("top-secret"));
            let events = events.lock().unwrap();
            assert_eq!(events.root_statements.len(), root_statements);
            assert_eq!(events.rematch_statements.len(), rematch_statements);
            assert_eq!(events.closes, 1);
        }
    }

    #[tokio::test]
    async fn owned_entry_streams_solutions_batches_hydration_and_returns_only_validated_result() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
        let events = events.lock().unwrap();
        assert_eq!((events.opens, events.closes), (1, 1));
        assert_eq!(events.solution_statements.len(), 1);
        assert_eq!(events.hydration_statements.len(), 1);
        let batch = &events.hydration_statements[0];
        assert_eq!(batch.targets.len(), 1);
        assert_eq!(batch.targets[0].concept_ids, vec!["0x01"]);
        assert_eq!(
            batch.targets[0].concrete_descriptors[0].fields[0].field_name,
            "name"
        );
    }

    #[tokio::test]
    async fn exact_one_uses_canonical_no_result_and_not_unique_errors() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();

        let (empty, empty_events) = database(CapabilitySet::all(), vec![Ok(vec![])], vec![]);
        let error = empty
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "no_result");
        assert!(empty_events.lock().unwrap().hydration_statements.is_empty());
        assert_eq!(empty_events.lock().unwrap().closes, 1);

        let (multiple, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
            ])],
            vec![Ok(vec![
                person_hydration(0, "0x01", "Alice"),
                person_hydration(0, "0x02", "Bob"),
            ])],
        );
        let error = multiple
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "not_unique");
        assert_eq!(events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn hidden_witness_duplicates_collapse_before_one_batched_hydration() {
        let registry = person_registry();
        registry
            .register_entity(EntityDescriptor {
                type_name: "company".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: BindingId::new(0),
                        descriptor: registry.descriptor_id("person").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: BindingId::new(1),
                        descriptor: registry.descriptor_id("company").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: None,
                allowed_cross_joins: BTreeSet::from([BindingPair::new(
                    BindingId::new(0),
                    BindingId::new(1),
                )]),
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
        let validated = validate_match_request(&registry, request).unwrap();
        let company = |concept_id: &str, name: &str| {
            AnswerItem::Document(serde_json::json!({
                "binding": 1,
                "concept_id": concept_id,
                "concrete_type": "company",
                "kind": "entity",
                "attributes": [{"field": "name", "value_type": "string", "values": [name]}],
                "roles": [],
            }))
        };
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01"), (1, "0x10")], &[]),
                solution(&[(0, "0x01"), (1, "0x11")], &[]),
            ])],
            vec![Ok(vec![
                person_hydration(0, "0x01", "Alice"),
                company("0x10", "Acme"),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
        let events = events.lock().unwrap();
        assert_eq!(events.hydration_statements.len(), 1);
        assert_eq!(
            events.hydration_statements[0].targets[1].concept_ids,
            vec!["0x10"]
        );
    }

    #[tokio::test]
    async fn unwindowed_prefix_is_order_checked_then_offset_and_limit_are_applied_atomically() {
        let registry = person_registry();
        let mut request = person_request(&registry, RowCardinality::BoundedMany, 1);
        let MatchOperation::FetchRows { window, .. } = &mut request.operation else {
            unreachable!()
        };
        window.offset = 1;
        let validated = validate_match_request(&registry, request).unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
            ])],
            vec![Ok(vec![
                person_hydration(0, "0x01", "Alice"),
                person_hydration(0, "0x02", "Bob"),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        let super::super::result::SlotValue::One(person) = &rows[0].slots()[0] else {
            panic!("expected singular slot")
        };
        assert_eq!(person.concept_id().as_str(), "0x02");
        let events = events.lock().unwrap();
        assert_eq!(
            events.hydration_statements[0].targets[0].concept_ids.len(),
            2
        );
    }

    #[tokio::test]
    async fn attribute_budget_rejects_the_first_over_limit_document_before_another_read() {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    key("name"),
                    OwnedAttributeDescriptor {
                        field_name: "tags".into(),
                        attr_name: "tags".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Card(0, None)],
                        is_optional: true,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                ],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let over_limit = AnswerItem::Document(serde_json::json!({
            "binding": 0,
            "concept_id": "0x01",
            "concrete_type": "person",
            "kind": "entity",
            "attributes": [
                {"field": "name", "value_type": "string", "values": ["Alice"]},
                {"field": "tags", "value_type": "string", "values": ["one", "two"]}
            ],
            "roles": [],
        }));
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![
                over_limit,
                person_hydration(0, "0x01", "must-not-be-read"),
            ])],
        );
        let error = database
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_attribute_values(2),
            )
            .await
            .unwrap_err();

        assert_eq!(match_code(&error), "hydrated_attribute_value_limit");
        assert_eq!(
            match_error(&error).category(),
            MatchErrorCategory::ResourceLimit
        );
        let events = events.lock().unwrap();
        assert_eq!((events.solution_answers, events.hydration_answers), (1, 1));
        assert_eq!(events.closes, 1);
    }

    #[tokio::test]
    async fn row_hydration_budget_weights_shared_documents_by_solution_multiplicity() {
        let registry = person_registry();
        registry
            .register_entity(EntityDescriptor {
                type_name: "company".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
            .register_relation(RelationDescriptor {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("code")],
                roles: vec![RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    cardinality: Some((1, Some(1))),
                    ..RoleDescriptor::default()
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let relation = BindingId::new(0);
        let company = BindingId::new(1);
        let validated = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![
                        MatchBinding {
                            id: relation,
                            descriptor: registry.descriptor_id("employment").unwrap(),
                            thing_kind: ThingKind::Relation,
                            match_mode: MatchMode::Exact,
                        },
                        MatchBinding {
                            id: company,
                            descriptor: registry.descriptor_id("company").unwrap(),
                            thing_kind: ThingKind::Entity,
                            match_mode: MatchMode::Exact,
                        },
                    ],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::from([BindingPair::new(relation, company)]),
                },
                MatchOperation::FetchRows {
                    output: FetchShape::Positional {
                        slots: vec![FetchSlot::One { binding: company }],
                    },
                    order: Vec::new(),
                    window: Window {
                        offset: 0,
                        limit: 2,
                    },
                    cardinality: RowCardinality::BoundedMany,
                },
            ),
        )
        .unwrap();
        let employment = AnswerItem::Document(serde_json::json!({
            "binding": 0,
            "concept_id": "0x10",
            "concrete_type": "employment",
            "kind": "relation",
            "attributes": [{"field": "code", "value_type": "string", "values": ["E-1"]}],
            "roles": [{
                "role": "employee",
                "players": [{
                    "concept_id": "0x01",
                    "declared_type": "person",
                    "concrete_type": "person",
                    "kind": "entity",
                    "attributes": [{
                        "field": "name",
                        "value_type": "string",
                        "values": ["Alice"]
                    }]
                }]
            }]
        }));
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x10"), (1, "0x20")], &[]),
                solution(&[(0, "0x10"), (1, "0x21")], &[]),
            ])],
            vec![
                Ok(vec![
                    AnswerItem::Document(entity_wire(1, "company", "0x20", "Acme")),
                    AnswerItem::Document(entity_wire(1, "company", "0x21", "Beta")),
                ]),
                Ok(vec![
                    employment,
                    AnswerItem::Document(serde_json::json!({"must": "not be read"})),
                ]),
            ],
        );
        let error = database
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    16 * 1024,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_hydrated_things(5),
            )
            .await
            .unwrap_err();

        assert_eq!(match_code(&error), "hydrated_thing_limit");
        assert_eq!(
            match_error(&error).category(),
            MatchErrorCategory::ResourceLimit
        );
        let events = events.lock().unwrap();
        assert_eq!((events.solution_answers, events.hydration_answers), (2, 3));
        assert_eq!(events.hydration_statements.len(), 2);
        assert_eq!(events.closes, 1);
    }

    #[tokio::test]
    async fn page_collection_budget_counts_multiplicity_distinctness_and_root_scope_streaming() {
        let registry = person_registry();
        registry
            .register_entity(EntityDescriptor {
                type_name: "company".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let rematch = |person: (&str, &str), company: (&str, &str)| {
            rematch_entities(vec![
                entity_wire(0, "person", person.0, person.1),
                entity_wire(1, "company", company.0, company.1),
            ])
        };
        let cases = vec![
            (
                false,
                vec![solution(&[(0, "0x01")], &[])],
                vec![
                    rematch(("0x01", "Alice"), ("0x10", "Acme")),
                    rematch(("0x01", "Alice"), ("0x10", "Acme")),
                    rematch(("0x01", "Alice"), ("0x11", "must-not-be-read")),
                ],
                1,
                2,
            ),
            (
                true,
                vec![solution(&[(0, "0x01")], &[])],
                vec![
                    rematch(("0x01", "Alice"), ("0x10", "Acme")),
                    rematch(("0x01", "Alice"), ("0x10", "Acme")),
                    rematch(("0x01", "Alice"), ("0x11", "Beta")),
                    rematch(("0x01", "Alice"), ("0x12", "must-not-be-read")),
                ],
                1,
                3,
            ),
            (
                true,
                vec![solution(&[(0, "0x01")], &[]), solution(&[(0, "0x02")], &[])],
                vec![
                    rematch(("0x01", "Alice"), ("0x10", "Acme")),
                    rematch(("0x02", "Bob"), ("0x10", "Acme")),
                    rematch(("0x02", "Bob"), ("0x11", "must-not-be-read")),
                ],
                2,
                2,
            ),
        ];

        for (distinct, roots, documents, page_limit, expected_reads) in cases {
            let validated = validate_match_request(
                &registry,
                person_company_page_request(&registry, distinct, page_limit),
            )
            .unwrap();
            let (database, events) =
                database(CapabilitySet::all(), vec![Ok(roots)], vec![Ok(documents)]);
            let error = database
                .execute_match_with_limits(
                    &registry,
                    &validated,
                    MatchExecutionLimits::tightened(
                        10,
                        32 * 1024,
                        Duration::from_secs(1),
                        AnswerCancellation::default(),
                    )
                    .with_max_collected_concepts(1),
                )
                .await
                .unwrap_err();

            assert_eq!(match_code(&error), "collected_concept_limit");
            assert_eq!(
                match_error(&error).category(),
                MatchErrorCategory::ResourceLimit
            );
            let events = events.lock().unwrap();
            assert_eq!(events.rematch_answers, expected_reads);
            assert_eq!(events.closes, 1);
        }
    }

    #[tokio::test]
    async fn statement_budget_closes_owned_once_and_leaves_borrowed_context_reusable() {
        let registry = person_registry();
        let validated =
            validate_match_request(&registry, person_page_request(&registry, false)).unwrap();
        let root_answer = || Ok(vec![solution(&[(0, "0x01")], &[])]);
        let page_answer = || Ok(vec![person_rematch("0x01", "Alice")]);
        let limits = MatchExecutionLimits::default().with_max_statements(1);

        let (owned, owned_events) = database(
            CapabilitySet::all(),
            vec![root_answer()],
            vec![page_answer()],
        );
        let error = owned
            .execute_match_with_limits(&registry, &validated, limits.clone())
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "statement_count_limit");
        {
            let events = owned_events.lock().unwrap();
            assert_eq!(
                (
                    events.root_statements.len(),
                    events.rematch_statements.len()
                ),
                (1, 0)
            );
            assert_eq!(events.closes, 1);
        }

        let (borrowed, borrowed_events) = database(
            CapabilitySet::all(),
            vec![root_answer(), root_answer()],
            vec![page_answer()],
        );
        let context = borrowed.transaction_context(TxType::Read).await.unwrap();
        let error = context
            .execute_match_with_limits(&registry, &validated, limits)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "statement_count_limit");
        context.execute_match(&registry, &validated).await.unwrap();
        {
            let events = borrowed_events.lock().unwrap();
            assert_eq!((events.opens, events.closes), (1, 0));
            assert_eq!(
                (
                    events.root_statements.len(),
                    events.rematch_statements.len()
                ),
                (2, 1)
            );
        }
        context.close().await.unwrap();
        assert_eq!(borrowed_events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn malformed_extra_missing_and_resource_failures_return_no_partial_result_and_close() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let cases = vec![
            (
                vec![Ok(vec![solution(&[(0, "not-an-iid")], &[])])],
                vec![],
                MatchExecutionLimits::default(),
                "malformed_provider_concept_id",
            ),
            (
                vec![Ok(vec![AnswerItem::Row(
                    serde_json::json!({"bindings": []}),
                )])],
                vec![],
                MatchExecutionLimits::default(),
                "missing_provider_binding",
            ),
            (
                vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
                vec![Ok(vec![])],
                MatchExecutionLimits::default(),
                "missing_hydrated_concept",
            ),
            (
                vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
                vec![Ok(vec![person_hydration(0, "0x02", "Mallory")])],
                MatchExecutionLimits::default(),
                "unexpected_hydrated_concept",
            ),
            (
                vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
                vec![],
                MatchExecutionLimits::tightened(
                    1,
                    1,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
                "response_byte_limit",
            ),
        ];
        for (solutions, hydrations, limits, expected) in cases {
            let (database, events) = database(CapabilitySet::all(), solutions, hydrations);
            let error = database
                .execute_match_with_limits(&registry, &validated, limits)
                .await
                .unwrap_err();
            assert_eq!(match_code(&error), expected);
            assert_eq!(events.lock().unwrap().closes, 1);
        }

        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        for (limits, expected) in [
            (
                MatchExecutionLimits::tightened(10, 4096, Duration::from_secs(1), cancellation),
                "provider_cancelled",
            ),
            (
                MatchExecutionLimits::tightened(
                    10,
                    4096,
                    Duration::ZERO,
                    AnswerCancellation::default(),
                ),
                "transaction_deadline_exceeded",
            ),
        ] {
            let (database, events) = database(
                CapabilitySet::all(),
                vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
                vec![],
            );
            let error = database
                .execute_match_with_limits(&registry, &validated, limits)
                .await
                .unwrap_err();
            assert_eq!(match_code(&error), expected);
            let events = events.lock().unwrap();
            assert_eq!((events.opens, events.closes), (0, 0));
            assert!(events.hydration_statements.is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn owned_deadline_interrupts_pending_transaction_open() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, state) = pending_provider_database(PendingProviderPhase::Open);
        let execution = tokio::spawn(async move {
            database
                .execute_match_with_limits(
                    &registry,
                    &validated,
                    MatchExecutionLimits::tightened(
                        10,
                        4096,
                        Duration::from_secs(5),
                        AnswerCancellation::default(),
                    ),
                )
                .await
        });

        state.entered.notified().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "transaction_deadline_exceeded");
        assert_eq!(state.opens.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn owned_deadline_interrupts_pending_statement_and_still_closes_once() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, state) = pending_provider_database(PendingProviderPhase::Statement);
        let execution = tokio::spawn(async move {
            database
                .execute_match_with_limits(
                    &registry,
                    &validated,
                    MatchExecutionLimits::tightened(
                        10,
                        4096,
                        Duration::from_secs(5),
                        AnswerCancellation::default(),
                    ),
                )
                .await
        });

        state.entered.notified().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "transaction_deadline_exceeded");
        assert_eq!(state.statements.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn owned_deadline_polls_close_once_but_bounds_a_pending_close() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, state) = pending_provider_database(PendingProviderPhase::Close);
        let execution = tokio::spawn(async move {
            database
                .execute_match_with_limits(
                    &registry,
                    &validated,
                    MatchExecutionLimits::tightened(
                        10,
                        4096,
                        Duration::from_secs(5),
                        AnswerCancellation::default(),
                    ),
                )
                .await
        });

        state.entered.notified().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "transaction_deadline_exceeded");
        assert_eq!(state.statements.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancellation_after_statement_start_interrupts_and_preserves_borrowed_context() {
        let registry = Arc::new(person_registry());
        let validated = Arc::new(
            validate_match_request(
                &registry,
                person_root_request(&registry, |root| MatchOperation::CountBy { root }),
            )
            .unwrap(),
        );
        let (database, state) = pending_provider_database(PendingProviderPhase::Statement);
        let context = database.transaction_context(TxType::Read).await.unwrap();
        let cancellation = AnswerCancellation::default();
        let execution_context = context.clone();
        let execution_registry = Arc::clone(&registry);
        let execution_validated = Arc::clone(&validated);
        let execution_cancellation = cancellation.clone();
        let execution = tokio::spawn(async move {
            execution_context
                .execute_match_with_limits(
                    &execution_registry,
                    &execution_validated,
                    MatchExecutionLimits::tightened(
                        10,
                        4096,
                        Duration::from_secs(5),
                        execution_cancellation,
                    ),
                )
                .await
        });

        state.entered.notified().await;
        cancellation.cancel();
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "provider_cancelled");

        context.execute_match(&registry, &validated).await.unwrap();
        assert_eq!(state.statements.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 0);
        context.close().await.unwrap();
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test]
    async fn duplicate_heavy_scan_ceiling_never_becomes_short_success_or_false_exact_one() {
        let registry = person_registry();
        for (cardinality, limit) in [
            (RowCardinality::ExactlyOne, 1),
            (RowCardinality::BoundedMany, 2),
        ] {
            let validated =
                validate_match_request(&registry, person_request(&registry, cardinality, limit))
                    .unwrap();
            let (database, events) = database(
                CapabilitySet::all(),
                vec![Ok(vec![
                    solution(&[(0, "0x01")], &[]),
                    solution(&[(0, "0x01")], &[]),
                    solution(&[(0, "0x01")], &[]),
                ])],
                vec![],
            );
            let error = database
                .execute_match_with_limits(
                    &registry,
                    &validated,
                    MatchExecutionLimits::tightened(
                        3,
                        4096,
                        Duration::from_secs(1),
                        AnswerCancellation::default(),
                    ),
                )
                .await
                .unwrap_err();
            assert_eq!(match_code(&error), "solution_scan_limit");
            let events = events.lock().unwrap();
            assert!(events.hydration_statements.is_empty());
            assert_eq!(events.closes, 1);
        }
    }

    #[tokio::test]
    async fn capability_and_stale_schema_fail_before_opening_or_statement_execution() {
        let registry = person_registry();
        let request = person_request(&registry, RowCardinality::ExactlyOne, 1);
        let validated = validate_match_request(&registry, request.clone()).unwrap();
        let (missing, missing_events) = database(CapabilitySet::new(), vec![], vec![]);
        let error = missing
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "missing_provider_capability");
        assert_eq!(missing_events.lock().unwrap().opens, 0);

        let mut subtype_request = request;
        subtype_request.plan.bindings[0].match_mode = MatchMode::Subtypes;
        let subtype = validate_match_request(&registry, subtype_request).unwrap();
        registry
            .register_entity(EntityDescriptor {
                type_name: "employee".into(),
                is_abstract: false,
                parent_type: Some("person".into()),
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let (stale, stale_events) = database(CapabilitySet::all(), vec![], vec![]);
        let error = stale.execute_match(&registry, &subtype).await.unwrap_err();
        assert_eq!(match_code(&error), "stale_schema");
        assert_eq!(stale_events.lock().unwrap().opens, 0);
    }

    #[tokio::test]
    async fn borrowed_context_is_reusable_and_never_consumed_or_closed_by_selected_execution() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![
                Ok(vec![solution(&[(0, "0x01")], &[])]),
                Ok(vec![solution(&[(0, "0x02")], &[])]),
            ],
            vec![
                Ok(vec![person_hydration(0, "0x01", "Alice")]),
                Ok(vec![person_hydration(0, "0x02", "Bob")]),
            ],
        );
        let context = database.transaction_context(TxType::Read).await.unwrap();
        context.execute_match(&registry, &validated).await.unwrap();
        context.execute_match(&registry, &validated).await.unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events.opens, 1);
        assert_eq!(events.solution_statements.len(), 2);
        assert_eq!(events.hydration_statements.len(), 2);
        assert_eq!((events.closes, events.commits, events.rollbacks), (0, 0, 0));
    }

    #[tokio::test]
    async fn borrowed_non_read_context_fails_before_data_statements_and_remains_caller_owned() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let (database, events) = database(CapabilitySet::all(), vec![], vec![]);
        let context = database.transaction_context(TxType::Write).await.unwrap();

        let error = context
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "borrowed_target_not_read_only");
        assert_eq!(
            match_error(&error).category(),
            MatchErrorCategory::InvalidPlan
        );
        {
            let events = events.lock().unwrap();
            assert_eq!((events.opens, events.closes), (1, 0));
            assert!(events.solution_statements.is_empty());
            assert!(events.hydration_statements.is_empty());
        }

        context.close().await.unwrap();
        assert_eq!(events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn borrowed_context_survives_decode_cancel_and_timeout_failures_atomically() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![
                Ok(vec![AnswerItem::Row(serde_json::json!({"bindings": []}))]),
                Ok(vec![solution(&[(0, "0x04")], &[])]),
            ],
            vec![Ok(vec![person_hydration(0, "0x04", "Dana")])],
        );
        let context = database.transaction_context(TxType::Read).await.unwrap();

        let error = context
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "missing_provider_binding");

        let cancellation = AnswerCancellation::default();
        cancellation.cancel();
        let error = context
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(10, 4096, Duration::from_secs(1), cancellation),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "provider_cancelled");

        let error = context
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    4096,
                    Duration::ZERO,
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "transaction_deadline_exceeded");

        context.execute_match(&registry, &validated).await.unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events.opens, 1);
        assert_eq!(events.solution_statements.len(), 2);
        assert_eq!(events.hydration_statements.len(), 1);
        assert_eq!((events.closes, events.commits, events.rollbacks), (0, 0, 0));
    }

    #[tokio::test]
    async fn provider_order_drift_is_rejected_after_complete_batch_hydration() {
        let registry = person_registry();
        let descriptor = registry.descriptor_id("person").unwrap();
        let mut request = person_request(&registry, RowCardinality::BoundedMany, 2);
        let MatchOperation::FetchRows { order, .. } = &mut request.operation else {
            unreachable!()
        };
        order.push(MatchOrder {
            field: BoundFieldId::new(
                BindingId::new(0),
                registry.field_id(&descriptor, "name").unwrap(),
            ),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        });
        let validated = validate_match_request(&registry, request).unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x01")], &[]),
            ])],
            vec![Ok(vec![
                person_hydration(0, "0x02", "Bob"),
                person_hydration(0, "0x01", "Alice"),
            ])],
        );
        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "unstable_provider_order");
        assert_eq!(events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn relation_edge_evidence_is_decoded_with_complete_roles_and_players() {
        let registry = person_registry();
        registry
            .register_relation(RelationDescriptor {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("code")],
                roles: vec![RoleDescriptor {
                    role_name: "employee".into(),
                    player_type_names: vec!["person".into()],
                    cardinality: Some((1, Some(1))),
                    ..RoleDescriptor::default()
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let person = registry.descriptor_id("person").unwrap();
        let employment = registry.descriptor_id("employment").unwrap();
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: BindingId::new(0),
                        descriptor: person,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: BindingId::new(1),
                        descriptor: employment.clone(),
                        thing_kind: ThingKind::Relation,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: Some(MatchExpr::RoleEdge {
                    id: RoleEdgeId::new(0),
                    relation: BindingId::new(1),
                    role: registry.role_id(&employment, "employee").unwrap(),
                    player: BindingId::new(0),
                }),
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![
                        FetchSlot::One {
                            binding: BindingId::new(0),
                        },
                        FetchSlot::One {
                            binding: BindingId::new(1),
                        },
                    ],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
            },
        );
        let validated = validate_match_request(&registry, request).unwrap();
        let relation = AnswerItem::Document(serde_json::json!({
            "binding": 1,
            "concept_id": "0x10",
            "concrete_type": "employment",
            "attributes": {"code": "E-1"},
            "roles": [{
                "role": "employment:employee",
                "player_concept_id": "0x01",
                "player_concrete_type": "person",
                "attributes": {"name": "Alice"}
            }]
        }));
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01"), (1, "0x10")], &[])])],
            vec![
                Ok(vec![person_hydration(0, "0x01", "Alice")]),
                Ok(vec![relation]),
            ],
        );

        database.execute_match(&registry, &validated).await.unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events.solution_statements.len(), 1);
        assert_eq!(events.hydration_statements.len(), 2);
        assert_eq!(events.hydration_statements[0].targets.len(), 1);
        assert_eq!(
            events.hydration_statements[0].targets[0].kind,
            TypedThingKind::Entity
        );
        assert_eq!(events.hydration_statements[1].targets.len(), 1);
        assert_eq!(
            events.hydration_statements[1].targets[0].kind,
            TypedThingKind::Relation
        );
        assert_eq!(
            events.hydration_statements[1].targets[0].concrete_descriptors[0].roles[0].role_name,
            "employee"
        );
    }
}
