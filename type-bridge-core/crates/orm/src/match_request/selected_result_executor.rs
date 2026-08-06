//! End-to-end selected-row execution over bounded typed provider statements.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::task::Poll;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticDetailValue};
use type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS;
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::query_plan::QueryInvocation;
use type_bridge_contract::query_remote_v2::RemoteReplyDecodeLimitsV2;
use type_bridge_core_lib::ast::{
    TypedFetchRows, TypedHydrateThings, TypedHydrationDescriptor, TypedHydrationField,
    TypedHydrationRole, TypedHydrationTarget, TypedPageRematch, TypedRootScan, TypedThingKind,
};

use super::capability::CapabilitySet;
use super::error::{MatchError, MatchErrorCategory, MatchErrorPath, MatchErrorPathSegment};
use super::ids::{BindingId, DescriptorId, FieldId, RoleEdgeId, RoleId};
use super::lowering::{
    LoweredMatchExecution, LoweredReduceGroup, LoweredReduceInput, LoweredReduceTerm, ReduceDomain,
    lower_match_execution, preflight_released_match_execution,
};
use super::model::{
    FetchShape, FetchSlot, MatchExpr, MatchOperation, Reduction, RowCardinality, ThingKind, Window,
};
use super::result::{
    BoundConceptEvidence, ConceptId, HydratedAttribute, HydratedRole, HydratedRolePlayer,
    HydratedThing, ProviderResultEvidence, ProviderSolutionEvidence, ReducedValue, ReductionRow,
    ValidatedMatchResult,
};
use super::result_validation::{
    ResultValidationLimits, canonicalize_provider_attribute_value, exactly_one_cardinality_error,
    validate_provider_result_with_limits, validated_match_result_from_v2,
};
use super::validation::ValidatedMatchRequest;
use crate::_descriptor::{TypeDescriptor, TypeDescriptorRef};
use crate::_registry::DescriptorRegistry;
use crate::error::OrmError;
use crate::query_v2::{
    QueryV2ExecutionError, execute_validated_model_query_borrowed,
    execute_validated_model_query_with_statement_limit,
};
use crate::query_v2_adapter::{
    AdaptedMatchRequest, MatchRequestAdaptation, MatchRequestAdapterAuthority, adapt_match_request,
};
use crate::session::backend::{
    AnswerCancellation, AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits,
    MAX_ERROR_DRAIN_BYTES, MAX_ERROR_DRAIN_ITEMS, QueryV2AnswerLimits, TxType,
};
use crate::session::{Database, Transaction, TransactionContext};
use crate::value::AttributeValue;

const MAX_PROCESSED_ITEMS: u64 = 100_000;
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSACTION_DURATION: Duration = Duration::from_secs(30);
const OWNED_TRANSACTION_CLOSE_GRACE: Duration = Duration::from_secs(1);
const TUPLE_PROOF_ROWS: u64 = 2;
const TUPLE_PROOF_ROW_ENVELOPE_BYTES: u64 = 64;
// The fixed allowance covers JSON keys, the IID prefix, column name, category,
// separators, and escaping. The concrete TypeDB label is request-derived
// separately so released V1 schemas are not subjected to a V2 label ceiling.
const TUPLE_PROOF_BINDING_FIXED_BYTES: u64 = 192 + MAX_THING_IID_HEX_DIGITS as u64;
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

#[derive(Debug, Clone, Copy)]
struct StatementResourceLimits {
    max_items: u64,
    max_bytes: u64,
    charge_to_caller: bool,
}

struct FiniteStatementLimits {
    provider: BoundedAnswerLimits,
    resources: StatementResourceLimits,
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

    fn begin_statement(
        &mut self,
        provider_max_items: u64,
    ) -> Result<FiniteStatementLimits, OrmError> {
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
        let max_items = provider_max_items.min(self.remaining_items);
        let max_bytes = self.remaining_bytes;
        Ok(FiniteStatementLimits {
            provider: BoundedAnswerLimits {
                max_items,
                max_bytes,
                deadline: self.deadline,
                cancellation: self.cancellation.clone(),
            },
            resources: StatementResourceLimits {
                max_items,
                max_bytes,
                charge_to_caller: true,
            },
        })
    }

    fn begin_exactly_one_proof(
        &self,
        provider_max_items: u64,
        max_bytes: u64,
    ) -> FiniteStatementLimits {
        // The tuple verifier is internal overhead added after the released V1
        // contract. Give it an independent structural ceiling so it neither
        // weakens safety nor spends the caller's statement/item/byte budgets.
        let max_items = provider_max_items.min(TUPLE_PROOF_ROWS);
        FiniteStatementLimits {
            provider: BoundedAnswerLimits {
                max_items,
                max_bytes,
                deadline: self.deadline,
                cancellation: self.cancellation.clone(),
            },
            resources: StatementResourceLimits {
                max_items,
                max_bytes,
                charge_to_caller: false,
            },
        }
    }

    const fn remaining_items(&self) -> u64 {
        self.remaining_items
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
        // Preserve the released successful-execution cleanup race: poll close
        // once, then let the invocation's cancellation/deadline bound it.
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

async fn dispatch_failed_execution_close(
    mut transaction: Transaction,
) -> Option<Result<(), OrmError>> {
    let mut close = Box::pin(async move {
        transaction
            .close()
            .await
            .map_err(provider_transaction_close_error)
    });
    let immediate = std::future::poll_fn(|context| {
        Poll::Ready(match close.as_mut().poll(context) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        })
    })
    .await;
    if immediate.is_none() {
        let _cleanup_task = tokio::spawn(async move {
            match tokio::time::timeout(OWNED_TRANSACTION_CLOSE_GRACE, close).await {
                Ok(Ok(())) => {}
                Ok(Err(close_error)) => tracing::warn!(
                    %close_error,
                    "owned match transaction background cleanup failed"
                ),
                Err(_) => {
                    tracing::warn!("owned match transaction background cleanup exceeded its grace")
                }
            }
        });
    }
    immediate
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

    /// Execute a released request through its production V2 compatibility
    /// program.
    ///
    /// Registries derived from a verified installed runtime projection already
    /// carry the complete generated-client authority and use the direct typed
    /// executor. Dynamic registries continue through the compatibility adapter.
    pub(crate) async fn execute_compatible_owned(
        &self,
        database: &Database,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let registry = self.registry.owned_registry_snapshot()?;
        let fenced = self.with_registry(&registry);
        fenced
            .execute_compatible_owned_fenced(database, validated)
            .await
    }

    async fn execute_compatible_owned_fenced(
        &self,
        database: &Database,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        self.preflight_compatibility(validated)?;
        if self.registry.uses_installed_projection_native_execution() {
            return self.execute_owned(database, validated).await;
        }
        let authority = MatchRequestAdapterAuthority::from_registry(self.registry)
            .map_err(adapter_diagnostic)?;
        match adapt_match_request(
            validated,
            self.registry,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .map_err(adapter_diagnostic)?
        {
            MatchRequestAdaptation::Adapted(adapted) => {
                self.execute_adapted_owned(database, validated, &adapted)
                    .await
            }
            MatchRequestAdaptation::LegacyRequired(_) | MatchRequestAdaptation::NativeOnly => {
                self.execute_owned(database, validated).await
            }
        }
    }

    /// Borrowed counterpart of [`Self::execute_compatible_owned`].
    pub(crate) async fn execute_compatible_borrowed(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let registry = self.registry.owned_registry_snapshot()?;
        let fenced = self.with_registry(&registry);
        fenced
            .execute_compatible_borrowed_fenced(context, validated)
            .await
    }

    async fn execute_compatible_borrowed_fenced(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        self.preflight_compatibility(validated)?;
        if context.tx_type() != TxType::Read {
            return Err(MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "borrowed_target_not_read_only",
                "selected-row execution requires a borrowed read transaction",
            )
            .at(MatchErrorPathSegment::Operation)
            .into());
        }
        if self.registry.uses_installed_projection_native_execution() {
            return self.execute_borrowed(context, validated).await;
        }
        let authority = MatchRequestAdapterAuthority::from_registry(self.registry)
            .map_err(adapter_diagnostic)?;
        match adapt_match_request(
            validated,
            self.registry,
            &authority.context(),
            StructuralLimits::CANONICAL,
        )
        .map_err(adapter_diagnostic)?
        {
            MatchRequestAdaptation::Adapted(adapted) => {
                self.execute_adapted_borrowed(context, validated, &adapted)
                    .await
            }
            MatchRequestAdaptation::LegacyRequired(_) | MatchRequestAdaptation::NativeOnly => {
                self.execute_borrowed(context, validated).await
            }
        }
    }

    fn preflight_compatibility(&self, validated: &ValidatedMatchRequest) -> Result<(), OrmError> {
        validated.recheck_schema(self.registry)?;
        validated.require_capabilities(&self.available_capabilities)?;
        preflight_released_match_execution(self.registry, validated)?;
        Ok(())
    }

    fn with_registry<'snapshot>(
        &self,
        registry: &'snapshot DescriptorRegistry,
    ) -> SelectedResultExecutor<'snapshot> {
        SelectedResultExecutor {
            registry,
            available_capabilities: self.available_capabilities.clone(),
            limits: self.limits.clone(),
        }
    }

    async fn execute_adapted_owned(
        &self,
        database: &Database,
        released: &ValidatedMatchRequest,
        adapted: &AdaptedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let deadline = tokio::time::Instant::now()
            .checked_add(self.limits.timeout)
            .map(tokio::time::Instant::into_std);
        let lifecycle = ExecutionBudget::new(&self.limits, deadline);
        let mut transaction = lifecycle
            .await_provider(async {
                database
                    .read_transaction()
                    .await
                    .map_err(provider_transaction_open_error)
            })
            .await?;
        let execution = self
            .execute_adapted_transaction(&mut transaction, released, adapted, deadline)
            .await;
        let result = match execution {
            Err(error) => {
                if let Some(Err(close_error)) = dispatch_failed_execution_close(transaction).await {
                    tracing::warn!(
                        %close_error,
                        execution_error = %error,
                        "owned adapted match execution failed and transaction cleanup also failed"
                    );
                }
                return Err(error);
            }
            Ok(result) => result,
        };
        let close = lifecycle
            .await_cleanup(async {
                transaction
                    .close()
                    .await
                    .map_err(provider_transaction_close_error)
            })
            .await;
        match close {
            Err(error) => Err(error),
            Ok(()) => Ok(result),
        }
    }

    async fn execute_adapted_transaction(
        &self,
        transaction: &mut Transaction,
        released: &ValidatedMatchRequest,
        adapted: &AdaptedMatchRequest,
        deadline: Option<Instant>,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let invocation =
            QueryInvocation::new(adapted.validated().plan(), adapted.operation(), Vec::new())
                .map_err(adapter_diagnostic)?;
        let (limits, reply_limits) = self.compatibility_limits(deadline);
        let outcome = execute_validated_model_query_with_statement_limit(
            transaction,
            adapted.validated(),
            &invocation,
            limits,
            reply_limits,
            self.limits.max_statements,
        )
        .await
        .map_err(compatibility_execution_error)?;
        validated_match_result_from_v2(self.registry, released, outcome).map_err(OrmError::from)
    }

    async fn execute_adapted_borrowed(
        &self,
        context: &TransactionContext,
        released: &ValidatedMatchRequest,
        adapted: &AdaptedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let deadline = tokio::time::Instant::now()
            .checked_add(self.limits.timeout)
            .map(tokio::time::Instant::into_std);
        let invocation =
            QueryInvocation::new(adapted.validated().plan(), adapted.operation(), Vec::new())
                .map_err(adapter_diagnostic)?;
        let (limits, reply_limits) = self.compatibility_limits(deadline);
        let outcome = execute_validated_model_query_borrowed(
            context,
            adapted.validated(),
            &invocation,
            limits,
            reply_limits,
            self.limits.max_statements,
        )
        .await
        .map_err(compatibility_execution_error)?;
        validated_match_result_from_v2(self.registry, released, outcome).map_err(OrmError::from)
    }

    fn compatibility_limits(
        &self,
        deadline: Option<Instant>,
    ) -> (QueryV2AnswerLimits, RemoteReplyDecodeLimitsV2) {
        (
            QueryV2AnswerLimits {
                answer: BoundedAnswerLimits {
                    max_items: self.limits.max_items,
                    max_bytes: self.limits.max_bytes,
                    deadline,
                    cancellation: self.limits.cancellation.clone(),
                },
                max_collection_members: self.limits.max_collected_concepts,
            },
            RemoteReplyDecodeLimitsV2 {
                max_bytes: self.limits.max_bytes,
                max_items: self.limits.max_items,
                max_collection_members: self.limits.max_collected_concepts,
                max_graph_nodes: self.limits.max_hydrated_things,
                max_attribute_values: self.limits.max_attribute_values,
                max_role_players: self.limits.max_hydrated_things,
            },
        )
    }

    pub(crate) async fn execute_owned(
        &self,
        database: &Database,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let statement = self.preflight(validated)?;
        let exactly_one_selection = exactly_one_tuple_proof_selection(&statement);
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
        let execution = async {
            let evidence = self
                .collect_from_transaction(&mut transaction, validated, statement, &mut budget)
                .await?;
            let result = self.validate(validated, evidence, &budget)?;
            if let Some(selection) = exactly_one_selection {
                self.prove_exactly_one_transaction(&mut transaction, selection, &mut budget)
                    .await?;
            }
            Ok(result)
        }
        .await;
        let result = match execution {
            Err(error) => {
                if let Some(Err(close_error)) = dispatch_failed_execution_close(transaction).await {
                    tracing::warn!(
                        %close_error,
                        execution_error = %error,
                        "owned match execution failed and transaction cleanup also failed"
                    );
                }
                return Err(error);
            }
            Ok(result) => result,
        };
        // Released V1 callers observe close errors, cancellation, and the
        // original transaction deadline after successful execution.
        let close = budget
            .await_cleanup(async {
                transaction
                    .close()
                    .await
                    .map_err(provider_transaction_close_error)
            })
            .await;
        match close {
            Err(error) => Err(error),
            Ok(()) => Ok(result),
        }
    }

    pub(crate) async fn execute_borrowed(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let statement = self.preflight(validated)?;
        let exactly_one_selection = exactly_one_tuple_proof_selection(&statement);
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
        let result = self.validate(validated, evidence, &budget)?;
        if let Some(selection) = exactly_one_selection {
            self.prove_exactly_one_context(context, selection, &mut budget)
                .await?;
        }
        Ok(result)
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
            LoweredMatchExecution::ExactlyOneBy { evidence, .. } => {
                self.collect_rows_transaction(transaction, validated, evidence, budget)
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
            LoweredMatchExecution::ReduceBy {
                root,
                group,
                scan,
                rematch,
                terms,
            } => {
                self.collect_reduce_transaction(
                    transaction,
                    validated,
                    root,
                    group,
                    scan,
                    rematch,
                    &terms,
                    budget,
                )
                .await
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
            LoweredMatchExecution::ExactlyOneBy { evidence, .. } => {
                self.collect_rows_context(context, validated, evidence, budget)
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
            LoweredMatchExecution::ReduceBy {
                root,
                group,
                scan,
                rematch,
                terms,
            } => {
                self.collect_reduce_context(
                    context, validated, root, group, scan, rematch, &terms, budget,
                )
                .await
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
        let scan_mode = solution_scan_mode(validated, &statement)?;
        statement.offset = 0;
        statement.limit = scan_mode.statement_limit(self.limits.max_items.max(1));
        let mut solutions = SolutionConsumer::new(validated, scan_mode.stops_at_prefix())?;
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_statement(statement.limit)?;
        let scan_limit = resources.max_items;
        let mut draining = DrainingConsumer::new(&mut solutions, resources);
        let stats = budget
            .await_provider(async {
                transaction
                    .query_typed_bounded(&statement, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
        require_selected_solution_scan_proof(
            stats,
            scan_limit,
            scan_mode,
            solutions.reached_prefix(),
        )?;
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
            let FiniteStatementLimits {
                provider: limits,
                resources,
            } = budget.begin_statement(hydration_answer_limit(&batch)?)?;
            let mut draining = DrainingConsumer::new(&mut hydration, resources);
            let stats = budget
                .await_provider(async {
                    transaction
                        .hydrate_typed_bounded(&batch, limits, &mut draining)
                        .await
                        .map_err(provider_statement_error)
                })
                .await;
            let stats = draining.complete(stats, budget)?;
            require_provider_exhaustion(stats)?;
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
        let scan_mode = solution_scan_mode(validated, &statement)?;
        statement.offset = 0;
        statement.limit = scan_mode.statement_limit(self.limits.max_items.max(1));
        let mut solutions = SolutionConsumer::new(validated, scan_mode.stops_at_prefix())?;
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_statement(statement.limit)?;
        let scan_limit = resources.max_items;
        let mut draining = DrainingConsumer::new(&mut solutions, resources);
        let stats = budget
            .await_provider(async {
                context
                    .query_typed_bounded(&statement, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
        require_selected_solution_scan_proof(
            stats,
            scan_limit,
            scan_mode,
            solutions.reached_prefix(),
        )?;
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
            let FiniteStatementLimits {
                provider: limits,
                resources,
            } = budget.begin_statement(hydration_answer_limit(&batch)?)?;
            let mut draining = DrainingConsumer::new(&mut hydration, resources);
            let stats = budget
                .await_provider(async {
                    context
                        .hydrate_typed_bounded(&batch, limits, &mut draining)
                        .await
                        .map_err(provider_statement_error)
                })
                .await;
            let stats = draining.complete(stats, budget)?;
            require_provider_exhaustion(stats)?;
        }
        rows_evidence_from_hydration(validated, solutions, hydration.finish()?)
    }

    async fn prove_exactly_one_transaction(
        &self,
        transaction: &mut Transaction,
        selection: TypedFetchRows,
        budget: &mut ExecutionBudget,
    ) -> Result<(), OrmError> {
        if !transaction.supports_exactly_one_tuple_proof()? {
            return Ok(());
        }
        let tuples = self
            .scan_tuples_transaction(transaction, selection, budget)
            .await?;
        if let Some(error) = exactly_one_cardinality_error(tuples) {
            return Err(error.into());
        }
        Ok(())
    }

    async fn prove_exactly_one_context(
        &self,
        context: &TransactionContext,
        selection: TypedFetchRows,
        budget: &mut ExecutionBudget,
    ) -> Result<(), OrmError> {
        if !budget
            .await_provider(context.supports_exactly_one_tuple_proof())
            .await?
        {
            return Ok(());
        }
        let tuples = self.scan_tuples_context(context, selection, budget).await?;
        if let Some(error) = exactly_one_cardinality_error(tuples) {
            return Err(error.into());
        }
        Ok(())
    }

    async fn scan_tuples_transaction(
        &self,
        transaction: &mut Transaction,
        selection: TypedFetchRows,
        budget: &mut ExecutionBudget,
    ) -> Result<usize, OrmError> {
        let mut tuples = TupleConsumer::new(&selection.projection);
        let max_bytes = exactly_one_proof_byte_limit(self.registry, &selection)?;
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_exactly_one_proof(selection.limit, max_bytes);
        let mut draining = DrainingConsumer::new(&mut tuples, resources);
        let stats = budget
            .await_provider(async {
                transaction
                    .query_tuple_typed_bounded(&selection, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
        require_provider_exhaustion(stats)?;
        if stats.processed_items > selection.limit {
            return Err(decode_error(
                "tuple_scan_limit_mismatch",
                "provider returned more distinct tuples than the typed statement limit",
            ));
        }
        Ok(tuples.len())
    }

    async fn scan_tuples_context(
        &self,
        context: &TransactionContext,
        selection: TypedFetchRows,
        budget: &mut ExecutionBudget,
    ) -> Result<usize, OrmError> {
        let mut tuples = TupleConsumer::new(&selection.projection);
        let max_bytes = exactly_one_proof_byte_limit(self.registry, &selection)?;
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_exactly_one_proof(selection.limit, max_bytes);
        let mut draining = DrainingConsumer::new(&mut tuples, resources);
        let stats = budget
            .await_provider(async {
                context
                    .query_tuple_typed_bounded(&selection, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
        require_provider_exhaustion(stats)?;
        if stats.processed_items > selection.limit {
            return Err(decode_error(
                "tuple_scan_limit_mismatch",
                "provider returned more distinct tuples than the typed statement limit",
            ));
        }
        Ok(tuples.len())
    }

    async fn scan_roots_transaction(
        &self,
        transaction: &mut Transaction,
        mut scan: TypedRootScan,
        root: BindingId,
        purpose: RootScanPurpose,
        budget: &mut ExecutionBudget,
    ) -> Result<Vec<String>, OrmError> {
        let scan_limit = budget.remaining_items();
        if purpose == RootScanPurpose::Count {
            scan.limit = Some(scan_limit.max(1));
        }
        let statement_limit = scan.limit.ok_or_else(|| {
            decode_error(
                "root_scan_missing_provider_limit",
                "typed root statement is missing its provider-owned answer ceiling",
            )
        })?;
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_statement(statement_limit)?;
        let retain_limit = (purpose != RootScanPurpose::Count)
            .then_some(scan.limit)
            .flatten();
        let mut roots = RootConsumer::new(root, retain_limit);
        let mut draining = DrainingConsumer::new(&mut roots, resources);
        let stats = budget
            .await_provider(async {
                transaction
                    .query_root_typed_bounded(&scan, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
        if purpose == RootScanPurpose::Count {
            require_solution_scan_proof(stats, scan_limit)?;
        } else {
            require_provider_exhaustion(stats)?;
        }
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
        let scan_limit = budget.remaining_items();
        if purpose == RootScanPurpose::Count {
            scan.limit = Some(scan_limit.max(1));
        }
        let statement_limit = scan.limit.ok_or_else(|| {
            decode_error(
                "root_scan_missing_provider_limit",
                "typed root statement is missing its provider-owned answer ceiling",
            )
        })?;
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_statement(statement_limit)?;
        let retain_limit = (purpose != RootScanPurpose::Count)
            .then_some(scan.limit)
            .flatten();
        let mut roots = RootConsumer::new(root, retain_limit);
        let mut draining = DrainingConsumer::new(&mut roots, resources);
        let stats = budget
            .await_provider(async {
                context
                    .query_root_typed_bounded(&scan, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
        if purpose == RootScanPurpose::Count {
            require_solution_scan_proof(stats, scan_limit)?;
        } else {
            require_provider_exhaustion(stats)?;
        }
        Ok(roots.finish())
    }

    #[allow(clippy::too_many_arguments)]
    async fn collect_reduce_transaction(
        &self,
        transaction: &mut Transaction,
        validated: &ValidatedMatchRequest,
        root: BindingId,
        group: Option<LoweredReduceGroup>,
        scan: TypedRootScan,
        rematch: Option<TypedPageRematch>,
        terms: &[LoweredReduceTerm],
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        let roots = self
            .scan_roots_transaction(transaction, scan, root, RootScanPurpose::Count, budget)
            .await?;
        let solutions = if let Some(mut rematch) = rematch
            && !roots.is_empty()
        {
            rematch.root_concept_ids.clone_from(&roots);
            let mut consumer = PageRematchConsumer::new(
                self.registry,
                validated,
                root,
                &roots,
                self.limits.semantic_limits(),
            )?;
            let scan_limit = budget.remaining_items();
            let FiniteStatementLimits {
                provider: limits,
                resources,
            } = budget.begin_statement(scan_limit.max(1))?;
            let mut draining = DrainingConsumer::new(&mut consumer, resources);
            let stats = budget
                .await_provider(async {
                    transaction
                        .rematch_page_typed_bounded(&rematch, limits, &mut draining)
                        .await
                        .map_err(provider_statement_error)
                })
                .await;
            let stats = draining.complete(stats, budget)?;
            require_solution_scan_proof(stats, scan_limit)?;
            consumer.finish()?
        } else {
            Vec::new()
        };
        let rows = reduce_rows(
            root,
            group.as_ref(),
            terms,
            &roots,
            &solutions,
            usize::try_from(self.limits.max_items).unwrap_or(usize::MAX),
        )?;
        Ok(reduction_evidence(validated, root, group, rows))
    }

    #[allow(clippy::too_many_arguments)]
    async fn collect_reduce_context(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
        root: BindingId,
        group: Option<LoweredReduceGroup>,
        scan: TypedRootScan,
        rematch: Option<TypedPageRematch>,
        terms: &[LoweredReduceTerm],
        budget: &mut ExecutionBudget,
    ) -> Result<ProviderResultEvidence, OrmError> {
        let roots = self
            .scan_roots_context(context, scan, root, RootScanPurpose::Count, budget)
            .await?;
        let solutions = if let Some(mut rematch) = rematch
            && !roots.is_empty()
        {
            rematch.root_concept_ids.clone_from(&roots);
            let mut consumer = PageRematchConsumer::new(
                self.registry,
                validated,
                root,
                &roots,
                self.limits.semantic_limits(),
            )?;
            let scan_limit = budget.remaining_items();
            let FiniteStatementLimits {
                provider: limits,
                resources,
            } = budget.begin_statement(scan_limit.max(1))?;
            let mut draining = DrainingConsumer::new(&mut consumer, resources);
            let stats = budget
                .await_provider(async {
                    context
                        .rematch_page_typed_bounded(&rematch, limits, &mut draining)
                        .await
                        .map_err(provider_statement_error)
                })
                .await;
            let stats = draining.complete(stats, budget)?;
            require_solution_scan_proof(stats, scan_limit)?;
            consumer.finish()?
        } else {
            Vec::new()
        };
        let rows = reduce_rows(
            root,
            group.as_ref(),
            terms,
            &roots,
            &solutions,
            usize::try_from(self.limits.max_items).unwrap_or(usize::MAX),
        )?;
        Ok(reduction_evidence(validated, root, group, rows))
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
            .scan_roots_transaction(transaction, selection, root, RootScanPurpose::Page, budget)
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
        let scan_limit = budget.remaining_items();
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_statement(scan_limit.max(1))?;
        let mut draining = DrainingConsumer::new(&mut consumer, resources);
        let stats = budget
            .await_provider(async {
                transaction
                    .rematch_page_typed_bounded(&rematch, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
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
            .scan_roots_context(context, selection, root, RootScanPurpose::Page, budget)
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
        let scan_limit = budget.remaining_items();
        let FiniteStatementLimits {
            provider: limits,
            resources,
        } = budget.begin_statement(scan_limit.max(1))?;
        let mut draining = DrainingConsumer::new(&mut consumer, resources);
        let stats = budget
            .await_provider(async {
                context
                    .rematch_page_typed_bounded(&rematch, limits, &mut draining)
                    .await
                    .map_err(provider_statement_error)
            })
            .await;
        let stats = draining.complete(stats, budget)?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SolutionScanMode {
    ProviderTerminal {
        statement_limit: u64,
        released_prefix: u64,
    },
    ReleasedPrefix,
}

impl SolutionScanMode {
    const fn statement_limit(self, released_scan_limit: u64) -> u64 {
        match self {
            Self::ProviderTerminal {
                statement_limit, ..
            } => statement_limit,
            Self::ReleasedPrefix => released_scan_limit,
        }
    }

    const fn stops_at_prefix(self) -> bool {
        matches!(self, Self::ReleasedPrefix)
    }
}

fn solution_scan_mode(
    validated: &ValidatedMatchRequest,
    statement: &TypedFetchRows,
) -> Result<SolutionScanMode, OrmError> {
    let released_prefix = released_solution_prefix(validated)?;
    if provider_distinct_matches_public_projection(statement) {
        return Ok(SolutionScanMode::ProviderTerminal {
            statement_limit: released_prefix.max(1),
            released_prefix,
        });
    }
    Ok(SolutionScanMode::ReleasedPrefix)
}

fn released_solution_prefix(validated: &ValidatedMatchRequest) -> Result<u64, OrmError> {
    let MatchOperation::FetchRows {
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
    Ok(match cardinality {
        RowCardinality::ExactlyOne => 2,
        RowCardinality::BoundedMany => window.offset.saturating_add(window.limit),
    })
}

fn provider_distinct_matches_public_projection(statement: &TypedFetchRows) -> bool {
    if !statement.distinct {
        return false;
    }
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
    if targets.len() != statement.targets.len()
        || projection.len() != statement.projection.len()
        || targets != projection
    {
        return false;
    }

    // The typed compiler also SELECTs every order field. Lowering admits only
    // at-most-one scalar order fields, so each is functionally determined by
    // its projected owner and cannot split one public identity into extra rows.
    statement.order.iter().all(|order| {
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

fn exactly_one_tuple_proof_selection(execution: &LoweredMatchExecution) -> Option<TypedFetchRows> {
    let LoweredMatchExecution::ExactlyOneBy {
        selection,
        evidence,
    } = execution
    else {
        return None;
    };
    // A terminal DISTINCT evidence stream over exactly the public projection
    // already proves V1 cardinality. Re-query only when hidden witnesses can
    // duplicate a public row and the released prefix scan cannot prove it.
    (!provider_distinct_matches_public_projection(evidence)).then(|| selection.clone())
}

fn require_selected_solution_scan_proof(
    stats: crate::session::backend::BoundedAnswerStats,
    max_items: u64,
    mode: SolutionScanMode,
    reached_prefix: bool,
) -> Result<(), OrmError> {
    match mode {
        SolutionScanMode::ReleasedPrefix if stats.stopped_early && reached_prefix => return Ok(()),
        SolutionScanMode::ReleasedPrefix => {
            if stats.processed_items >= max_items {
                return Err(solution_scan_limit_error(max_items));
            }
            require_provider_exhaustion(stats)?;
        }
        SolutionScanMode::ProviderTerminal {
            released_prefix, ..
        } => {
            // Keep the released caller-budget precedence even though a wider
            // finite TypeQL limit can sometimes prove natural EOF exactly at
            // the tightened item ceiling.
            if max_items < released_prefix && stats.processed_items >= max_items {
                return Err(solution_scan_limit_error(max_items));
            }
            require_provider_exhaustion(stats)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
enum CollectedInputs {
    Long(Vec<i64>),
    Double(Vec<f64>),
}

impl CollectedInputs {
    fn new(domain: ReduceDomain) -> Self {
        match domain {
            ReduceDomain::Long => Self::Long(Vec::new()),
            ReduceDomain::Double => Self::Double(Vec::new()),
        }
    }

    fn push(&mut self, value: &AttributeValue) -> Result<(), OrmError> {
        match (self, value) {
            (Self::Long(values), AttributeValue::Long(value)) => {
                values.push(*value);
                Ok(())
            }
            (Self::Double(values), AttributeValue::Double(value)) => {
                if !value.is_finite() {
                    return Err(decode_error(
                        "reduction_input_not_finite",
                        "provider reducer input is not a finite double",
                    ));
                }
                values.push(*value);
                Ok(())
            }
            _ => Err(decode_error(
                "reduction_input_domain",
                "provider reducer input does not match its validated domain",
            )),
        }
    }

    fn as_doubles(&self) -> Vec<f64> {
        match self {
            Self::Long(values) => values.iter().map(|value| *value as f64).collect(),
            Self::Double(values) => values.clone(),
        }
    }
}

fn finite_reduction_double(value: f64) -> Result<ReducedValue, OrmError> {
    if !value.is_finite() {
        return Err(resource_error(
            "reduction_overflow",
            "reduction result left the finite double domain",
        ));
    }
    Ok(ReducedValue::Double(Some(value)))
}

/// Reduce one collected input stream with canonical semantics: sums stay
/// total (zero on empty), extrema and statistical reducers are absent on
/// empty streams, sample standard deviation requires two values, and all
/// double results must remain finite.
fn reduce_collected(
    reduction: Reduction,
    collected: &CollectedInputs,
) -> Result<ReducedValue, OrmError> {
    match reduction {
        Reduction::Count => Err(decode_error(
            "reduction_input_domain",
            "count consumes the distinct root stream, not a field input",
        )),
        Reduction::Sum => match collected {
            CollectedInputs::Long(values) => {
                let mut total = 0_i64;
                for value in values {
                    total = total.checked_add(*value).ok_or_else(|| {
                        resource_error(
                            "reduction_overflow",
                            "integer sum left the canonical long domain",
                        )
                    })?;
                }
                Ok(ReducedValue::Long(Some(total)))
            }
            CollectedInputs::Double(values) => finite_reduction_double(values.iter().sum::<f64>()),
        },
        Reduction::Min | Reduction::Max => match collected {
            CollectedInputs::Long(values) => {
                let extreme = if reduction == Reduction::Min {
                    values.iter().min()
                } else {
                    values.iter().max()
                };
                Ok(ReducedValue::Long(extreme.copied()))
            }
            CollectedInputs::Double(values) => {
                let mut extreme: Option<f64> = None;
                for value in values {
                    extreme = Some(match extreme {
                        None => *value,
                        Some(current) if reduction == Reduction::Min => current.min(*value),
                        Some(current) => current.max(*value),
                    });
                }
                Ok(ReducedValue::Double(extreme))
            }
        },
        Reduction::Mean => {
            let values = collected.as_doubles();
            if values.is_empty() {
                return Ok(ReducedValue::Double(None));
            }
            finite_reduction_double(values.iter().sum::<f64>() / values.len() as f64)
        }
        Reduction::Median => {
            let mut values = collected.as_doubles();
            if values.is_empty() {
                return Ok(ReducedValue::Double(None));
            }
            values
                .sort_by(|left, right| left.partial_cmp(right).expect("reducer inputs are finite"));
            let middle = values.len() / 2;
            let median = if values.len() % 2 == 1 {
                values[middle]
            } else {
                (values[middle - 1] + values[middle]) / 2.0
            };
            finite_reduction_double(median)
        }
        Reduction::Std => {
            let values = collected.as_doubles();
            if values.len() < 2 {
                return Ok(ReducedValue::Double(None));
            }
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let variance = values
                .iter()
                .map(|value| {
                    let delta = value - mean;
                    delta * delta
                })
                .sum::<f64>()
                / (values.len() - 1) as f64;
            finite_reduction_double(variance.sqrt())
        }
    }
}

fn reduction_input_value<'a>(
    solution: &'a ProviderSolutionEvidence,
    input: &LoweredReduceInput,
) -> Result<Option<&'a AttributeValue>, OrmError> {
    let thing = solution
        .bindings()
        .iter()
        .find(|bound| bound.binding() == input.binding)
        .map(BoundConceptEvidence::thing)
        .ok_or_else(|| {
            decode_error(
                "reduction_binding_missing",
                "provider solution omitted a reducer input binding",
            )
        })?;
    Ok(thing
        .attributes()
        .iter()
        .find(|attribute| attribute.field() == &input.field)
        .and_then(|attribute| attribute.values().first()))
}

fn reduction_evidence(
    validated: &ValidatedMatchRequest,
    root: BindingId,
    group: Option<LoweredReduceGroup>,
    rows: Vec<ReductionRow>,
) -> ProviderResultEvidence {
    match group {
        None => ProviderResultEvidence::reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            None,
            rows,
        ),
        Some(LoweredReduceGroup::Binding(group)) => ProviderResultEvidence::reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            Some(group),
            rows,
        ),
        Some(LoweredReduceGroup::Field(group)) => ProviderResultEvidence::field_reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            group,
            rows,
        ),
        Some(LoweredReduceGroup::Fields(groups)) => ProviderResultEvidence::field_tuple_reduction(
            validated.request_token(),
            validated.shape_id().clone(),
            root,
            groups,
            rows,
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum AttributeGroupKey {
    String(String),
    Long(i64),
    Double(u64),
    Boolean(bool),
    Date(String),
    DateTime(String),
    DateTimeTz(String),
    Decimal(String),
    Duration(String),
}

fn attribute_group_key(value: &AttributeValue) -> Result<AttributeGroupKey, OrmError> {
    Ok(match value {
        AttributeValue::String(value) => AttributeGroupKey::String(value.clone()),
        AttributeValue::Long(value) => AttributeGroupKey::Long(*value),
        AttributeValue::Double(value) if value.is_finite() => {
            AttributeGroupKey::Double(if *value == 0.0 { 0 } else { value.to_bits() })
        }
        AttributeValue::Double(_) => {
            return Err(decode_error(
                "reduction_group_value_invalid",
                "field-grouped reduction received a non-finite double",
            ));
        }
        AttributeValue::Boolean(value) => AttributeGroupKey::Boolean(*value),
        AttributeValue::Date(value) => AttributeGroupKey::Date(value.clone()),
        AttributeValue::DateTime(value) => AttributeGroupKey::DateTime(value.clone()),
        AttributeValue::DateTimeTZ(value) => AttributeGroupKey::DateTimeTz(value.clone()),
        AttributeValue::Decimal(value) => AttributeGroupKey::Decimal(value.clone()),
        AttributeValue::Duration(value) => AttributeGroupKey::Duration(value.clone()),
    })
}

/// Assemble typed reduction rows over the distinct selected-identity stream.
///
/// The exhaustively proven root scan is authoritative for distinct root
/// counting; solutions contribute each distinct (group, root) pair's input
/// values exactly once, and grouped rows are deterministically ordered by
/// group concept identity.
fn reduce_rows(
    root: BindingId,
    group: Option<&LoweredReduceGroup>,
    terms: &[LoweredReduceTerm],
    roots: &[String],
    solutions: &[ProviderSolutionEvidence],
    max_group_rows: usize,
) -> Result<Vec<ReductionRow>, OrmError> {
    struct GroupAccumulator {
        thing: HydratedThing,
        roots: BTreeSet<String>,
        inputs: Vec<Option<CollectedInputs>>,
    }
    struct FieldGroupAccumulator {
        value: AttributeValue,
        roots: BTreeSet<String>,
        inputs: Vec<Option<CollectedInputs>>,
    }
    struct FieldTupleGroupAccumulator {
        values: Vec<AttributeValue>,
        roots: BTreeSet<String>,
        inputs: Vec<Option<CollectedInputs>>,
    }
    let fresh_inputs = |terms: &[LoweredReduceTerm]| {
        terms
            .iter()
            .map(|term| {
                term.input
                    .as_ref()
                    .map(|input| CollectedInputs::new(input.domain))
            })
            .collect::<Vec<_>>()
    };
    let collect_solution = |accumulated: &mut Vec<Option<CollectedInputs>>,
                            solution: &ProviderSolutionEvidence|
     -> Result<(), OrmError> {
        for (term, collected) in terms.iter().zip(accumulated.iter_mut()) {
            let (Some(input), Some(collected)) = (&term.input, collected) else {
                continue;
            };
            if let Some(value) = reduction_input_value(solution, input)? {
                collected.push(value)?;
            }
        }
        Ok(())
    };
    let finish = |root_count: usize,
                  accumulated: &[Option<CollectedInputs>]|
     -> Result<Vec<ReducedValue>, OrmError> {
        terms
            .iter()
            .zip(accumulated)
            .map(|(term, collected)| match collected {
                None => Ok(ReducedValue::Count(root_count as u64)),
                Some(collected) => reduce_collected(term.reduction, collected),
            })
            .collect()
    };
    let solution_root = |solution: &ProviderSolutionEvidence| -> Result<String, OrmError> {
        solution
            .bindings()
            .iter()
            .find(|bound| bound.binding() == root)
            .map(|bound| bound.thing().concept_id().as_str().to_owned())
            .ok_or_else(|| {
                decode_error(
                    "reduction_binding_missing",
                    "provider solution omitted the reduced root binding",
                )
            })
    };
    match group {
        None => {
            let mut seen = BTreeSet::new();
            let mut accumulated = fresh_inputs(terms);
            for solution in solutions {
                let root_id = solution_root(solution)?;
                if seen.insert(root_id) {
                    collect_solution(&mut accumulated, solution)?;
                }
            }
            let values = finish(roots.len(), &accumulated)?;
            Ok(vec![ReductionRow::new(None, values)])
        }
        Some(LoweredReduceGroup::Binding(group)) => {
            let mut groups: BTreeMap<String, GroupAccumulator> = BTreeMap::new();
            for solution in solutions {
                let root_id = solution_root(solution)?;
                let thing = solution
                    .bindings()
                    .iter()
                    .find(|bound| bound.binding() == *group)
                    .map(BoundConceptEvidence::thing)
                    .ok_or_else(|| {
                        decode_error(
                            "reduction_binding_missing",
                            "provider solution omitted the group binding",
                        )
                    })?;
                let key = thing.concept_id().as_str().to_owned();
                let entry = groups.entry(key).or_insert_with(|| GroupAccumulator {
                    thing: thing.clone(),
                    roots: BTreeSet::new(),
                    inputs: fresh_inputs(terms),
                });
                if entry.roots.insert(root_id) {
                    collect_solution(&mut entry.inputs, solution)?;
                }
            }
            groups
                .into_values()
                .map(|accumulator| {
                    let values = finish(accumulator.roots.len(), &accumulator.inputs)?;
                    Ok(ReductionRow::new(Some(accumulator.thing), values))
                })
                .collect()
        }
        Some(LoweredReduceGroup::Field(group)) => {
            let mut groups: BTreeMap<AttributeGroupKey, FieldGroupAccumulator> = BTreeMap::new();
            for solution in solutions {
                let root_id = solution_root(solution)?;
                let thing = solution
                    .bindings()
                    .iter()
                    .find(|bound| bound.binding() == group.binding)
                    .map(BoundConceptEvidence::thing)
                    .ok_or_else(|| {
                        decode_error(
                            "reduction_binding_missing",
                            "provider solution omitted the field-group owner binding",
                        )
                    })?;
                let Some(attribute) = thing
                    .attributes()
                    .iter()
                    .find(|attribute| attribute.field() == &group.field)
                else {
                    continue;
                };
                for value in attribute.values() {
                    let key = attribute_group_key(value)?;
                    if !groups.contains_key(&key) && groups.len() >= max_group_rows {
                        return Err(resource_error(
                            "reduction_group_limit",
                            "field-grouped reduction exceeded the result-row ceiling",
                        ));
                    }
                    let entry = groups.entry(key).or_insert_with(|| FieldGroupAccumulator {
                        value: value.clone(),
                        roots: BTreeSet::new(),
                        inputs: fresh_inputs(terms),
                    });
                    if entry.roots.insert(root_id.clone()) {
                        collect_solution(&mut entry.inputs, solution)?;
                    }
                }
            }
            groups
                .into_values()
                .map(|accumulator| {
                    let values = finish(accumulator.roots.len(), &accumulator.inputs)?;
                    Ok(ReductionRow::new_field(accumulator.value, values))
                })
                .collect()
        }
        Some(LoweredReduceGroup::Fields(group_fields)) => {
            let mut groups: BTreeMap<Vec<AttributeGroupKey>, FieldTupleGroupAccumulator> =
                BTreeMap::new();
            for solution in solutions {
                let root_id = solution_root(solution)?;
                let mut choices = Vec::with_capacity(group_fields.len());
                let mut omitted = false;
                for group in group_fields {
                    let thing = solution
                        .bindings()
                        .iter()
                        .find(|bound| bound.binding() == group.binding)
                        .map(BoundConceptEvidence::thing)
                        .ok_or_else(|| {
                            decode_error(
                                "reduction_binding_missing",
                                "provider solution omitted a tuple-group field owner binding",
                            )
                        })?;
                    let Some(attribute) = thing
                        .attributes()
                        .iter()
                        .find(|attribute| attribute.field() == &group.field)
                    else {
                        omitted = true;
                        break;
                    };
                    let mut distinct = BTreeMap::new();
                    for value in attribute.values() {
                        distinct
                            .entry(attribute_group_key(value)?)
                            .or_insert_with(|| value.clone());
                    }
                    if distinct.is_empty() {
                        omitted = true;
                        break;
                    }
                    choices.push(distinct.into_iter().collect::<Vec<_>>());
                }
                if omitted {
                    continue;
                }
                let tuple_count = choices.iter().try_fold(1_usize, |count, values| {
                    count.checked_mul(values.len()).ok_or_else(|| {
                        resource_error(
                            "reduction_group_limit",
                            "tuple-field reduction group cardinality overflowed",
                        )
                    })
                })?;
                if tuple_count > max_group_rows {
                    return Err(resource_error(
                        "reduction_group_limit",
                        "tuple-field reduction exceeded the result-row ceiling",
                    ));
                }
                let mut tuples = vec![(Vec::new(), Vec::new())];
                for values in choices {
                    let capacity = tuples.len().checked_mul(values.len()).ok_or_else(|| {
                        resource_error(
                            "reduction_group_limit",
                            "tuple-field reduction group cardinality overflowed",
                        )
                    })?;
                    let mut expanded = Vec::with_capacity(capacity);
                    for (keys, scalars) in &tuples {
                        for (key, scalar) in &values {
                            let mut next_keys = keys.clone();
                            next_keys.push(key.clone());
                            let mut next_scalars = scalars.clone();
                            next_scalars.push(scalar.clone());
                            expanded.push((next_keys, next_scalars));
                        }
                    }
                    tuples = expanded;
                }
                for (key, values) in tuples {
                    if !groups.contains_key(&key) && groups.len() >= max_group_rows {
                        return Err(resource_error(
                            "reduction_group_limit",
                            "tuple-field reduction exceeded the result-row ceiling",
                        ));
                    }
                    let entry = groups
                        .entry(key)
                        .or_insert_with(|| FieldTupleGroupAccumulator {
                            values,
                            roots: BTreeSet::new(),
                            inputs: fresh_inputs(terms),
                        });
                    if entry.roots.insert(root_id.clone()) {
                        collect_solution(&mut entry.inputs, solution)?;
                    }
                }
            }
            groups
                .into_values()
                .map(|accumulator| {
                    let values = finish(accumulator.roots.len(), &accumulator.inputs)?;
                    Ok(ReductionRow::new_fields(accumulator.values, values))
                })
                .collect()
        }
    }
}

fn require_solution_scan_proof(
    stats: crate::session::backend::BoundedAnswerStats,
    max_items: u64,
) -> Result<(), OrmError> {
    require_provider_exhaustion(stats)?;
    if stats.processed_items >= max_items {
        return Err(solution_scan_limit_error(max_items));
    }
    Ok(())
}

fn solution_scan_limit_error(max_items: u64) -> OrmError {
    MatchError::new(
        MatchErrorCategory::ResourceLimit,
        "solution_scan_limit",
        "provider solution ceiling was reached before result completeness was proven",
    )
    .at(MatchErrorPathSegment::ProviderEvidence)
    .with_detail("limit", max_items)
    .into()
}

fn require_provider_exhaustion(
    stats: crate::session::backend::BoundedAnswerStats,
) -> Result<(), OrmError> {
    if stats.stopped_early {
        return Err(decode_error(
            "provider_stream_not_exhausted",
            "provider stopped before the bounded typed statement reached its terminal frame",
        ));
    }
    Ok(())
}

fn exactly_one_proof_byte_limit(
    registry: &DescriptorRegistry,
    selection: &TypedFetchRows,
) -> Result<u64, OrmError> {
    let projected_bindings = selection.projection.len();
    if projected_bindings == 0 || projected_bindings > super::limits::MAX_BINDINGS {
        return Err(decode_error(
            "tuple_proof_shape_limit",
            "distinct-tuple proof projection exceeds the validated binding ceiling",
        ));
    }
    let bindings = u64::try_from(projected_bindings).map_err(|_| {
        resource_error(
            "answer_byte_counter_overflow",
            "distinct-tuple proof binding count exceeds the counter range",
        )
    })?;
    let projected = selection
        .projection
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if projected.len() != projected_bindings {
        return Err(decode_error(
            "tuple_proof_shape_limit",
            "distinct-tuple proof projection contains duplicate bindings",
        ));
    }
    let mut targets = BTreeMap::new();
    for target in &selection.targets {
        if projected.contains(&target.binding) && targets.insert(target.binding, target).is_some() {
            return Err(decode_error(
                "tuple_proof_shape_limit",
                "distinct-tuple proof contains duplicate projected targets",
            ));
        }
    }
    if targets.len() != projected_bindings {
        return Err(decode_error(
            "tuple_proof_shape_limit",
            "distinct-tuple proof projection is missing a typed target",
        ));
    }

    // Exact targets can only return their declared label. Subtype-inclusive
    // targets can return registered descendants of the same thing kind. Do
    // not let unrelated registry entries or unprojected targets inflate this
    // internal proof allowance.
    let snapshot = if targets.values().any(|target| !target.exact) {
        registry.snapshot()
    } else {
        Vec::new()
    };
    let mut entity_children = BTreeMap::<&str, Vec<&str>>::new();
    let mut relation_children = BTreeMap::<&str, Vec<&str>>::new();
    for descriptor in &snapshot {
        match descriptor {
            TypeDescriptor::Entity(descriptor) => {
                if let Some(parent) = descriptor.parent_type.as_deref() {
                    entity_children
                        .entry(parent)
                        .or_default()
                        .push(&descriptor.type_name);
                }
            }
            TypeDescriptor::Relation(descriptor) => {
                if let Some(parent) = descriptor.parent_type.as_deref() {
                    relation_children
                        .entry(parent)
                        .or_default()
                        .push(&descriptor.type_name);
                }
            }
        }
    }
    let mut entity_maxima = BTreeMap::<&str, usize>::new();
    let mut relation_maxima = BTreeMap::<&str, usize>::new();
    let max_label_bytes = targets
        .values()
        .map(|target| {
            if target.exact {
                return target.type_name.len();
            }
            match target.kind {
                TypedThingKind::Entity => {
                    *entity_maxima.entry(&target.type_name).or_insert_with(|| {
                        max_target_closure_label_bytes(&entity_children, &target.type_name)
                    })
                }
                TypedThingKind::Relation => {
                    *relation_maxima.entry(&target.type_name).or_insert_with(|| {
                        max_target_closure_label_bytes(&relation_children, &target.type_name)
                    })
                }
            }
        })
        .max()
        .unwrap_or(0);
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
    let derived_limit = TUPLE_PROOF_ROW_ENVELOPE_BYTES
        .checked_add(bindings.checked_mul(binding_bytes).ok_or_else(|| {
            resource_error(
                "answer_byte_counter_overflow",
                "distinct-tuple proof byte ceiling overflowed",
            )
        })?)
        .and_then(|row| row.checked_mul(TUPLE_PROOF_ROWS))
        .ok_or_else(|| {
            resource_error(
                "answer_byte_counter_overflow",
                "distinct-tuple proof byte ceiling overflowed",
            )
        })?;
    Ok(derived_limit.min(MAX_RESPONSE_BYTES))
}

fn max_target_closure_label_bytes<'a>(
    children: &BTreeMap<&'a str, Vec<&'a str>>,
    root: &'a str,
) -> usize {
    let mut maximum = root.len();
    let mut pending = vec![root];
    let mut visited = BTreeSet::new();
    while let Some(parent) = pending.pop() {
        if !visited.insert(parent) {
            continue;
        }
        if let Some(descendants) = children.get(parent) {
            for descendant in descendants {
                maximum = maximum.max(descendant.len());
                pending.push(descendant);
            }
        }
    }
    maximum
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
    Page,
}

/// Preserve the first semantic/decode failure while making a bounded attempt
/// to consume the current provider statement through its terminal frame.
///
/// The TypeDB driver uses resumable answer streams; returning an error from a
/// nested consumer would abandon the stream just like `AnswerControl::Stop`.
/// Later answers are not decoded and both the provider reader and this adapter
/// enforce finite item/byte ceilings. If the small suffix allowance is spent,
/// `Stop` terminates this statement. Owned execution then closes its private
/// transaction; borrowed execution never closes its caller-owned context.
struct DrainingConsumer<'a> {
    inner: &'a mut dyn AnswerConsumer,
    limits: StatementResourceLimits,
    processed_items: u64,
    response_bytes: u64,
    drained_items: u64,
    drained_bytes: u64,
    first_error: Option<OrmError>,
    consumer_stopped: bool,
}

impl<'a> DrainingConsumer<'a> {
    fn new(inner: &'a mut dyn AnswerConsumer, limits: StatementResourceLimits) -> Self {
        Self {
            inner,
            limits,
            processed_items: 0,
            response_bytes: 0,
            drained_items: 0,
            drained_bytes: 0,
            first_error: None,
            consumer_stopped: false,
        }
    }

    fn complete(
        mut self,
        provider: Result<crate::session::backend::BoundedAnswerStats, OrmError>,
        budget: &mut ExecutionBudget,
    ) -> Result<crate::session::backend::BoundedAnswerStats, OrmError> {
        // Sticky consumer failures bypass the provider future's map_err seam;
        // route them through the same released redaction boundary before they
        // can reach a caller or the dual-failure cleanup warning.
        let first_error = self.first_error.take().map(provider_statement_error);
        match (first_error, provider) {
            (Some(error), Ok(stats)) => {
                let stats = self.reconcile_stats(stats);
                if self.limits.charge_to_caller {
                    budget.charge(stats);
                }
                Err(error)
            }
            (None, Ok(stats)) => {
                let stats = self.reconcile_stats(stats);
                if self.limits.charge_to_caller {
                    budget.charge(stats);
                }
                Ok(stats)
            }
            (Some(error), Err(_)) => Err(error),
            (None, Err(error)) => Err(error),
        }
    }

    fn reconcile_stats(
        &self,
        mut provider: crate::session::backend::BoundedAnswerStats,
    ) -> crate::session::backend::BoundedAnswerStats {
        // Third-party backends are an open extension seam. Never let a custom
        // provider weaken the caller's remaining budget by reporting fewer
        // items or bytes than this local consumer actually observed.
        provider.processed_items = provider.processed_items.max(self.processed_items);
        provider.response_bytes = provider.response_bytes.max(self.response_bytes);
        provider.stopped_early |= self.consumer_stopped;
        provider
    }

    fn reject(&mut self, error: OrmError) -> Result<AnswerControl, OrmError> {
        self.first_error = Some(error);
        Ok(if self.has_drain_capacity() {
            AnswerControl::Continue
        } else {
            AnswerControl::Stop
        })
    }

    fn has_drain_capacity(&self) -> bool {
        self.processed_items < self.limits.max_items
            && self.response_bytes < self.limits.max_bytes
            && self.drained_items < MAX_ERROR_DRAIN_ITEMS
            && self.drained_bytes < MAX_ERROR_DRAIN_BYTES
    }

    fn accept_suffix(&mut self, item: AnswerItem) -> AnswerControl {
        let Ok(item_bytes) = item.encoded_bytes() else {
            return AnswerControl::Stop;
        };
        let Some(next_items) = self.processed_items.checked_add(1) else {
            return AnswerControl::Stop;
        };
        let Some(next_bytes) = self.response_bytes.checked_add(item_bytes) else {
            return AnswerControl::Stop;
        };
        let Some(next_drained_items) = self.drained_items.checked_add(1) else {
            return AnswerControl::Stop;
        };
        let Some(next_drained_bytes) = self.drained_bytes.checked_add(item_bytes) else {
            return AnswerControl::Stop;
        };
        if next_items > self.limits.max_items
            || next_bytes > self.limits.max_bytes
            || next_drained_items > MAX_ERROR_DRAIN_ITEMS
            || next_drained_bytes > MAX_ERROR_DRAIN_BYTES
        {
            return AnswerControl::Stop;
        }
        self.processed_items = next_items;
        self.response_bytes = next_bytes;
        self.drained_items = next_drained_items;
        self.drained_bytes = next_drained_bytes;
        if self.has_drain_capacity() {
            AnswerControl::Continue
        } else {
            AnswerControl::Stop
        }
    }
}

impl AnswerConsumer for DrainingConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        if self.first_error.is_some() {
            return Ok(self.accept_suffix(item));
        }

        let next_items = match self.processed_items.checked_add(1) {
            Some(next_items) => next_items,
            None => {
                return self.reject(resource_error(
                    "processed_item_counter_overflow",
                    "processed provider item counter overflowed",
                ));
            }
        };
        if next_items > self.limits.max_items {
            return self.reject(resource_error(
                "processed_item_limit",
                "provider answer exceeded the processed-item ceiling",
            ));
        }
        let item_bytes = match item.encoded_bytes() {
            Ok(item_bytes) => item_bytes,
            Err(error) => return self.reject(error),
        };
        let next_bytes = match self.response_bytes.checked_add(item_bytes) {
            Some(next_bytes) => next_bytes,
            None => {
                return self.reject(resource_error(
                    "answer_byte_counter_overflow",
                    "provider answer byte counter overflowed",
                ));
            }
        };
        if next_bytes > self.limits.max_bytes {
            return self.reject(resource_error(
                "response_byte_limit",
                "provider answer exceeded the response-byte ceiling",
            ));
        }
        self.processed_items = next_items;
        self.response_bytes = next_bytes;
        match self.inner.accept(item) {
            Ok(AnswerControl::Continue) => Ok(AnswerControl::Continue),
            Ok(AnswerControl::Stop) => {
                self.consumer_stopped = true;
                Ok(AnswerControl::Stop)
            }
            Err(error) => self.reject(error),
        }
    }
}

struct TupleConsumer {
    selected: Vec<BindingId>,
    expected: BTreeSet<BindingId>,
    seen: BTreeSet<Vec<String>>,
}

impl TupleConsumer {
    fn new(selected: &[u16]) -> Self {
        let selected = selected
            .iter()
            .copied()
            .map(BindingId::new)
            .collect::<Vec<_>>();
        Self {
            expected: selected.iter().copied().collect(),
            selected,
            seen: BTreeSet::new(),
        }
    }

    fn len(&self) -> usize {
        self.seen.len()
    }
}

impl AnswerConsumer for TupleConsumer {
    fn accept(&mut self, item: AnswerItem) -> Result<AnswerControl, OrmError> {
        let AnswerItem::Row(value) = item else {
            return Err(decode_error(
                "tuple_answer_kind",
                "distinct-tuple statement returned a document instead of a row",
            ));
        };
        let wire: SolutionWire = serde_json::from_value(value).map_err(|error| {
            decode_error_owned(
                "malformed_tuple_row",
                format!("invalid distinct-tuple row: {error}"),
            )
        })?;
        if !wire.satisfied_role_edges.is_empty() {
            return Err(decode_error(
                "malformed_tuple_row",
                "distinct-tuple row must not claim role-edge evidence",
            ));
        }
        let mut bindings = BTreeMap::new();
        for assignment in wire.bindings {
            let binding = BindingId::new(assignment.binding);
            if !self.expected.contains(&binding) {
                return Err(decode_error(
                    "unknown_provider_binding",
                    "distinct-tuple row contains an unselected binding",
                ));
            }
            validate_provider_iid(&assignment.concept_id)?;
            if bindings.insert(binding, assignment.concept_id).is_some() {
                return Err(decode_error(
                    "duplicate_provider_binding",
                    "distinct-tuple row assigns one binding more than once",
                ));
            }
        }
        if bindings.len() != self.expected.len() {
            return Err(decode_error(
                "missing_provider_binding",
                "provider solution omits a positive binding",
            ));
        }
        self.seen.insert(
            self.selected
                .iter()
                .map(|binding| bindings[binding].clone())
                .collect(),
        );
        // The TypeQL statement owns `limit 2`; consume its terminal frame.
        Ok(AnswerControl::Continue)
    }
}

struct RootConsumer {
    root: BindingId,
    retain_limit: Option<u64>,
    seen: BTreeSet<String>,
    roots: Vec<String>,
}

impl RootConsumer {
    fn new(root: BindingId, retain_limit: Option<u64>) -> Self {
        Self {
            root,
            retain_limit,
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
        if self
            .retain_limit
            .is_none_or(|limit| (self.roots.len() as u64) < limit)
            && self.seen.insert(assignment.concept_id.clone())
        {
            self.roots.push(assignment.concept_id.clone());
        }
        // Every root-selection statement carries its semantic limit in TypeQL.
        // Continue through the provider's terminal stream frame instead of
        // abandoning a resumable stream after the final desired identity.
        Ok(AnswerControl::Continue)
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
    stop_at_prefix: bool,
    seen: BTreeSet<Vec<String>>,
    solutions: Vec<UnhydratedSolution>,
}

impl SolutionConsumer {
    fn new(validated: &ValidatedMatchRequest, stop_at_prefix: bool) -> Result<Self, OrmError> {
        let MatchOperation::FetchRows { output, .. } = &validated.request().operation else {
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
        Ok(Self {
            expected: validated
                .request()
                .plan
                .bindings
                .iter()
                .map(|binding| binding.id)
                .collect(),
            selected,
            stop_after: released_solution_prefix(validated)?,
            stop_at_prefix,
            seen: BTreeSet::new(),
            solutions: Vec::new(),
        })
    }

    fn finish(self) -> Vec<UnhydratedSolution> {
        self.solutions
    }

    fn reached_prefix(&self) -> bool {
        self.solutions.len() as u64 >= self.stop_after
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
        if (self.solutions.len() as u64) < self.stop_after && self.seen.insert(identity) {
            self.solutions.push(UnhydratedSolution {
                bindings,
                satisfied_role_edges: wire
                    .satisfied_role_edges
                    .into_iter()
                    .map(RoleEdgeId::new)
                    .collect(),
            });
        }
        if self.stop_at_prefix && self.solutions.len() as u64 >= self.stop_after {
            return Ok(AnswerControl::Stop);
        }
        // The provider statement is itself capped. Continue until its terminal
        // frame so a successful bounded result never abandons a resumable
        // TypeDB stream; retain only the semantic prefix needed by the caller.
        Ok(AnswerControl::Continue)
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
        let collections = match &validated.request().operation {
            MatchOperation::PageBy { output, .. } => output_slots(output)
                .filter_map(|slot| match slot {
                    FetchSlot::One { .. } => None,
                    FetchSlot::Collect {
                        binding, distinct, ..
                    } => Some((*binding, *distinct)),
                })
                .collect(),
            MatchOperation::ReduceBy { .. }
            | MatchOperation::ReduceByField { .. }
            | MatchOperation::ReduceByFields { .. } => Vec::new(),
            _ => {
                return Err(decode_error(
                    "page_operation_mismatch",
                    "re-match consumption requires a PageBy or reduction operation",
                ));
            }
        };
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
                AttributeValue::from_json(value, field.value_type.as_str())
                    .ok_or_else(|| {
                        decode_error(
                            "hydrated_attribute_value_type",
                            "TypeDB wildcard attribute value has the wrong value type",
                        )
                    })
                    .and_then(|value| {
                        canonicalize_provider_attribute_value(value).map_err(OrmError::Match)
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
    descriptors: &[crate::_descriptor::RoleDescriptor],
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
                    AttributeValue::from_json(value, &attribute.value_type)
                        .ok_or_else(|| {
                            decode_error(
                                "hydrated_attribute_value_type",
                                "hydrated attribute value does not match its value type",
                            )
                        })
                        .and_then(|value| {
                            canonicalize_provider_attribute_value(value).map_err(OrmError::Match)
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
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::Reachable { .. } => {}
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

fn hydration_answer_limit(batch: &TypedHydrateThings) -> Result<u64, OrmError> {
    batch.targets.iter().try_fold(0_u64, |total, target| {
        let identities = u64::try_from(target.concept_ids.len()).map_err(|_| {
            resource_error(
                "processed_item_counter_overflow",
                "typed hydration identity count exceeds the provider counter range",
            )
        })?;
        total.checked_add(identities).ok_or_else(|| {
            resource_error(
                "processed_item_counter_overflow",
                "typed hydration identity counter overflowed",
            )
        })
    })
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

fn adapter_diagnostic(diagnostic: Diagnostic) -> OrmError {
    let category = match diagnostic.category() {
        DiagnosticCategory::InvalidContract => MatchErrorCategory::InvalidPlan,
        DiagnosticCategory::UnsupportedCapability => MatchErrorCategory::UnsupportedCapability,
        DiagnosticCategory::ResourceLimit => MatchErrorCategory::ResourceLimit,
        DiagnosticCategory::Integrity => MatchErrorCategory::ResultDecode,
    };
    MatchError::new(category, diagnostic.code().as_str(), diagnostic.message())
        .at(MatchErrorPathSegment::Operation)
        .into()
}

fn compatibility_execution_error(error: QueryV2ExecutionError) -> OrmError {
    match error {
        QueryV2ExecutionError::Provider(error) => provider_statement_error(error),
        QueryV2ExecutionError::Validation(diagnostic)
            if diagnostic.code().as_str() == "query_v2_model_exactly_one" =>
        {
            let actual = diagnostic
                .details()
                .get("actual")
                .and_then(|value| match value {
                    DiagnosticDetailValue::Long(value) => usize::try_from(*value).ok(),
                    DiagnosticDetailValue::Text(_)
                    | DiagnosticDetailValue::Boolean(_)
                    | DiagnosticDetailValue::TextList(_) => None,
                })
                .unwrap_or(usize::MAX);
            exactly_one_cardinality_error(actual)
                .unwrap_or_else(|| {
                    MatchError::new(
                        MatchErrorCategory::ResultDecode,
                        "result_operation_mismatch",
                        "adapted exactly-one result lost its cardinality proof",
                    )
                    .at(MatchErrorPathSegment::Result)
                })
                .into()
        }
        QueryV2ExecutionError::Validation(diagnostic) => {
            let (category, code, mapped_message) = released_execution_diagnostic(&diagnostic);
            let path = released_execution_path(&diagnostic);
            let message = match (category, code, path.segments()) {
                (
                    MatchErrorCategory::ResultDecode,
                    _,
                    [MatchErrorPathSegment::ProviderEvidence],
                ) => "provider evidence failed canonical typed match decoding",
                (
                    MatchErrorCategory::ResourceLimit,
                    "processed_item_counter_overflow"
                    | "processed_item_limit"
                    | "answer_byte_counter_overflow"
                    | "response_byte_limit",
                    _,
                ) => "provider resource limits prevented complete typed match evidence",
                _ => mapped_message,
            };
            MatchError::new(category, code, message)
                .with_path(path)
                .into()
        }
    }
}

fn released_execution_path(diagnostic: &Diagnostic) -> MatchErrorPath {
    let segments = match diagnostic.code().as_str() {
        "query_v2_model_regex" => vec![MatchErrorPathSegment::Predicate],
        "query_v2_model_rows_collection" => vec![MatchErrorPathSegment::Operation],
        "query_v2_model_contract_missing"
        | "query_v2_model_provider_plan_missing"
        | "query_v2_model_provider_plan_mismatch"
        | "query_v2_model_tuple_plan"
        | "query_v2_model_page_total_plan" => vec![MatchErrorPathSegment::Result],
        "query_v2_model_predicate_equal_type"
        | "query_v2_model_predicate_order_type"
        | "query_v2_model_predicate_string_type" => vec![MatchErrorPathSegment::Result],
        "query_v2_model_order_non_scalar"
        | "query_v2_model_order_missing"
        | "query_v2_model_order_value_type" => {
            let owner = diagnostic.details().get("field_owner").and_then(|value| {
                if let DiagnosticDetailValue::Text(value) = value {
                    Some(value.clone())
                } else {
                    None
                }
            });
            let name = diagnostic.details().get("field_name").and_then(|value| {
                if let DiagnosticDetailValue::Text(value) = value {
                    Some(value.clone())
                } else {
                    None
                }
            });
            match (owner, name) {
                (Some(owner), Some(name)) => vec![
                    MatchErrorPathSegment::Result,
                    MatchErrorPathSegment::Field(FieldId::new(DescriptorId::new(owner), name)),
                ],
                _ => vec![MatchErrorPathSegment::Result],
            }
        }
        _ => vec![MatchErrorPathSegment::ProviderEvidence],
    };
    MatchErrorPath::from_segments(segments)
}

fn released_execution_diagnostic(
    diagnostic: &Diagnostic,
) -> (MatchErrorCategory, &'static str, &'static str) {
    match diagnostic.code().as_str() {
        "statement_count_limit" => (
            MatchErrorCategory::ResourceLimit,
            "statement_count_limit",
            "match execution exceeded its statement ceiling",
        ),
        "provider_cancelled" => (
            MatchErrorCategory::ResourceLimit,
            "provider_cancelled",
            "provider answer processing was cancelled",
        ),
        "transaction_deadline_exceeded" => (
            MatchErrorCategory::ResourceLimit,
            "transaction_deadline_exceeded",
            "provider transaction deadline expired",
        ),
        "processed_item_counter_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_counter_overflow",
            "processed provider item counter overflowed",
        ),
        "processed_item_limit" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        "answer_byte_counter_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "answer_byte_counter_overflow",
            "provider answer byte counter overflowed",
        ),
        "response_byte_limit" => (
            MatchErrorCategory::ResourceLimit,
            "response_byte_limit",
            "provider answer exceeded the response-byte ceiling",
        ),
        "query_v2_model_item_limit" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        "query_v2_model_byte_limit" => (
            MatchErrorCategory::ResourceLimit,
            "response_byte_limit",
            "provider answer exceeded the response-byte ceiling",
        ),
        "query_v2_model_collection_limit" => (
            MatchErrorCategory::ResourceLimit,
            "collected_concept_limit",
            "provider result exceeded the collected-concept ceiling",
        ),
        "query_v2_model_graph_limit" | "query_v2_model_hydration_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_thing_limit",
            "provider result exceeded the hydrated-thing ceiling",
        ),
        "query_v2_model_attribute_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_attribute_value_limit",
            "provider result exceeded the hydrated attribute-value ceiling",
        ),
        "query_v2_model_role_player_limit" => (
            MatchErrorCategory::ResourceLimit,
            "hydrated_thing_limit",
            "provider result exceeded the hydrated-thing ceiling",
        ),
        "query_v2_model_solution_limit" => (
            MatchErrorCategory::ResourceLimit,
            "solution_scan_limit",
            "provider solution ceiling was reached before result completeness was proven",
        ),
        "query_v2_model_count_overflow" => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_counter_overflow",
            "processed provider item counter overflowed",
        ),
        "query_v2_model_contract_missing"
        | "query_v2_model_provider_plan_missing"
        | "query_v2_model_provider_plan_mismatch"
        | "query_v2_model_tuple_plan" => (
            MatchErrorCategory::ResultDecode,
            "result_operation_mismatch",
            "adapted V2 result variant does not match the validated released operation",
        ),
        "query_v2_model_rows_collection" => (
            MatchErrorCategory::InvalidPlan,
            "unsupported_collection_slot",
            "selected-row execution does not support collected output slots",
        ),
        "query_v2_model_page_total_plan" => (
            MatchErrorCategory::ResultDecode,
            "page_operation_mismatch",
            "typed page plan does not belong to a PageBy operation",
        ),
        "query_v2_model_provider_not_exhausted" => (
            MatchErrorCategory::ResultDecode,
            "provider_stream_not_exhausted",
            "provider stopped before the bounded typed statement reached its terminal frame",
        ),
        "query_v2_model_solution_kind" => (
            MatchErrorCategory::ResultDecode,
            "solution_answer_kind",
            "selected solution scan returned a document instead of a row",
        ),
        "query_v2_model_solution_malformed"
        | "query_v2_model_predicate_evidence"
        | "query_v2_model_role_edge_evidence" => (
            MatchErrorCategory::ResultDecode,
            "malformed_solution_row",
            "provider solution row does not match the typed evidence shape",
        ),
        "query_v2_model_solution_binding" => (
            MatchErrorCategory::ResultDecode,
            "missing_provider_binding",
            "provider solution omitted a required positive binding",
        ),
        "query_v2_model_solution_unknown_binding" => (
            MatchErrorCategory::ResultDecode,
            "unknown_provider_binding",
            "provider solution contains an unknown binding",
        ),
        "query_v2_model_solution_duplicate_binding" => (
            MatchErrorCategory::ResultDecode,
            "duplicate_provider_binding",
            "provider solution assigns one binding more than once",
        ),
        "query_v2_model_solution_malformed_iid" => (
            MatchErrorCategory::ResultDecode,
            "malformed_provider_concept_id",
            "provider solution contains a malformed provider IID",
        ),
        "query_v2_model_unstable_provider_order" => (
            MatchErrorCategory::ResultDecode,
            "unstable_provider_order",
            "provider solutions violate the validator-derived stable order",
        ),
        "query_v2_model_order_non_scalar" => (
            MatchErrorCategory::ResultDecode,
            "non_scalar_order_evidence",
            "provider returned multiple values for a validated scalar order field",
        ),
        "query_v2_model_order_missing" => (
            MatchErrorCategory::ResultDecode,
            "missing_order_value",
            "provider omitted a value required by stable ordering",
        ),
        "query_v2_model_order_value_type" => (
            MatchErrorCategory::ResultDecode,
            "order_value_type_mismatch",
            "provider order values are not mutually comparable",
        ),
        "query_v2_model_predicate_equal_type" => (
            MatchErrorCategory::ResultDecode,
            "predicate_value_type_mismatch",
            "provider predicate values do not have the same validated type",
        ),
        "query_v2_model_predicate_order_type" => (
            MatchErrorCategory::ResultDecode,
            "predicate_value_type_mismatch",
            "provider predicate values are not order-compatible",
        ),
        "query_v2_model_predicate_string_type" => (
            MatchErrorCategory::ResultDecode,
            "predicate_value_type_mismatch",
            "provider predicate values do not support the validated string operator",
        ),
        "query_v2_model_regex" => (
            MatchErrorCategory::InvalidPlan,
            "invalid_regex_pattern",
            "validated request contains an invalid regular expression",
        ),
        "query_v2_model_tuple_evidence" => (
            MatchErrorCategory::ResultDecode,
            "malformed_tuple_row",
            "provider tuple proof row does not match the typed evidence shape",
        ),
        "query_v2_model_root_kind" => (
            MatchErrorCategory::ResultDecode,
            "root_answer_kind",
            "distinct-root scan returned a document instead of a row",
        ),
        "query_v2_model_root_binding" => (
            MatchErrorCategory::ResultDecode,
            "root_binding_mismatch",
            "distinct-root row contains the wrong binding",
        ),
        "query_v2_model_root_malformed" => (
            MatchErrorCategory::ResultDecode,
            "malformed_root_row",
            "distinct-root row does not match the typed evidence shape",
        ),
        "query_v2_model_page_kind" => (
            MatchErrorCategory::ResultDecode,
            "page_rematch_answer_kind",
            "page re-match returned a row instead of a hydrated document",
        ),
        "query_v2_model_page_document" => (
            MatchErrorCategory::ResultDecode,
            "malformed_page_rematch_document",
            "page re-match document does not match the typed evidence shape",
        ),
        "query_v2_model_page_binding" => (
            MatchErrorCategory::ResultDecode,
            "malformed_page_rematch_binding",
            "page re-match omitted a required positive binding",
        ),
        "query_v2_model_page_root_set" => (
            MatchErrorCategory::ResultDecode,
            "selected_root_set_mismatch",
            "page re-match root set does not exactly equal root selection",
        ),
        "query_v2_model_page_unexpected_root" => (
            MatchErrorCategory::ResultDecode,
            "unexpected_hydrated_root",
            "page re-match returned a root outside the selected root set",
        ),
        "query_v2_model_page_total_length" => (
            MatchErrorCategory::ResultDecode,
            "page_total_length_mismatch",
            "provider page length is inconsistent with its same-snapshot total and window",
        ),
        "query_v2_model_hydration_missing" => (
            MatchErrorCategory::ResultDecode,
            "missing_hydrated_concept",
            "batched hydration omitted a requested binding/IID pair",
        ),
        "query_v2_model_hydration_unexpected" => (
            MatchErrorCategory::ResultDecode,
            "unexpected_hydrated_concept",
            "batched hydration returned an unrequested binding/IID pair",
        ),
        "query_v2_model_hydration_value_type" | "query_v2_model_hydration_value_order" => (
            MatchErrorCategory::ResultDecode,
            "hydrated_attribute_value_type",
            "TypeDB wildcard attribute value has the wrong value type",
        ),
        "query_v2_model_hydration_attributes" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydrated_attributes",
            "TypeDB wildcard hydration attributes are malformed",
        ),
        "query_v2_model_hydration_roles" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydrated_roles",
            "TypeDB nested role hydration is malformed",
        ),
        "query_v2_model_hydration_role_player" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydrated_role_player",
            "TypeDB nested hydration role player is malformed",
        ),
        "query_v2_model_hydration_document"
        | "query_v2_model_hydration_binding"
        | "query_v2_model_hydration_iid"
        | "query_v2_model_hydration_descriptor"
        | "query_v2_model_hydration_kind" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydration_document",
            "hydration document does not match the typed evidence shape",
        ),
        "query_v2_model_hydration_conflict" => (
            MatchErrorCategory::ResultDecode,
            "duplicate_hydrated_concept",
            "batched hydration returned contradictory evidence for one provider concept",
        ),
        "query_v2_model_hydration_authority" | "query_v2_model_hydration_state" => (
            MatchErrorCategory::ResultDecode,
            "malformed_hydration_document",
            "hydration document does not match the typed evidence shape",
        ),
        _ if diagnostic.code().as_str().starts_with("query_v2_model_") => (
            MatchErrorCategory::ResultDecode,
            "unmapped_model_execution_diagnostic",
            "model execution emitted a diagnostic without a released compatibility mapping",
        ),
        _ if diagnostic.category() == DiagnosticCategory::ResourceLimit => (
            MatchErrorCategory::ResourceLimit,
            "processed_item_limit",
            "provider answer exceeded the processed-item ceiling",
        ),
        _ => (
            MatchErrorCategory::ResultDecode,
            "malformed_solution_row",
            "provider evidence does not match the validated request",
        ),
    }
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
        "malformed_tuple_row",
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
        "tuple_answer_kind",
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
    if type_bridge_contract::id::is_canonical_thing_iid(value) {
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
    use crate::_attribute::ValueType;
    use crate::_descriptor::{
        EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
    };
    use crate::_entity::Annotation;
    use crate::MatchErrorPath;
    use crate::match_request::ids::BoundFieldId;
    use crate::match_request::model::{
        BindingPair, MatchBinding, MatchExpr, MatchMode, MatchOrder, MatchPlan, MatchRequest,
        MissingOrder, ReduceTerm, SortDirection, Window,
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
    fn provider_scalar_spellings_are_canonicalized_at_the_hydration_boundary() {
        assert_eq!(
            canonicalize_provider_attribute_value(AttributeValue::DateTime(
                "2026-07-28T03:55:00.000000000".into(),
            ))
            .unwrap(),
            AttributeValue::DateTime("2026-07-28T03:55:00".into())
        );
        assert_eq!(
            canonicalize_provider_attribute_value(AttributeValue::Decimal("00123.4500dec".into(),))
                .unwrap(),
            AttributeValue::Decimal("123.45".into())
        );
        assert_eq!(
            canonicalize_provider_attribute_value(AttributeValue::DateTimeTZ(
                "2026-08-03T03:55:00.000000000+00:00".into(),
            ))
            .unwrap(),
            AttributeValue::DateTimeTZ("2026-08-03T03:55:00Z".into())
        );
        for duration in ["PT1H", "P1DT30M"] {
            assert_eq!(
                canonicalize_provider_attribute_value(AttributeValue::Duration(duration.into()))
                    .unwrap(),
                AttributeValue::Duration(duration.into())
            );
        }

        let descriptor = TypeDescriptorRef::Entity(Arc::new(EntityDescriptor {
            type_name: "record".into(),
            is_abstract: false,
            parent_type: None,
            owned_attributes: vec![OwnedAttributeDescriptor {
                field_name: "val_datetime".into(),
                attr_name: "val-datetime".into(),
                value_type: ValueType::DateTime,
                annotations: vec![Annotation::Card(1, Some(1))],
                is_optional: false,
                is_ordered: false,
                doc: None,
                meta: Default::default(),
            }],
            doc: None,
            meta: Default::default(),
        }));
        let provider = serde_json::json!({
            "val-datetime": "2026-07-28T03:55:00.000000000"
        });
        let decoded = decode_wildcard_attributes(
            &DescriptorId::new("entity:record"),
            &descriptor,
            Some(&provider),
        )
        .unwrap();
        assert_eq!(
            decoded[0].values(),
            &[AttributeValue::DateTime("2026-07-28T03:55:00".into())]
        );
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
    async fn compatible_execution_preserves_released_nullable_order_error_before_provider_io() {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    key("name"),
                    OwnedAttributeDescriptor {
                        field_name: "ranking".into(),
                        attr_name: "person-ranking".into(),
                        value_type: ValueType::Long,
                        annotations: vec![Annotation::Card(0, Some(1))],
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
        let nullable_field = FieldId::new(registry.descriptor_id("person").unwrap(), "ranking");
        let mut request = person_request(&registry, RowCardinality::BoundedMany, 2);
        let MatchOperation::FetchRows { order, .. } = &mut request.operation else {
            unreachable!()
        };
        *order = vec![MatchOrder {
            field: BoundFieldId::new(BindingId::new(0), nullable_field.clone()),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        }];
        let validated = validate_match_request(&registry, request).unwrap();
        let (database, events) = database(CapabilitySet::all(), Vec::new(), Vec::new());

        let assert_released_error = |error: &OrmError| {
            let error = match_error(error);
            assert_eq!(error.category(), MatchErrorCategory::UnsupportedCapability);
            assert_eq!(error.code().as_str(), "nullable_order_field_unsupported");
            assert_eq!(
                error.message(),
                "the selected provider cannot window by a nullable order field without filtering missing roots"
            );
            assert_eq!(
                error.path().segments(),
                &[MatchErrorPathSegment::Field(nullable_field.clone())]
            );
            assert!(error.details().is_empty());
        };

        let owned = database
            .execute_match(&registry, &validated)
            .await
            .expect_err("owned compatibility execution must retain V1 lowering rejection");
        assert_released_error(&owned);
        assert_eq!(events.lock().unwrap().opens, 0);

        let context = database.transaction_context(TxType::Read).await.unwrap();
        let borrowed = context
            .execute_match(&registry, &validated)
            .await
            .expect_err("borrowed compatibility execution must retain V1 lowering rejection");
        assert_released_error(&borrowed);
        {
            let events = events.lock().unwrap();
            assert_eq!(events.opens, 1);
            assert!(events.solution_statements.is_empty());
            assert!(events.hydration_statements.is_empty());
        }
        context.close().await.unwrap();
    }

    #[test]
    fn exactly_one_proof_bytes_are_derived_from_bounded_identity_shape() {
        fn selection(type_name: &str, projected_bindings: usize) -> TypedFetchRows {
            let bindings = (0..projected_bindings)
                .map(|binding| u16::try_from(binding).unwrap())
                .collect::<Vec<_>>();
            TypedFetchRows {
                targets: bindings
                    .iter()
                    .copied()
                    .map(|binding| type_bridge_core_lib::ast::TypedMatchTarget {
                        binding,
                        kind: TypedThingKind::Entity,
                        type_name: type_name.to_owned(),
                        exact: true,
                    })
                    .collect(),
                fields: Vec::new(),
                predicate: None,
                projection: bindings,
                distinct: true,
                order: Vec::new(),
                offset: 0,
                limit: TUPLE_PROOF_ROWS,
            }
        }

        let registry = person_registry();
        let one = selection("person", 1);
        let maximum_shape = selection("person", super::super::limits::MAX_BINDINGS);
        let one_binding = exactly_one_proof_byte_limit(&registry, &one).unwrap();
        let maximum = exactly_one_proof_byte_limit(&registry, &maximum_shape).unwrap();
        assert!(one_binding < 1536);
        assert!(maximum < 400 * 1024);
        assert!(
            exactly_one_proof_byte_limit(
                &registry,
                &selection("person", super::super::limits::MAX_BINDINGS + 1),
            )
            .is_err()
        );

        let unrelated_registry = person_registry();
        let unrelated_label = format!("unrelated_{}", "x".repeat(4096));
        unrelated_registry
            .register_entity(EntityDescriptor {
                type_name: unrelated_label.clone(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: Vec::new(),
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        assert_eq!(
            exactly_one_proof_byte_limit(&unrelated_registry, &one).unwrap(),
            one_binding
        );
        let mut with_unprojected_target = one.clone();
        with_unprojected_target
            .targets
            .push(type_bridge_core_lib::ast::TypedMatchTarget {
                binding: u16::MAX,
                kind: TypedThingKind::Entity,
                type_name: unrelated_label,
                exact: true,
            });
        assert_eq!(
            exactly_one_proof_byte_limit(&registry, &with_unprojected_target).unwrap(),
            one_binding
        );

        let closure_registry = person_registry();
        closure_registry
            .register_entity(EntityDescriptor {
                type_name: format!("person_{}", "x".repeat(140 * 1024)),
                is_abstract: false,
                parent_type: Some("person".into()),
                owned_attributes: Vec::new(),
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let mut subtype_shape = maximum_shape.clone();
        for target in &mut subtype_shape.targets {
            target.exact = false;
        }
        assert_eq!(
            exactly_one_proof_byte_limit(&closure_registry, &subtype_shape).unwrap(),
            MAX_RESPONSE_BYTES
        );

        let iid = format!("0x{}", "f".repeat(MAX_THING_IID_HEX_DIGITS));
        let assignments = (0..super::super::limits::MAX_BINDINGS)
            .map(|_| serde_json::json!({"binding": u16::MAX, "concept_id": iid.clone()}))
            .collect::<Vec<_>>();
        let row = AnswerItem::Row(serde_json::json!({
            "bindings": assignments,
            "satisfied_role_edges": [],
        }));
        let two_maximum_rows = row.encoded_bytes().unwrap().checked_mul(2).unwrap();
        assert!(two_maximum_rows <= maximum);

        let concept = serde_json::json!({
            "category": "entity",
            "label": "person",
            "iid": iid.clone(),
        });
        let raw_row = AnswerItem::Row(serde_json::Value::Object(
            (0..super::super::limits::MAX_BINDINGS)
                .map(|binding| (format!("$b{binding}"), concept.clone()))
                .collect(),
        ));
        let two_raw_rows = raw_row.encoded_bytes().unwrap().checked_mul(2).unwrap();
        assert!(two_raw_rows <= maximum);

        let long_label = format!("legacy_{}", "x".repeat(4096));
        let long_registry = DescriptorRegistry::new();
        long_registry
            .register_entity(EntityDescriptor {
                type_name: long_label.clone(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: Vec::new(),
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let long_shape = selection(&long_label, 1);
        let long_limit = exactly_one_proof_byte_limit(&long_registry, &long_shape).unwrap();
        assert!(long_limit > one_binding);
        let long_concept = serde_json::json!({
            "category": "entity",
            "label": long_label,
            "iid": iid,
        });
        let long_raw_row = AnswerItem::Row(serde_json::json!({"$b0": long_concept}));
        let two_long_rows = long_raw_row
            .encoded_bytes()
            .unwrap()
            .checked_mul(2)
            .unwrap();
        assert!(two_long_rows <= long_limit);
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
        tuple_answers: usize,
        solution_statements: Vec<TypedFetchRows>,
        solution_limits: Vec<(u64, u64)>,
        tuple_statements: Vec<TypedFetchRows>,
        tuple_limits: Vec<(u64, u64)>,
        root_statements: Vec<TypedRootScan>,
        root_limits: Vec<(u64, u64)>,
        rematch_statements: Vec<TypedPageRematch>,
        rematch_limits: Vec<(u64, u64)>,
        hydration_statements: Vec<TypedHydrateThings>,
        hydration_limits: Vec<(u64, u64)>,
    }

    struct ProofCapabilityLockState {
        pause_hydration: AtomicBool,
        hydration_entered: Notify,
        resume_hydration: Notify,
        blocker_entered: Notify,
    }

    impl ProofCapabilityLockState {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                pause_hydration: AtomicBool::new(true),
                hydration_entered: Notify::new(),
                resume_hydration: Notify::new(),
                blocker_entered: Notify::new(),
            })
        }
    }

    struct RecordingBackend {
        events: Arc<Mutex<Events>>,
        capabilities: CapabilitySet,
        solutions: RecordedAnswers,
        hydrations: RecordedAnswers,
        tuple_proof: bool,
        tuple_response_override: Option<Result<Vec<AnswerItem>, String>>,
        close_failure: Option<String>,
        forged_early_stop: Option<RecordedAnswerKind>,
        forged_underreported_stats: Option<RecordedAnswerKind>,
        proof_capability_lock: Option<Arc<ProofCapabilityLockState>>,
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
            let tuple_proof = self.tuple_proof;
            let tuple_response_override = self.tuple_response_override.clone();
            let close_failure = self.close_failure.clone();
            let forged_early_stop = self.forged_early_stop;
            let forged_underreported_stats = self.forged_underreported_stats;
            let proof_capability_lock = self.proof_capability_lock.clone();
            Box::pin(async move {
                events.lock().unwrap().opens += 1;
                Ok(Box::new(RecordingTransaction {
                    events,
                    solutions,
                    hydrations,
                    tuple_proof,
                    pending_tuple_evidence: None,
                    tuple_response_override,
                    close_failure,
                    forged_early_stop,
                    forged_underreported_stats,
                    proof_capability_lock,
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
        tuple_proof: bool,
        pending_tuple_evidence: Option<Result<Vec<AnswerItem>, String>>,
        tuple_response_override: Option<Result<Vec<AnswerItem>, String>>,
        close_failure: Option<String>,
        forged_early_stop: Option<RecordedAnswerKind>,
        forged_underreported_stats: Option<RecordedAnswerKind>,
        proof_capability_lock: Option<Arc<ProofCapabilityLockState>>,
    }

    impl TransactionOps for RecordingTransaction {
        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            let proof_capability_lock = self.proof_capability_lock.clone();
            Box::pin(async move {
                let Some(state) = proof_capability_lock else {
                    panic!("selected executor used legacy string query")
                };
                state.blocker_entered.notify_one();
                std::future::pending::<Result<QueryResult, OrmError>>().await
            })
        }

        fn query_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let recorded_limits = (limits.max_items, limits.max_bytes);
            let events = Arc::clone(&self.events);
            let forged_early_stop = self.forged_early_stop;
            let forged_underreported_stats = self.forged_underreported_stats;
            // Evidence runs before the internal tuple proof so released
            // statement/item/byte errors retain precedence. Preserve the same
            // recorded snapshot for the proof instead of consuming a second
            // synthetic provider response.
            let response = self
                .solutions
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            self.pending_tuple_evidence = self.tuple_proof.then(|| response.clone());
            Box::pin(async move {
                let response = response.map(|items| {
                    items
                        .into_iter()
                        .take(query.limit as usize)
                        .collect::<Vec<_>>()
                });
                let mut locked = events.lock().unwrap();
                locked.solution_statements.push(query);
                locked.solution_limits.push(recorded_limits);
                drop(locked);
                let stats = feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Solution,
                )?;
                Ok(forge_provider_stats(
                    stats,
                    forged_early_stop,
                    forged_underreported_stats,
                    RecordedAnswerKind::Solution,
                ))
            })
        }

        fn supports_exactly_one_tuple_proof(&self) -> bool {
            self.tuple_proof
        }

        fn query_tuple_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedFetchRows,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let recorded_limits = (limits.max_items, limits.max_bytes);
            let events = Arc::clone(&self.events);
            let forged_early_stop = self.forged_early_stop;
            let forged_underreported_stats = self.forged_underreported_stats;
            let pending_tuple_evidence = self.pending_tuple_evidence.take();
            let response = self
                .tuple_response_override
                .take()
                .or(pending_tuple_evidence)
                .unwrap_or_else(|| {
                    self.solutions
                        .lock()
                        .unwrap()
                        .pop_front()
                        .unwrap_or(Ok(vec![]))
                });
            Box::pin(async move {
                let mut locked = events.lock().unwrap();
                locked.tuple_statements.push(query.clone());
                locked.tuple_limits.push(recorded_limits);
                drop(locked);
                let response = project_recorded_tuple(response, &query.projection, query.limit);
                let stats = feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Tuple,
                )?;
                Ok(forge_provider_stats(
                    stats,
                    forged_early_stop,
                    forged_underreported_stats,
                    RecordedAnswerKind::Tuple,
                ))
            })
        }

        fn hydrate_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedHydrateThings,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let recorded_limits = (limits.max_items, limits.max_bytes);
            let answer_limit = hydration_answer_limit(&query);
            let events = Arc::clone(&self.events);
            let forged_early_stop = self.forged_early_stop;
            let forged_underreported_stats = self.forged_underreported_stats;
            let proof_capability_lock = self.proof_capability_lock.clone();
            let response = self
                .hydrations
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                if let Some(state) = proof_capability_lock
                    && state.pause_hydration.swap(false, AtomicOrdering::SeqCst)
                {
                    state.hydration_entered.notify_one();
                    state.resume_hydration.notified().await;
                }
                let answer_limit = usize::try_from(answer_limit?).unwrap_or(usize::MAX);
                let response =
                    response.map(|items| items.into_iter().take(answer_limit).collect::<Vec<_>>());
                let mut locked = events.lock().unwrap();
                locked.hydration_statements.push(query);
                locked.hydration_limits.push(recorded_limits);
                drop(locked);
                let stats = feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Hydration,
                )?;
                Ok(forge_provider_stats(
                    stats,
                    forged_early_stop,
                    forged_underreported_stats,
                    RecordedAnswerKind::Hydration,
                ))
            })
        }

        fn query_root_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedRootScan,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let recorded_limits = (limits.max_items, limits.max_bytes);
            let events = Arc::clone(&self.events);
            let forged_early_stop = self.forged_early_stop;
            let forged_underreported_stats = self.forged_underreported_stats;
            let response = self
                .solutions
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                let response = project_recorded_roots(response, query.root, query.limit);
                let mut locked = events.lock().unwrap();
                locked.root_statements.push(query);
                locked.root_limits.push(recorded_limits);
                drop(locked);
                let stats = feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Root,
                )?;
                Ok(forge_provider_stats(
                    stats,
                    forged_early_stop,
                    forged_underreported_stats,
                    RecordedAnswerKind::Root,
                ))
            })
        }

        fn rematch_page_typed_bounded<'a>(
            &'a mut self,
            query: &'a TypedPageRematch,
            limits: BoundedAnswerLimits,
            consumer: &'a mut dyn AnswerConsumer,
        ) -> BoxFuture<'a, Result<BoundedAnswerStats, OrmError>> {
            let query = query.clone();
            let recorded_limits = (limits.max_items, limits.max_bytes);
            let answer_limit = usize::try_from(limits.max_items).unwrap_or(usize::MAX);
            let events = Arc::clone(&self.events);
            let forged_early_stop = self.forged_early_stop;
            let forged_underreported_stats = self.forged_underreported_stats;
            let response = self
                .hydrations
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(vec![]));
            Box::pin(async move {
                let response =
                    response.map(|items| items.into_iter().take(answer_limit).collect::<Vec<_>>());
                let mut locked = events.lock().unwrap();
                locked.rematch_statements.push(query);
                locked.rematch_limits.push(recorded_limits);
                drop(locked);
                let stats = feed_recorded(
                    response,
                    limits,
                    consumer,
                    &events,
                    RecordedAnswerKind::Rematch,
                )?;
                Ok(forge_provider_stats(
                    stats,
                    forged_early_stop,
                    forged_underreported_stats,
                    RecordedAnswerKind::Rematch,
                ))
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

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum RecordedAnswerKind {
        Solution,
        Tuple,
        Root,
        Hydration,
        Rematch,
    }

    fn forge_provider_stats(
        mut stats: BoundedAnswerStats,
        forged_early_stop: Option<RecordedAnswerKind>,
        forged_underreported_stats: Option<RecordedAnswerKind>,
        current: RecordedAnswerKind,
    ) -> BoundedAnswerStats {
        if forged_early_stop == Some(current) {
            stats.stopped_early = true;
        }
        if forged_underreported_stats == Some(current) {
            stats.processed_items = 0;
            stats.response_bytes = 0;
        }
        stats
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
                    RecordedAnswerKind::Tuple => &mut events.tuple_answers,
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

    fn project_recorded_tuple(
        response: Result<Vec<AnswerItem>, String>,
        projection: &[u16],
        limit: u64,
    ) -> Result<Vec<AnswerItem>, String> {
        response.map(|items| {
            let mut seen = BTreeSet::new();
            let mut projected = Vec::new();
            for item in items {
                let identity = recorded_tuple_identity(&item, projection);
                let item = match item {
                    AnswerItem::Row(Value::Object(mut row)) => {
                        if let Some(Value::Array(bindings)) = row.get_mut("bindings") {
                            bindings.retain(|assignment| {
                                assignment
                                    .get("binding")
                                    .and_then(Value::as_u64)
                                    .and_then(|binding| u16::try_from(binding).ok())
                                    .is_some_and(|binding| projection.contains(&binding))
                            });
                            row.insert("satisfied_role_edges".into(), Value::Array(Vec::new()));
                        }
                        AnswerItem::Row(Value::Object(row))
                    }
                    other => other,
                };
                if identity.is_none_or(|identity| seen.insert(identity)) {
                    projected.push(item);
                    if projected.len() as u64 >= limit {
                        break;
                    }
                }
            }
            projected
        })
    }

    fn project_recorded_roots(
        response: Result<Vec<AnswerItem>, String>,
        root: u16,
        limit: Option<u64>,
    ) -> Result<Vec<AnswerItem>, String> {
        response.map(|items| {
            let mut seen = BTreeSet::new();
            let mut projected = Vec::new();
            for item in items {
                let identity = recorded_binding_iid(&item, root).map(str::to_owned);
                if identity.is_none_or(|identity| seen.insert(identity)) {
                    projected.push(item);
                    if limit.is_some_and(|limit| projected.len() as u64 >= limit) {
                        break;
                    }
                }
            }
            projected
        })
    }

    fn recorded_tuple_identity(item: &AnswerItem, projection: &[u16]) -> Option<Vec<String>> {
        projection
            .iter()
            .map(|binding| recorded_binding_iid(item, *binding).map(str::to_owned))
            .collect()
    }

    fn recorded_binding_iid(item: &AnswerItem, expected: u16) -> Option<&str> {
        let AnswerItem::Row(Value::Object(row)) = item else {
            return None;
        };
        let Value::Array(bindings) = row.get("bindings")? else {
            return None;
        };
        bindings.iter().find_map(|assignment| {
            let binding = assignment.get("binding")?.as_u64()?;
            (binding == u64::from(expected))
                .then(|| assignment.get("concept_id")?.as_str())
                .flatten()
        })
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
        StatementThenClose,
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
                pending_statement: AtomicBool::new(matches!(
                    phase,
                    PendingProviderPhase::Statement | PendingProviderPhase::StatementThenClose
                )),
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
                if matches!(
                    state.phase,
                    PendingProviderPhase::Close | PendingProviderPhase::StatementThenClose
                ) {
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

    fn person_with_hidden_witness_request(registry: &DescriptorRegistry) -> MatchRequest {
        let person = BindingId::new(0);
        let witness = BindingId::new(1);
        let descriptor = registry.descriptor_id("person").unwrap();
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: person,
                        descriptor: descriptor.clone(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: witness,
                        descriptor,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: None,
                allowed_cross_joins: BTreeSet::from([BindingPair::new(person, witness)]),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding: person }],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::ExactlyOne,
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
            tuple_proof: true,
            tuple_response_override: None,
            close_failure,
            forged_early_stop: None,
            forged_underreported_stats: None,
            proof_capability_lock: None,
        };
        (Database::with_backend(Box::new(backend), "test"), events)
    }

    fn database_with_forged_early_stop(
        capabilities: CapabilitySet,
        solutions: Vec<Result<Vec<AnswerItem>, String>>,
        hydrations: Vec<Result<Vec<AnswerItem>, String>>,
        forged_early_stop: RecordedAnswerKind,
    ) -> (Database, Arc<Mutex<Events>>) {
        let events = Arc::new(Mutex::new(Events::default()));
        let backend = RecordingBackend {
            events: Arc::clone(&events),
            capabilities,
            solutions: Arc::new(Mutex::new(solutions.into())),
            hydrations: Arc::new(Mutex::new(hydrations.into())),
            tuple_proof: true,
            tuple_response_override: None,
            close_failure: None,
            forged_early_stop: Some(forged_early_stop),
            forged_underreported_stats: None,
            proof_capability_lock: None,
        };
        (Database::with_backend(Box::new(backend), "test"), events)
    }

    fn database_with_tuple_response(
        capabilities: CapabilitySet,
        solutions: Vec<Result<Vec<AnswerItem>, String>>,
        hydrations: Vec<Result<Vec<AnswerItem>, String>>,
        tuple_response: Result<Vec<AnswerItem>, String>,
    ) -> (Database, Arc<Mutex<Events>>) {
        let events = Arc::new(Mutex::new(Events::default()));
        let backend = RecordingBackend {
            events: Arc::clone(&events),
            capabilities,
            solutions: Arc::new(Mutex::new(solutions.into())),
            hydrations: Arc::new(Mutex::new(hydrations.into())),
            tuple_proof: true,
            tuple_response_override: Some(tuple_response),
            close_failure: None,
            forged_early_stop: None,
            forged_underreported_stats: None,
            proof_capability_lock: None,
        };
        (Database::with_backend(Box::new(backend), "test"), events)
    }

    fn database_with_underreported_stats(
        capabilities: CapabilitySet,
        solutions: Vec<Result<Vec<AnswerItem>, String>>,
        hydrations: Vec<Result<Vec<AnswerItem>, String>>,
        forged_underreported_stats: RecordedAnswerKind,
    ) -> (Database, Arc<Mutex<Events>>) {
        let events = Arc::new(Mutex::new(Events::default()));
        let backend = RecordingBackend {
            events: Arc::clone(&events),
            capabilities,
            solutions: Arc::new(Mutex::new(solutions.into())),
            hydrations: Arc::new(Mutex::new(hydrations.into())),
            tuple_proof: true,
            tuple_response_override: None,
            close_failure: None,
            forged_early_stop: None,
            forged_underreported_stats: Some(forged_underreported_stats),
            proof_capability_lock: None,
        };
        (Database::with_backend(Box::new(backend), "test"), events)
    }

    fn database_without_tuple_proof(
        capabilities: CapabilitySet,
        solutions: Vec<Result<Vec<AnswerItem>, String>>,
        hydrations: Vec<Result<Vec<AnswerItem>, String>>,
    ) -> (Database, Arc<Mutex<Events>>) {
        let events = Arc::new(Mutex::new(Events::default()));
        let backend = RecordingBackend {
            events: Arc::clone(&events),
            capabilities,
            solutions: Arc::new(Mutex::new(solutions.into())),
            hydrations: Arc::new(Mutex::new(hydrations.into())),
            tuple_proof: false,
            tuple_response_override: None,
            close_failure: None,
            forged_early_stop: None,
            forged_underreported_stats: None,
            proof_capability_lock: None,
        };
        (Database::with_backend(Box::new(backend), "test"), events)
    }

    fn database_with_proof_capability_lock()
    -> (Database, Arc<Mutex<Events>>, Arc<ProofCapabilityLockState>) {
        let events = Arc::new(Mutex::new(Events::default()));
        let proof_capability_lock = ProofCapabilityLockState::new();
        let backend = RecordingBackend {
            events: Arc::clone(&events),
            capabilities: CapabilitySet::all(),
            solutions: Arc::new(Mutex::new(
                vec![Ok(vec![solution(&[(0, "0x01")], &[])])].into(),
            )),
            hydrations: Arc::new(Mutex::new(
                vec![Ok(vec![person_hydration(0, "0x01", "Alice")])].into(),
            )),
            tuple_proof: true,
            tuple_response_override: None,
            close_failure: None,
            forged_early_stop: None,
            forged_underreported_stats: None,
            proof_capability_lock: Some(Arc::clone(&proof_capability_lock)),
        };
        (
            Database::with_backend(Box::new(backend), "test"),
            events,
            proof_capability_lock,
        )
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
    fn every_model_execution_diagnostic_has_an_explicit_released_mapping() {
        let source = include_str!("../query_v2_model.rs");
        let emitted = source
            .split('"')
            .filter(|value| value.starts_with("query_v2_model_"))
            .collect::<BTreeSet<_>>();
        assert!(
            emitted.len() >= 50,
            "source scan did not find the complete model diagnostic vocabulary"
        );
        for code in emitted {
            // Exactly-one carries a dynamic cardinality detail and is mapped by
            // the dedicated compatibility arm before the static mapper.
            if code == "query_v2_model_exactly_one" {
                continue;
            }
            let diagnostic =
                crate::query_v2::failure(DiagnosticCategory::Integrity, code, "mapping probe");
            let (_, mapped, _) = released_execution_diagnostic(&diagnostic);
            assert_ne!(
                mapped, "unmapped_model_execution_diagnostic",
                "{code} has no explicit released error mapping"
            );
        }

        let future = crate::query_v2::failure(
            DiagnosticCategory::Integrity,
            "query_v2_model_future_unmapped_code",
            "mapping probe",
        );
        assert_eq!(
            released_execution_diagnostic(&future).1,
            "unmapped_model_execution_diagnostic"
        );
    }

    #[test]
    fn model_contract_and_claim_failures_preserve_released_error_surfaces() {
        fn assert_surface(
            diagnostic: Diagnostic,
            category: MatchErrorCategory,
            code: &str,
            message: &str,
            path: Vec<MatchErrorPathSegment>,
        ) {
            let error =
                compatibility_execution_error(QueryV2ExecutionError::Validation(diagnostic));
            let error = match_error(&error);
            assert_eq!(error.category(), category);
            assert_eq!(error.code().as_str(), code);
            assert_eq!(error.message(), message);
            assert_eq!(error.path().segments(), path);
        }

        let integrity = |code| {
            crate::query_v2::failure(DiagnosticCategory::Integrity, code, "untrusted detail")
        };
        let invalid = |code| {
            crate::query_v2::failure(
                DiagnosticCategory::InvalidContract,
                code,
                "untrusted detail",
            )
        };
        let resource = |code| {
            crate::query_v2::failure(DiagnosticCategory::ResourceLimit, code, "untrusted detail")
        };

        for code in [
            "query_v2_model_contract_missing",
            "query_v2_model_provider_plan_missing",
            "query_v2_model_provider_plan_mismatch",
            "query_v2_model_tuple_plan",
        ] {
            assert_surface(
                invalid(code),
                MatchErrorCategory::ResultDecode,
                "result_operation_mismatch",
                "adapted V2 result variant does not match the validated released operation",
                vec![MatchErrorPathSegment::Result],
            );
        }
        assert_surface(
            invalid("query_v2_model_page_total_plan"),
            MatchErrorCategory::ResultDecode,
            "page_operation_mismatch",
            "typed page plan does not belong to a PageBy operation",
            vec![MatchErrorPathSegment::Result],
        );
        assert_surface(
            invalid("query_v2_model_rows_collection"),
            MatchErrorCategory::InvalidPlan,
            "unsupported_collection_slot",
            "selected-row execution does not support collected output slots",
            vec![MatchErrorPathSegment::Operation],
        );
        assert_surface(
            resource("query_v2_model_count_overflow"),
            MatchErrorCategory::ResourceLimit,
            "processed_item_counter_overflow",
            "provider resource limits prevented complete typed match evidence",
            vec![MatchErrorPathSegment::ProviderEvidence],
        );
        for code in [
            "query_v2_model_hydration_authority",
            "query_v2_model_hydration_state",
        ] {
            assert_surface(
                invalid(code),
                MatchErrorCategory::ResultDecode,
                "malformed_hydration_document",
                "provider evidence failed canonical typed match decoding",
                vec![MatchErrorPathSegment::ProviderEvidence],
            );
        }

        for (code, message) in [
            (
                "query_v2_model_predicate_equal_type",
                "provider predicate values do not have the same validated type",
            ),
            (
                "query_v2_model_predicate_order_type",
                "provider predicate values are not order-compatible",
            ),
            (
                "query_v2_model_predicate_string_type",
                "provider predicate values do not support the validated string operator",
            ),
        ] {
            assert_surface(
                integrity(code),
                MatchErrorCategory::ResultDecode,
                "predicate_value_type_mismatch",
                message,
                vec![MatchErrorPathSegment::Result],
            );
        }
        assert_surface(
            invalid("query_v2_model_regex"),
            MatchErrorCategory::InvalidPlan,
            "invalid_regex_pattern",
            "validated request contains an invalid regular expression",
            vec![MatchErrorPathSegment::Predicate],
        );

        let field_path = vec![
            MatchErrorPathSegment::Result,
            MatchErrorPathSegment::Field(FieldId::new(DescriptorId::new("entity:person"), "name")),
        ];
        for (code, released_code, message) in [
            (
                "query_v2_model_order_non_scalar",
                "non_scalar_order_evidence",
                "provider returned multiple values for a validated scalar order field",
            ),
            (
                "query_v2_model_order_missing",
                "missing_order_value",
                "provider omitted a value required by stable ordering",
            ),
            (
                "query_v2_model_order_value_type",
                "order_value_type_mismatch",
                "provider order values are not mutually comparable",
            ),
        ] {
            let diagnostic = integrity(code)
                .with_detail("field_owner", "entity:person")
                .with_detail("field_name", "name");
            assert_surface(
                diagnostic,
                MatchErrorCategory::ResultDecode,
                released_code,
                message,
                field_path.clone(),
            );
        }
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

    #[tokio::test]
    async fn drained_decode_failure_is_canonical_and_redacted_before_cleanup_warning() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let sentinel = "credential=top-secret";
        let malformed = AnswerItem::Row(serde_json::json!({
            "bindings": [{"binding": sentinel, "concept_id": "0x01"}],
            "satisfied_role_edges": [],
        }));
        let (database, events) = database_with_close_failure(
            CapabilitySet::all(),
            vec![Ok(vec![malformed])],
            vec![],
            Some(format!("close {sentinel}")),
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        let canonical = match_error(&error);
        assert_eq!(canonical.category(), MatchErrorCategory::ResultDecode);
        assert_eq!(canonical.code().as_str(), "malformed_root_row");
        assert_eq!(
            canonical.message(),
            "provider evidence failed canonical typed match decoding"
        );
        assert_eq!(
            canonical.path().segments(),
            &[MatchErrorPathSegment::ProviderEvidence]
        );
        assert!(canonical.details().is_empty());
        assert!(!error.to_string().contains(sentinel));
        assert_eq!(events.lock().unwrap().closes, 1);
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
    async fn owned_close_failure_after_success_preserves_released_error_precedence() {
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
            .expect_err("released V1 returns a close failure after successful execution");
        assert_eq!(match_code(&error), "provider_transaction_close_failed");
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
    async fn count_and_exists_use_distinct_root_streams_and_exists_reaches_semantic_eof() {
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

    fn reduction_registry() -> DescriptorRegistry {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    key("name"),
                    OwnedAttributeDescriptor {
                        field_name: "age".into(),
                        attr_name: "person-age".into(),
                        value_type: ValueType::Long,
                        annotations: vec![Annotation::Card(0, Some(1))],
                        is_optional: true,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeDescriptor {
                        field_name: "department".into(),
                        attr_name: "department".into(),
                        value_type: ValueType::String,
                        annotations: vec![Annotation::Card(0, None)],
                        is_optional: true,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
                    OwnedAttributeDescriptor {
                        field_name: "city".into(),
                        attr_name: "city".into(),
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
        registry
            .register_entity(EntityDescriptor {
                type_name: "team".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    fn person_age_wire(binding: u16, concept_id: &str, name: &str, age: Option<i64>) -> Value {
        let mut attributes = vec![serde_json::json!({
            "field": "name",
            "value_type": "string",
            "values": [name],
        })];
        if let Some(age) = age {
            attributes.push(serde_json::json!({
                "field": "age",
                "value_type": "long",
                "values": [age],
            }));
        }
        serde_json::json!({
            "binding": binding,
            "concept_id": concept_id,
            "concrete_type": "person",
            "kind": "entity",
            "attributes": attributes,
            "roles": [],
        })
    }

    fn person_department_wire(
        binding: u16,
        concept_id: &str,
        name: &str,
        departments: &[&str],
    ) -> Value {
        serde_json::json!({
            "binding": binding,
            "concept_id": concept_id,
            "concrete_type": "person",
            "kind": "entity",
            "attributes": [
                {
                    "field": "name",
                    "value_type": "string",
                    "values": [name],
                },
                {
                    "field": "department",
                    "value_type": "string",
                    "values": departments,
                },
            ],
            "roles": [],
        })
    }

    fn person_city_department_wire(
        binding: u16,
        concept_id: &str,
        name: &str,
        cities: &[&str],
        departments: &[&str],
    ) -> Value {
        serde_json::json!({
            "binding": binding,
            "concept_id": concept_id,
            "concrete_type": "person",
            "kind": "entity",
            "attributes": [
                {
                    "field": "name",
                    "value_type": "string",
                    "values": [name],
                },
                {
                    "field": "city",
                    "value_type": "string",
                    "values": cities,
                },
                {
                    "field": "department",
                    "value_type": "string",
                    "values": departments,
                },
            ],
            "roles": [],
        })
    }

    fn person_reduce_request(
        registry: &DescriptorRegistry,
        reducers: Vec<ReduceTerm>,
    ) -> MatchRequest {
        person_root_request(registry, |root| MatchOperation::ReduceBy {
            root,
            group: None,
            reducers,
        })
    }

    fn age_input(registry: &DescriptorRegistry) -> BoundFieldId {
        BoundFieldId::new(
            BindingId::new(0),
            FieldId::new(registry.descriptor_id("person").unwrap(), "age"),
        )
    }

    #[tokio::test]
    async fn ungrouped_reduction_reduces_the_distinct_root_stream_through_one_rematch() {
        let registry = reduction_registry();
        let age = age_input(&registry);
        let validated = validate_match_request(
            &registry,
            person_reduce_request(
                &registry,
                vec![
                    ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    },
                    ReduceTerm {
                        reduction: Reduction::Sum,
                        input: Some(age.clone()),
                    },
                    ReduceTerm {
                        reduction: Reduction::Mean,
                        input: Some(age.clone()),
                    },
                    ReduceTerm {
                        reduction: Reduction::Min,
                        input: Some(age),
                    },
                ],
            ),
        )
        .unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
            ])],
            vec![Ok(vec![
                rematch_entities(vec![person_age_wire(0, "0x01", "Alice", Some(10))]),
                rematch_entities(vec![person_age_wire(0, "0x01", "Alice", Some(10))]),
                rematch_entities(vec![person_age_wire(0, "0x02", "Bea", Some(30))]),
                rematch_entities(vec![person_age_wire(0, "0x03", "Cleo", None)]),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Reduction { root, group, rows } = result.result()
        else {
            panic!("expected reduction result")
        };
        assert_eq!(*root, BindingId::new(0));
        assert!(group.is_none());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].group().is_none());
        assert_eq!(
            rows[0].values(),
            &[
                ReducedValue::Count(3),
                ReducedValue::Long(Some(40)),
                ReducedValue::Double(Some(20.0)),
                ReducedValue::Long(Some(10)),
            ]
        );
        let events = events.lock().unwrap();
        assert_eq!(events.root_statements.len(), 1);
        assert_eq!(events.rematch_statements.len(), 1);
        assert_eq!((events.opens, events.closes), (1, 1));
    }

    #[tokio::test]
    async fn grouped_reduction_orders_rows_by_group_identity_and_admits_absent_inputs() {
        let registry = reduction_registry();
        let root = BindingId::new(0);
        let team = BindingId::new(1);
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
                        id: team,
                        descriptor: registry.descriptor_id("team").unwrap(),
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: None,
                allowed_cross_joins: BTreeSet::from([BindingPair::new(root, team)]),
            },
            MatchOperation::ReduceBy {
                root,
                group: Some(team),
                reducers: vec![
                    ReduceTerm {
                        reduction: Reduction::Count,
                        input: None,
                    },
                    ReduceTerm {
                        reduction: Reduction::Sum,
                        input: Some(age_input(&registry)),
                    },
                ],
            },
        );
        let validated = validate_match_request(&registry, request).unwrap();
        let member = |person: Value, team_id: &str, team_name: &str| {
            rematch_entities(vec![person, entity_wire(1, "team", team_id, team_name)])
        };
        let (database, _events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
            ])],
            vec![Ok(vec![
                member(
                    person_age_wire(0, "0x01", "Alice", Some(10)),
                    "0x22",
                    "Beta",
                ),
                member(person_age_wire(0, "0x02", "Bea", Some(30)), "0x22", "Beta"),
                member(person_age_wire(0, "0x03", "Cleo", None), "0x21", "Alpha"),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Reduction { root, group, rows } = result.result()
        else {
            panic!("expected reduction result")
        };
        assert_eq!(*root, BindingId::new(0));
        assert_eq!(*group, Some(team));
        assert_eq!(
            rows.iter()
                .map(|row| row.group().unwrap().concept_id().as_str())
                .collect::<Vec<_>>(),
            vec!["0x21", "0x22"]
        );
        assert_eq!(
            rows[0].values(),
            &[ReducedValue::Count(1), ReducedValue::Long(Some(0))]
        );
        assert_eq!(
            rows[1].values(),
            &[ReducedValue::Count(2), ReducedValue::Long(Some(40))]
        );
    }

    #[tokio::test]
    async fn field_grouped_reduction_groups_distinct_roots_by_each_owned_value() {
        let registry = reduction_registry();
        let root = BindingId::new(0);
        let department = BoundFieldId::new(
            root,
            FieldId::new(registry.descriptor_id("person").unwrap(), "department"),
        );
        let request = person_root_request(&registry, |root| MatchOperation::ReduceByField {
            root,
            group: department.clone(),
            reducers: vec![ReduceTerm {
                reduction: Reduction::Count,
                input: None,
            }],
        });
        let validated = validate_match_request(&registry, request).unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
                solution(&[(0, "0x04")], &[]),
            ])],
            vec![Ok(vec![
                rematch_entities(vec![person_department_wire(
                    0,
                    "0x01",
                    "Alice",
                    &["Beta", "Shared"],
                )]),
                rematch_entities(vec![person_department_wire(0, "0x02", "Bea", &["Beta"])]),
                rematch_entities(vec![person_department_wire(
                    0,
                    "0x03",
                    "Cleo",
                    &["Alpha", "Shared"],
                )]),
                rematch_entities(vec![person_department_wire(0, "0x04", "Dana", &[])]),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::FieldReduction { root, group, rows } =
            result.result()
        else {
            panic!("expected field-grouped reduction result")
        };
        assert_eq!(*root, BindingId::new(0));
        assert_eq!(group, &department);
        assert_eq!(
            rows.iter()
                .map(|row| (row.field_group().unwrap().clone(), row.values().to_vec()))
                .collect::<Vec<_>>(),
            vec![
                (
                    AttributeValue::String("Alpha".into()),
                    vec![ReducedValue::Count(1)],
                ),
                (
                    AttributeValue::String("Beta".into()),
                    vec![ReducedValue::Count(2)],
                ),
                (
                    AttributeValue::String("Shared".into()),
                    vec![ReducedValue::Count(2)],
                ),
            ]
        );
        let events = events.lock().unwrap();
        assert_eq!(events.root_statements.len(), 1);
        assert_eq!(events.rematch_statements.len(), 1);
    }

    #[tokio::test]
    async fn tuple_field_grouped_reduction_uses_cartesian_values_and_distinct_roots() {
        let registry = reduction_registry();
        let root = BindingId::new(0);
        let city = BoundFieldId::new(
            root,
            FieldId::new(registry.descriptor_id("person").unwrap(), "city"),
        );
        let department = BoundFieldId::new(
            root,
            FieldId::new(registry.descriptor_id("person").unwrap(), "department"),
        );
        let request = person_root_request(&registry, |root| MatchOperation::ReduceByFields {
            root,
            groups: vec![city.clone(), department.clone()],
            reducers: vec![ReduceTerm {
                reduction: Reduction::Count,
                input: None,
            }],
        });
        let validated = validate_match_request(&registry, request).unwrap();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
            ])],
            vec![Ok(vec![
                rematch_entities(vec![person_city_department_wire(
                    0,
                    "0x01",
                    "Alice",
                    &["New York", "Paris"],
                    &["Engineering", "Shared"],
                )]),
                rematch_entities(vec![person_city_department_wire(
                    0,
                    "0x01",
                    "Alice",
                    &["New York", "Paris"],
                    &["Engineering", "Shared"],
                )]),
                rematch_entities(vec![person_city_department_wire(
                    0,
                    "0x02",
                    "Bea",
                    &["New York"],
                    &["Engineering"],
                )]),
                rematch_entities(vec![person_city_department_wire(
                    0,
                    "0x03",
                    "Cleo",
                    &[],
                    &["Sales"],
                )]),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::FieldTupleReduction { root, groups, rows } =
            result.result()
        else {
            panic!("expected tuple-field-grouped reduction result")
        };
        assert_eq!(*root, BindingId::new(0));
        assert_eq!(groups, &[city, department]);
        assert_eq!(
            rows.iter()
                .map(|row| (row.field_groups().unwrap().to_vec(), row.values().to_vec()))
                .collect::<Vec<_>>(),
            vec![
                (
                    vec![
                        AttributeValue::String("New York".into()),
                        AttributeValue::String("Engineering".into()),
                    ],
                    vec![ReducedValue::Count(2)],
                ),
                (
                    vec![
                        AttributeValue::String("New York".into()),
                        AttributeValue::String("Shared".into()),
                    ],
                    vec![ReducedValue::Count(1)],
                ),
                (
                    vec![
                        AttributeValue::String("Paris".into()),
                        AttributeValue::String("Engineering".into()),
                    ],
                    vec![ReducedValue::Count(1)],
                ),
                (
                    vec![
                        AttributeValue::String("Paris".into()),
                        AttributeValue::String("Shared".into()),
                    ],
                    vec![ReducedValue::Count(1)],
                ),
            ]
        );
        let events = events.lock().unwrap();
        assert_eq!(events.root_statements.len(), 1);
        assert_eq!(events.rematch_statements.len(), 1);
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
                solution(&[(0, "0x02")], &[]),
                solution(&[(0, "0x03")], &[]),
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
                solution(&[(0, "0x02")], &[]),
            ])],
            vec![Ok(vec![person_rematch("0x01", "Alice")])],
        );
        let error = short
            .execute_match_with_limits(
                &registry,
                &page,
                MatchExecutionLimits::tightened(
                    4,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "selected_root_set_mismatch");
        assert_eq!(events.lock().unwrap().rematch_statements.len(), 1);
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
                person_hydration(0, "0x03", "Cara"),
            ])],
        );
        let error = multiple
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "not_unique");
        let events = events.lock().unwrap();
        assert!(events.tuple_statements.is_empty());
        assert_eq!(events.solution_answers, 2);
        assert_eq!(events.solution_statements[0].offset, 0);
        assert_eq!(events.solution_statements[0].limit, 2);
        assert_eq!(events.closes, 1);
    }

    #[tokio::test]
    async fn hidden_witness_duplicates_drain_released_scan_before_hydration_and_proof() {
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
        let company_iids = (0_u16..64)
            .map(|index| format!("0x{:02x}", 0x10 + index))
            .collect::<Vec<_>>();
        let hidden_witnesses = company_iids
            .iter()
            .map(|company_iid| solution(&[(0, "0x01"), (1, company_iid)], &[]))
            .collect();
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(hidden_witnesses)],
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
        assert_eq!(events.tuple_answers, 1);
        assert_eq!(events.solution_answers, 64);
        assert_eq!(events.hydration_statements.len(), 1);
        assert_eq!(
            events.hydration_statements[0].targets[1].concept_ids,
            vec!["0x10"]
        );
    }

    #[tokio::test]
    async fn exactly_one_cardinality_uses_the_complete_public_tuple() {
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
        let person = BindingId::new(0);
        let company = BindingId::new(1);
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: person,
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
                allowed_cross_joins: BTreeSet::from([BindingPair::new(person, company)]),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![
                        FetchSlot::One { binding: person },
                        FetchSlot::One { binding: company },
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
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01"), (1, "0x10")], &[]),
                solution(&[(0, "0x01"), (1, "0x11")], &[]),
            ])],
            vec![Ok(vec![
                person_hydration(0, "0x01", "Alice"),
                AnswerItem::Document(serde_json::json!({
                    "binding": 1,
                    "concept_id": "0x10",
                    "concrete_type": "company",
                    "kind": "entity",
                    "attributes": [
                        {"field": "name", "value_type": "string", "values": ["Acme"]}
                    ],
                    "roles": [],
                })),
                AnswerItem::Document(serde_json::json!({
                    "binding": 1,
                    "concept_id": "0x11",
                    "concrete_type": "company",
                    "kind": "entity",
                    "attributes": [
                        {"field": "name", "value_type": "string", "values": ["Beta"]}
                    ],
                    "roles": [],
                })),
            ])],
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "not_unique");
        let events = events.lock().unwrap();
        assert!(events.tuple_statements.is_empty());
        assert_eq!(events.solution_answers, 2);
        assert_eq!(events.hydration_answers, 3);
        assert_eq!(events.hydration_statements.len(), 1);
    }

    #[tokio::test]
    async fn provider_limits_are_finite_and_error_suffix_drain_is_bounded() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::BoundedMany, 100),
        )
        .unwrap();

        let mut hostile_suffix = vec![AnswerItem::Row(serde_json::json!({"malformed": true}))];
        hostile_suffix
            .extend((0..100).map(|index| solution(&[(0, &format!("0x{:x}", index + 1))], &[])));
        let (malformed, events) = database(CapabilitySet::all(), vec![Ok(hostile_suffix)], vec![]);
        let error = malformed
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    100,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "malformed_solution_row");
        {
            let observed = events.lock().unwrap();
            assert_eq!(
                observed.solution_answers,
                usize::try_from(MAX_ERROR_DRAIN_ITEMS).unwrap() + 1
            );
            assert_eq!(observed.solution_limits, [(100, 4096)]);
            assert_eq!(observed.closes, 1);
        }

        let (byte_limited, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![],
        );
        let error = byte_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    1,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "response_byte_limit");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_limits, [(10, 1)]);
            assert!(
                observed
                    .solution_limits
                    .iter()
                    .all(|(_, max_bytes)| *max_bytes != u64::MAX)
            );
            assert_eq!(observed.closes, 1);
        }
    }

    #[tokio::test]
    async fn bounded_error_drain_does_not_close_caller_owned_context() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::BoundedMany, 100),
        )
        .unwrap();
        let mut hostile_suffix = vec![AnswerItem::Row(serde_json::json!({"malformed": true}))];
        hostile_suffix
            .extend((0..100).map(|index| solution(&[(0, &format!("0x{:x}", index + 1))], &[])));
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(hostile_suffix), Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
        );
        let context = database.transaction_context(TxType::Read).await.unwrap();

        let error = context
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    100,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "malformed_solution_row");
        assert_eq!(events.lock().unwrap().closes, 0);

        context.execute_match(&registry, &validated).await.unwrap();
        assert_eq!(events.lock().unwrap().closes, 0);
        context.close().await.unwrap();
        assert_eq!(events.lock().unwrap().closes, 1);
    }

    #[tokio::test]
    async fn terminal_exactly_one_uses_only_released_statement_item_and_byte_budgets() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let selected = solution(&[(0, "0x01")], &[]);
        let hydrated = person_hydration(0, "0x01", "Alice");
        let selected_bytes = selected.encoded_bytes().unwrap();
        let hydrated_bytes = hydrated.encoded_bytes().unwrap();
        let released_bytes = selected_bytes.checked_add(hydrated_bytes).unwrap();
        let (successful, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![selected])],
            vec![Ok(vec![hydrated])],
        );
        let result = successful
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    2,
                    released_bytes,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_statements(2),
            )
            .await
            .unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
        {
            let observed = events.lock().unwrap();
            assert_eq!((observed.tuple_answers, observed.solution_answers), (0, 1));
            assert_eq!(observed.hydration_answers, 1);
            assert!(observed.tuple_limits.is_empty());
            assert_eq!(observed.solution_limits, [(2, released_bytes)]);
            assert_eq!(observed.hydration_limits, [(1, hydrated_bytes)]);
            assert!(
                observed
                    .solution_limits
                    .iter()
                    .chain(&observed.hydration_limits)
                    .all(|(_, max_bytes)| *max_bytes != u64::MAX)
            );
            assert_eq!(observed.closes, 1);
        }

        let (item_limited, item_events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
        );
        let error = item_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    1,
                    released_bytes,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_statements(2),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "solution_scan_limit");
        {
            let observed = item_events.lock().unwrap();
            assert_eq!((observed.tuple_answers, observed.solution_answers), (0, 1));
            assert!(observed.hydration_limits.is_empty());
        }

        let (byte_limited, byte_events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
        );
        let error = byte_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    2,
                    released_bytes - 1,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_statements(2),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "response_byte_limit");
        let observed = byte_events.lock().unwrap();
        assert_eq!((observed.tuple_answers, observed.solution_answers), (0, 1));
        assert_eq!(observed.hydration_limits[0].1, hydrated_bytes - 1);
    }

    #[tokio::test]
    async fn exactly_one_released_budget_errors_precede_internal_tuple_proof() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let two_solutions = || {
            Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
            ])
        };

        let (owned, events) = database(CapabilitySet::all(), vec![two_solutions()], vec![]);
        let error = owned
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::default().with_max_statements(0),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "statement_count_limit");
        {
            let observed = events.lock().unwrap();
            assert!(observed.solution_statements.is_empty());
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let (item_limited, events) = database(CapabilitySet::all(), vec![two_solutions()], vec![]);
        let error = item_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    0,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "processed_item_limit");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_answers, 1);
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let (empty_item_limited, events) = database(CapabilitySet::all(), vec![Ok(vec![])], vec![]);
        let error = empty_item_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    0,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "solution_scan_limit");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_statements.len(), 1);
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let (byte_limited, events) = database(CapabilitySet::all(), vec![two_solutions()], vec![]);
        let error = byte_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    0,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "response_byte_limit");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_answers, 1);
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let first = solution(&[(0, "0x01")], &[]);
        let second = solution(&[(0, "0x02")], &[]);
        let first_bytes = first.encoded_bytes().unwrap();
        let (second_row_byte_limited, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![first.clone(), second])],
            vec![],
        );
        let error = second_row_byte_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    first_bytes,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "response_byte_limit");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_answers, 2);
            assert!(observed.hydration_statements.is_empty());
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let (malformed_second_row, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                first,
                AnswerItem::Row(serde_json::json!({"bindings": []})),
            ])],
            vec![],
        );
        let error = malformed_second_row
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    10,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "missing_provider_binding");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_answers, 2);
            assert!(observed.hydration_statements.is_empty());
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let (borrowed, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
        );
        let context = borrowed.transaction_context(TxType::Read).await.unwrap();
        let error = context
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::default().with_max_statements(0),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "statement_count_limit");
        {
            let observed = events.lock().unwrap();
            assert!(observed.solution_statements.is_empty());
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 0);
        }
        context.execute_match(&registry, &validated).await.unwrap();
        assert_eq!(events.lock().unwrap().closes, 0);
        context.close().await.unwrap();
        assert_eq!(events.lock().unwrap().closes, 1);

        let error = context
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::default().with_max_statements(0),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "statement_count_limit");
    }

    #[tokio::test]
    async fn local_observation_prevents_custom_backend_budget_underreporting() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let selected = solution(&[(0, "0x01")], &[]);
        let hydrated = person_hydration(0, "0x01", "Alice");
        let selected_bytes = selected.encoded_bytes().unwrap();
        let hydrated_bytes = hydrated.encoded_bytes().unwrap();
        let released_bytes = selected_bytes.checked_add(hydrated_bytes).unwrap();

        let (item_limited, item_events) = database_with_underreported_stats(
            CapabilitySet::all(),
            vec![Ok(vec![selected.clone()])],
            vec![Ok(vec![hydrated.clone()])],
            RecordedAnswerKind::Solution,
        );
        let error = item_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    1,
                    released_bytes,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_statements(2),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "solution_scan_limit");
        assert!(item_events.lock().unwrap().hydration_limits.is_empty());

        let (byte_limited, byte_events) = database_with_underreported_stats(
            CapabilitySet::all(),
            vec![Ok(vec![selected])],
            vec![Ok(vec![hydrated])],
            RecordedAnswerKind::Solution,
        );
        let error = byte_limited
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    2,
                    released_bytes - 1,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_statements(2),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "response_byte_limit");
        assert_eq!(
            byte_events.lock().unwrap().hydration_limits[0].1,
            hydrated_bytes - 1
        );
    }

    #[tokio::test]
    async fn hidden_witness_tuple_proof_rejects_oversized_provider_rows_after_released_evidence() {
        let registry = person_registry();
        let validated =
            validate_match_request(&registry, person_with_hidden_witness_request(&registry))
                .unwrap();
        let LoweredMatchExecution::ExactlyOneBy { selection, .. } =
            lower_match_execution(&registry, &validated).unwrap()
        else {
            panic!("expected exactly-one lowering")
        };
        let proof_limit = exactly_one_proof_byte_limit(&registry, &selection).unwrap();
        let oversized = AnswerItem::Row(serde_json::json!({
            "bindings": [{"binding": 0, "concept_id": "0x02"}],
            "satisfied_role_edges": [],
            "padding": "x".repeat(usize::try_from(proof_limit).unwrap()),
        }));
        let (database, events) = database_with_tuple_response(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01"), (1, "0x10")], &[])])],
            vec![Ok(vec![
                person_hydration(0, "0x01", "Alice"),
                person_hydration(1, "0x10", "Witness"),
            ])],
            Ok(vec![oversized]),
        );

        let error = database
            .execute_match(&registry, &validated)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "response_byte_limit");
        let observed = events.lock().unwrap();
        assert_eq!(observed.tuple_limits, [(2, proof_limit)]);
        assert_eq!(observed.solution_answers, 1);
        assert_eq!(observed.hydration_answers, 2);
        assert_eq!(observed.tuple_answers, 1);
        assert_eq!(observed.closes, 1);
    }

    #[tokio::test]
    async fn custom_typed_backend_keeps_released_exactly_one_full_scan_path() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::ExactlyOne, 1),
        )
        .unwrap();
        let selected = solution(&[(0, "0x01")], &[]);
        let hydrated = person_hydration(0, "0x01", "Alice");
        let max_bytes = selected
            .encoded_bytes()
            .unwrap()
            .checked_add(hydrated.encoded_bytes().unwrap())
            .unwrap();
        let (database, events) = database_without_tuple_proof(
            CapabilitySet::all(),
            vec![Ok(vec![selected])],
            vec![Ok(vec![hydrated])],
        );

        let result = database
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    2,
                    max_bytes,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                )
                .with_max_statements(2),
            )
            .await
            .unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
        let observed = events.lock().unwrap();
        assert!(observed.tuple_statements.is_empty());
        assert_eq!(observed.solution_statements.len(), 1);
        assert_eq!(observed.hydration_statements.len(), 1);
        assert_eq!(observed.closes, 1);
    }

    #[tokio::test]
    async fn every_selected_stream_rejects_forged_early_stop_before_accepting_evidence() {
        let registry = person_registry();

        let bounded = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::BoundedMany, 2),
        )
        .unwrap();
        let (database, events) = database_with_forged_early_stop(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
            RecordedAnswerKind::Solution,
        );
        let error = database
            .execute_match(&registry, &bounded)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "provider_stream_not_exhausted");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.solution_answers, 1);
            assert!(observed.hydration_statements.is_empty());
            assert_eq!(observed.closes, 1);
        }

        let count = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, events) = database_with_forged_early_stop(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![],
            RecordedAnswerKind::Root,
        );
        let error = database.execute_match(&registry, &count).await.unwrap_err();
        assert_eq!(match_code(&error), "provider_stream_not_exhausted");
        {
            let observed = events.lock().unwrap();
            assert_eq!(observed.root_answers, 1);
            assert_eq!(observed.closes, 1);
        }

        let page =
            validate_match_request(&registry, person_page_request(&registry, false)).unwrap();
        let (database, events) = database_with_forged_early_stop(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_rematch("0x01", "Alice")])],
            RecordedAnswerKind::Rematch,
        );
        let error = database.execute_match(&registry, &page).await.unwrap_err();
        assert_eq!(match_code(&error), "provider_stream_not_exhausted");
        {
            let observed = events.lock().unwrap();
            assert_eq!((observed.root_answers, observed.rematch_answers), (1, 1));
            assert_eq!(observed.closes, 1);
        }

        let (database, events) = database_with_forged_early_stop(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![Ok(vec![person_hydration(0, "0x01", "Alice")])],
            RecordedAnswerKind::Hydration,
        );
        let error = database
            .execute_match(&registry, &bounded)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "provider_stream_not_exhausted");
        let observed = events.lock().unwrap();
        assert_eq!(
            (observed.solution_answers, observed.hydration_answers),
            (1, 1)
        );
        assert_eq!(observed.closes, 1);
    }

    #[tokio::test]
    async fn tightened_item_ceiling_keeps_released_solution_scan_error_before_forged_stop() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_request(&registry, RowCardinality::BoundedMany, 2),
        )
        .unwrap();
        let (database, events) = database_with_forged_early_stop(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01")], &[])])],
            vec![],
            RecordedAnswerKind::Solution,
        );

        let error = database
            .execute_match_with_limits(
                &registry,
                &validated,
                MatchExecutionLimits::tightened(
                    1,
                    4096,
                    Duration::from_secs(1),
                    AnswerCancellation::default(),
                ),
            )
            .await
            .unwrap_err();

        assert_eq!(match_code(&error), "solution_scan_limit");
        let observed = events.lock().unwrap();
        assert_eq!(observed.solution_answers, 1);
        assert!(observed.hydration_statements.is_empty());
        assert_eq!(observed.closes, 1);
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
    async fn full_projection_small_window_over_hard_ceiling_reads_only_its_finite_prefix() {
        let registry = person_registry();
        let mut request = person_request(&registry, RowCardinality::BoundedMany, 1);
        let MatchOperation::FetchRows { window, .. } = &mut request.operation else {
            unreachable!()
        };
        window.offset = 2;
        let validated = validate_match_request(&registry, request).unwrap();
        let solutions = (1..=MAX_PROCESSED_ITEMS + 1)
            .map(|index| {
                let concept_id = format!("0x{index:x}");
                solution(&[(0, &concept_id)], &[])
            })
            .collect::<Vec<_>>();
        let (database, events) = database_without_tuple_proof(
            CapabilitySet::all(),
            vec![Ok(solutions)],
            vec![Ok(vec![
                person_hydration(0, "0x1", "Alice"),
                person_hydration(0, "0x2", "Bob"),
                person_hydration(0, "0x3", "Cara"),
            ])],
        );

        let result = database.execute_match(&registry, &validated).await.unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        let super::super::result::SlotValue::One(person) = &rows[0].slots()[0] else {
            panic!("expected singular slot")
        };
        assert_eq!(person.concept_id().as_str(), "0x3");
        let observed = events.lock().unwrap();
        assert_eq!(observed.solution_answers, 3);
        let statement = &observed.solution_statements[0];
        assert_eq!(statement.offset, 0);
        assert_eq!(statement.limit, 3);
        assert!(!statement.order.is_empty());
        assert!(statement.order.iter().all(|order| {
            statement
                .fields
                .iter()
                .find(|field| field.id == order.field)
                .is_some_and(|field| statement.projection.contains(&field.owner))
        }));
        assert_eq!(observed.solution_limits[0].0, 3);
        assert_eq!(observed.hydration_answers, 3);
        assert_eq!(observed.closes, 1);
    }

    #[tokio::test]
    async fn attribute_budget_keeps_first_error_while_draining_hydration_to_eof() {
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
            person_request(&registry, RowCardinality::BoundedMany, 2),
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
            vec![Ok(vec![
                solution(&[(0, "0x01")], &[]),
                solution(&[(0, "0x02")], &[]),
            ])],
            vec![Ok(vec![
                over_limit,
                person_hydration(0, "0x02", "drained-after-first-error"),
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
        assert_eq!((events.solution_answers, events.hydration_answers), (2, 2));
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
                3,
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
                4,
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
                3,
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
    async fn owned_pending_close_after_success_preserves_released_deadline_error() {
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
        tokio::time::advance(OWNED_TRANSACTION_CLOSE_GRACE).await;
        tokio::task::yield_now().await;
        assert!(!execution.is_finished());
        tokio::time::advance(Duration::from_secs(4)).await;
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "transaction_deadline_exceeded");
        assert_eq!(state.statements.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn timed_out_statement_dispatches_close_without_extending_the_deadline() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, state) = pending_provider_database(PendingProviderPhase::StatementThenClose);
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
        state.entered.notified().await;
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "transaction_deadline_exceeded");
        assert_eq!(state.statements.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 1);

        // The detached close retains a bounded lifetime after the public
        // deadline; advancing its grace must not re-enter provider cleanup.
        tokio::time::advance(OWNED_TRANSACTION_CLOSE_GRACE).await;
        tokio::task::yield_now().await;
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
    async fn borrowed_terminal_exactly_one_skips_redundant_tuple_proof_lock_wait() {
        let registry = Arc::new(person_registry());
        let validated = Arc::new(
            validate_match_request(
                &registry,
                person_request(&registry, RowCardinality::ExactlyOne, 1),
            )
            .unwrap(),
        );
        let (database, events, state) = database_with_proof_capability_lock();
        let context = database.transaction_context(TxType::Read).await.unwrap();
        let execution_context = context.clone();
        let execution_registry = Arc::clone(&registry);
        let execution_validated = Arc::clone(&validated);
        let execution = tokio::spawn(async move {
            execution_context
                .execute_match_with_limits(
                    &execution_registry,
                    &execution_validated,
                    MatchExecutionLimits::tightened(
                        10,
                        4096,
                        Duration::from_secs(5),
                        AnswerCancellation::default(),
                    ),
                )
                .await
        });

        // Queue a caller-owned raw query while hydration still holds the
        // context mutex. Tokio's FIFO mutex grants that waiter the lock next,
        // but the terminal DISTINCT stream has already proved cardinality and
        // selected execution must return without reacquiring the mutex.
        state.hydration_entered.notified().await;
        let blocker_context = context.clone();
        let blocker =
            tokio::spawn(async move { blocker_context.query("match $x isa thing;").await });
        tokio::task::yield_now().await;
        state.resume_hydration.notify_one();
        state.blocker_entered.notified().await;

        let result = execution.await.unwrap().unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);

        blocker.abort();
        let _cancelled = blocker.await;
        assert!(context.supports_exactly_one_tuple_proof().await.unwrap());
        let events = events.lock().unwrap();
        assert_eq!(events.solution_statements.len(), 1);
        assert_eq!(events.hydration_statements.len(), 1);
        assert!(events.tuple_statements.is_empty());
        assert_eq!(events.closes, 0);
    }

    #[tokio::test]
    async fn owned_execution_fences_one_registry_snapshot_across_concurrent_registration() {
        let registry = Arc::new(person_registry());
        let mut request = person_request(&registry, RowCardinality::ExactlyOne, 1);
        request.plan.bindings[0].match_mode = MatchMode::Subtypes;
        let validated = Arc::new(validate_match_request(&registry, request).unwrap());
        let (database, _events, state) = database_with_proof_capability_lock();

        let execution_registry = Arc::clone(&registry);
        let execution_validated = Arc::clone(&validated);
        let execution = tokio::spawn(async move {
            database
                .execute_match(&execution_registry, &execution_validated)
                .await
        });

        // Hydration begins only after preflight, authority construction, and
        // adaptation. A newly registered relevant subtype would make a second
        // live-registry read stale even though the in-flight provider snapshot
        // and result remain valid.
        state.hydration_entered.notified().await;
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
        assert_eq!(
            validated
                .recheck_schema(&registry)
                .unwrap_err()
                .code()
                .as_str(),
            "stale_schema"
        );
        state.resume_hydration.notify_one();

        let result = execution.await.unwrap().unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn borrowed_execution_fences_one_registry_snapshot_across_concurrent_registration() {
        let registry = Arc::new(person_registry());
        let mut request = person_request(&registry, RowCardinality::ExactlyOne, 1);
        request.plan.bindings[0].match_mode = MatchMode::Subtypes;
        let validated = Arc::new(validate_match_request(&registry, request).unwrap());
        let (database, _events, state) = database_with_proof_capability_lock();
        let context = database.transaction_context(TxType::Read).await.unwrap();

        let execution_context = context.clone();
        let execution_registry = Arc::clone(&registry);
        let execution_validated = Arc::clone(&validated);
        let execution = tokio::spawn(async move {
            execution_context
                .execute_match(&execution_registry, &execution_validated)
                .await
        });

        state.hydration_entered.notified().await;
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
        assert_eq!(
            validated
                .recheck_schema(&registry)
                .unwrap_err()
                .code()
                .as_str(),
            "stale_schema"
        );
        state.resume_hydration.notify_one();

        let result = execution.await.unwrap().unwrap();
        let super::super::result::MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
        context.close().await.unwrap();
    }

    #[tokio::test]
    async fn duplicate_heavy_bounded_many_scan_ceiling_never_becomes_short_success() {
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
        let person = BindingId::new(0);
        let company = BindingId::new(1);
        let validated = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![
                        MatchBinding {
                            id: person,
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
                    allowed_cross_joins: BTreeSet::from([BindingPair::new(person, company)]),
                },
                MatchOperation::FetchRows {
                    output: FetchShape::Positional {
                        slots: vec![FetchSlot::One { binding: person }],
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
        let (database, events) = database(
            CapabilitySet::all(),
            vec![Ok(vec![
                solution(&[(0, "0x01"), (1, "0x10")], &[]),
                solution(&[(0, "0x01"), (1, "0x11")], &[]),
                solution(&[(0, "0x01"), (1, "0x12")], &[]),
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
        assert!(events.tuple_statements.is_empty());
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
