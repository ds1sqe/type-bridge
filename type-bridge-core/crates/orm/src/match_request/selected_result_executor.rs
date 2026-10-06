//! End-to-end selected-row execution over bounded typed provider statements.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::task::Poll;
use std::time::{Duration, Instant};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticDetailValue};
use type_bridge_contract::query_plan::QueryInvocation;
use type_bridge_contract::query_remote_v2::RemoteReplyDecodeLimitsV2;

use super::capability::{Capability, CapabilitySet};
use super::error::{MatchError, MatchErrorCategory, MatchErrorPath, MatchErrorPathSegment};
use super::ids::{DescriptorId, FieldId};
use super::provider_order::check_provider_orders;
use super::result::ValidatedMatchResult;
use super::result_validation::{
    MAX_COLLECTED_CONCEPTS, MAX_HYDRATED_ATTRIBUTE_VALUES, MAX_HYDRATED_THINGS,
    exactly_one_cardinality_error, validated_match_result_from_v2,
};
use super::validation::ValidatedMatchRequest;
#[cfg(test)]
use super::{ReducedValue, Reduction};
use crate::_registry::DescriptorRegistry;
use crate::error::OrmError;
use crate::query_execution_limits::QueryExecutionDeadline;
use crate::query_v2::{
    QueryV2ExecutionError, execute_validated_model_query_borrowed,
    execute_validated_model_query_with_statement_limit,
};
use crate::query_v2_adapter::{
    AdaptedMatchRequest, MatchRequestAdapterAuthority, adapt_match_request,
};
use crate::session::backend::{
    AnswerCancellation, BoundedAnswerLimits, QueryV2AnswerLimits, TxType,
};
use crate::session::{Database, Transaction, TransactionContext};

const MAX_PROCESSED_ITEMS: u64 = 100_000;
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSACTION_DURATION: Duration = Duration::from_secs(30);
const OWNED_TRANSACTION_CLOSE_GRACE: Duration = Duration::from_secs(1);
const MAX_STATEMENTS: u8 = 3;

/// Caller policy clamped to the selected executor's hard ceilings.
#[derive(Debug, Clone)]
pub struct MatchExecutionLimits {
    max_items: u64,
    max_bytes: u64,
    timeout: Duration,
    absolute_deadline: Option<QueryExecutionDeadline>,
    cancellation: AnswerCancellation,
    max_hydrated_things: u64,
    max_attribute_values: u64,
    max_collected_concepts: u64,
    max_role_players: u64,
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
            absolute_deadline: None,
            cancellation,
            max_hydrated_things: u64::try_from(MAX_HYDRATED_THINGS).unwrap_or(u64::MAX),
            max_attribute_values: u64::try_from(MAX_HYDRATED_ATTRIBUTE_VALUES).unwrap_or(u64::MAX),
            max_collected_concepts: u64::try_from(MAX_COLLECTED_CONCEPTS).unwrap_or(u64::MAX),
            max_role_players: u64::try_from(MAX_HYDRATED_THINGS).unwrap_or(u64::MAX),
            max_statements: MAX_STATEMENTS,
        }
    }

    /// Bind execution to one deadline captured before semantic construction.
    #[doc(hidden)]
    #[must_use]
    pub fn with_absolute_deadline(mut self, deadline: QueryExecutionDeadline) -> Self {
        self.absolute_deadline = Some(deadline);
        self
    }

    fn deadline(&self) -> Option<Instant> {
        self.absolute_deadline
            .map(QueryExecutionDeadline::instant)
            .or_else(|| {
                tokio::time::Instant::now()
                    .checked_add(self.timeout)
                    .map(tokio::time::Instant::into_std)
            })
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

    /// Further tighten the hydrated relation-role-player reference ceiling.
    pub fn with_max_role_players(mut self, max_role_players: u64) -> Self {
        self.max_role_players = max_role_players.min(self.max_role_players);
        self
    }

    /// Further tighten the number of provider statements in one execution.
    pub fn with_max_statements(mut self, max_statements: u8) -> Self {
        self.max_statements = max_statements.min(self.max_statements);
        self
    }

    #[cfg(test)]
    pub(crate) const fn resource_dimensions(&self) -> (u64, u64, u64, u64, u8) {
        (
            self.max_hydrated_things,
            self.max_attribute_values,
            self.max_collected_concepts,
            self.max_role_players,
            self.max_statements,
        )
    }
}

/// The deadline and cancellation one invocation's transaction lifecycle
/// runs under.
struct ExecutionBudget {
    deadline: Option<Instant>,
    cancellation: AnswerCancellation,
}

impl ExecutionBudget {
    fn new(limits: &MatchExecutionLimits, deadline: Option<Instant>) -> Self {
        Self {
            deadline,
            cancellation: limits.cancellation.clone(),
        }
    }

    fn check_before_await(&self) -> Result<(), OrmError> {
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
                result = &mut future => result,
                () = &mut cancellation => Err(cancelled_error(
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

async fn catch_execution_unwind<F>(future: F) -> std::thread::Result<F::Output>
where
    F: Future,
{
    let mut future = Box::pin(future);
    std::future::poll_fn(|context| {
        match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(context))) {
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        }
    })
    .await
}

async fn close_after_execution_panic(transaction: Transaction) {
    match catch_execution_unwind(dispatch_failed_execution_close(transaction)).await {
        Ok(Some(Err(error))) => {
            tracing::warn!(%error, "owned match transaction cleanup failed after panic")
        }
        Err(_) => tracing::warn!("owned match transaction cleanup panicked after execution panic"),
        Ok(Some(Ok(())) | None) => {}
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

    /// Execute one validated request in an owned read transaction.
    ///
    /// The request is adapted to a V2 plan under the registry's schema
    /// authority and run on the V2 model executor: the package's embedded
    /// authority for an installed projection, one rebuilt from descriptors
    /// for a dynamic registry.
    pub(crate) async fn execute_compatible_owned(
        &self,
        database: &Database,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        self.check_lifecycle_budget()?;
        let registry = self.registry.owned_registry_snapshot()?;
        self.check_lifecycle_budget()?;
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
        self.check_lifecycle_budget()?;
        validated.recheck_schema(self.registry)?;
        if validated
            .capabilities()
            .contains(Capability::SchemaFunctionCall)
            && (!self
                .available_capabilities
                .contains(Capability::SchemaFunctionCall)
                || !database.supports_given_stage())
        {
            return Err(super::capability::schema_function_given_rows_unsupported().into());
        }
        self.preflight_compatibility(validated)?;
        self.check_lifecycle_budget()?;
        self.require_schema_authority()?;
        let authority = MatchRequestAdapterAuthority::from_registry(self.registry)
            .map_err(adapter_diagnostic)?;
        self.check_lifecycle_budget()?;
        let adapted = adapt_match_request(validated, self.registry, &authority.context())
            .map_err(adapter_diagnostic)?;
        self.execute_adapted_owned(database, validated, &adapted)
            .await
    }

    /// Borrowed counterpart of [`Self::execute_compatible_owned`].
    pub(crate) async fn execute_compatible_borrowed(
        &self,
        context: &TransactionContext,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        self.check_lifecycle_budget()?;
        let registry = self.registry.owned_registry_snapshot()?;
        self.check_lifecycle_budget()?;
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
        self.check_lifecycle_budget()?;
        validated.recheck_schema(self.registry)?;
        if validated
            .capabilities()
            .contains(Capability::SchemaFunctionCall)
            && (!self
                .available_capabilities
                .contains(Capability::SchemaFunctionCall)
                || !context.supports_given_stage().await?)
        {
            return Err(super::capability::schema_function_given_rows_unsupported().into());
        }
        self.preflight_compatibility(validated)?;
        self.check_lifecycle_budget()?;
        if context.tx_type() != TxType::Read {
            return Err(MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "borrowed_target_not_read_only",
                "selected-row execution requires a borrowed read transaction",
            )
            .at(MatchErrorPathSegment::Operation)
            .into());
        }
        self.require_schema_authority()?;
        let authority = MatchRequestAdapterAuthority::from_registry(self.registry)
            .map_err(adapter_diagnostic)?;
        self.check_lifecycle_budget()?;
        let adapted = adapt_match_request(validated, self.registry, &authority.context())
            .map_err(adapter_diagnostic)?;
        self.execute_adapted_borrowed(context, validated, &adapted)
            .await
    }

    fn preflight_compatibility(&self, validated: &ValidatedMatchRequest) -> Result<(), OrmError> {
        validated.recheck_schema(self.registry)?;
        validated.require_capabilities(&self.available_capabilities)?;
        check_provider_orders(self.registry, validated)?;
        Ok(())
    }

    fn check_lifecycle_budget(&self) -> Result<(), OrmError> {
        if self.limits.cancellation.is_cancelled() {
            return Err(cancelled_error(
                "provider_cancelled",
                "provider answer processing was cancelled",
            ));
        }
        if self
            .limits
            .absolute_deadline
            .is_some_and(QueryExecutionDeadline::is_expired)
        {
            return Err(resource_error(
                "transaction_deadline_exceeded",
                "provider transaction deadline expired",
            ));
        }
        if self.limits.max_statements == 0 {
            return Err(resource_error(
                "statement_count_limit",
                "match execution requires at least one provider statement",
            ));
        }
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

    /// Refuse a generated package that was installed without its schema
    /// authority.
    ///
    /// Adaptation needs the package's resolved schema. One rebuilt from
    /// descriptors loses schema functions and ordered collections, so such
    /// a package is refused instead of run on a guess.
    fn require_schema_authority(&self) -> Result<(), OrmError> {
        if self.registry.is_installed_projection()
            && self.registry.installed_schema_authority().is_none()
        {
            return Err(MatchError::new(
                MatchErrorCategory::InvalidPlan,
                "schema_authority_required",
                "this generated package carries no schema authority; regenerate it with the current generator",
            )
            .at(MatchErrorPathSegment::Plan)
            .into());
        }
        Ok(())
    }

    async fn execute_adapted_owned(
        &self,
        database: &Database,
        released: &ValidatedMatchRequest,
        adapted: &AdaptedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let deadline = self.limits.deadline();
        let lifecycle = ExecutionBudget::new(&self.limits, deadline);
        let mut transaction = lifecycle
            .await_provider(async {
                database
                    .read_transaction()
                    .await
                    .map_err(provider_transaction_open_error)
            })
            .await?;
        let execution = catch_execution_unwind(self.execute_adapted_transaction(
            &mut transaction,
            released,
            adapted,
            deadline,
        ))
        .await;
        let result = match execution {
            Err(payload) => {
                close_after_execution_panic(transaction).await;
                resume_unwind(payload);
            }
            Ok(Err(error)) => {
                if let Some(Err(close_error)) = dispatch_failed_execution_close(transaction).await {
                    tracing::warn!(
                        %close_error,
                        execution_error = %error,
                        "owned adapted match execution failed and transaction cleanup also failed"
                    );
                }
                return Err(error);
            }
            Ok(Ok(result)) => result,
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
        let invocation = QueryInvocation::new(
            adapted.validated().plan(),
            adapted.operation(),
            adapted.inputs().to_vec(),
        )
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
        let deadline = self.limits.deadline();
        let invocation = QueryInvocation::new(
            adapted.validated().plan(),
            adapted.operation(),
            adapted.inputs().to_vec(),
        )
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
                max_role_players: self.limits.max_role_players,
            },
        )
    }
}

fn resource_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::ProviderEvidence)
        .into()
}

fn cancelled_error(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::Cancelled, code, message)
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
        DiagnosticCategory::Cancelled => MatchErrorCategory::Cancelled,
        DiagnosticCategory::Integrity => MatchErrorCategory::ResultDecode,
    };
    let mut error = MatchError::new(category, diagnostic.code().as_str(), diagnostic.message())
        .at(MatchErrorPathSegment::Operation);
    // Keep what a refusal says about its limit: which one, its value, and
    // where that value came from.
    for (key, value) in diagnostic.details() {
        error = match value {
            DiagnosticDetailValue::Text(value) => error.with_detail(key.clone(), value.clone()),
            DiagnosticDetailValue::Long(value) => match u64::try_from(*value) {
                Ok(unsigned) => error.with_detail(key.clone(), unsigned),
                Err(_) => {
                    error.with_detail(key.clone(), crate::MatchErrorDetailValue::Signed(*value))
                }
            },
            DiagnosticDetailValue::Boolean(_) | DiagnosticDetailValue::TextList(_) => error,
        };
    }
    error.into()
}

fn compatibility_execution_error(error: QueryV2ExecutionError) -> OrmError {
    match error {
        QueryV2ExecutionError::Provider(error) => provider_statement_error(error),
        QueryV2ExecutionError::Validation(diagnostic) => {
            released_model_execution_error(&diagnostic).into()
        }
    }
}

/// Restore the released match-error surface for one internal V2 model executor diagnostic.
///
/// Direct execution and authenticated remote execution must both cross this
/// compatibility map before a language binding observes the failure.
pub(crate) fn released_model_execution_error(diagnostic: &Diagnostic) -> MatchError {
    if diagnostic.code().as_str() == "query_v2_model_exactly_one" {
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
        return exactly_one_cardinality_error(actual).unwrap_or_else(|| {
            MatchError::new(
                MatchErrorCategory::ResultDecode,
                "result_operation_mismatch",
                "adapted exactly-one result lost its cardinality proof",
            )
            .at(MatchErrorPathSegment::Result)
        });
    }

    let (category, code, mapped_message) = released_execution_diagnostic(diagnostic);
    let path = released_execution_path(diagnostic);
    let message = match (category, code, path.segments()) {
        (MatchErrorCategory::ResultDecode, _, [MatchErrorPathSegment::ProviderEvidence]) => {
            "provider evidence failed canonical typed match decoding"
        }
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
    MatchError::new(category, code, message).with_path(path)
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
            MatchErrorCategory::Cancelled,
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
            "hydrated_role_player_limit",
            "provider result exceeded the hydrated role-player ceiling",
        ),
        "query_v2_model_solution_limit" => (
            MatchErrorCategory::ResourceLimit,
            "solution_scan_limit",
            "provider solution ceiling was reached before result completeness was proven",
        ),
        "query_v2_model_complete_rows_exceeded" => (
            MatchErrorCategory::ResourceLimit,
            "row_window_exceeded",
            "provider returned more distinct rows than the validated window permits",
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
        "query_v2_model_reduction_descriptor" | "query_v2_model_reduction_group" => (
            MatchErrorCategory::ResultDecode,
            "malformed_reduction_row",
            "provider reduction evidence references an unexpected concrete descriptor",
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
        "hydrated_role_player_limit",
        "hydrated_thing_limit",
        "processed_item_counter_overflow",
        "processed_item_limit",
        "response_byte_limit",
        "transaction_deadline_exceeded",
    ];
    const CANCELLED_CODES: &[&str] = &["provider_cancelled"];
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
        MatchErrorCategory::Cancelled if CANCELLED_CODES.contains(&code) => (
            MatchErrorCategory::Cancelled,
            "provider cancellation prevented complete typed match evidence",
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::{Arc, Mutex};

    use std::collections::BTreeSet;

    use serde_json::Value;

    use type_bridge_core_lib::ast::{
        TypedFetchRows, TypedHydrateThings, TypedPageRematch, TypedRootScan, TypedThingKind,
    };

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{
        EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
    };

    use crate::_entity::Annotation;
    use crate::MatchErrorPath;
    use crate::match_request::ids::BoundFieldId;
    use crate::match_request::ids::{BindingId, RoleEdgeId};
    use crate::match_request::model::{
        BindingPair, MatchBinding, MatchExpr, MatchMode, MatchOrder, MatchPlan, MatchRequest,
        MissingOrder, ReduceTerm, SortDirection, Window,
    };
    use crate::match_request::model::{
        FetchShape, FetchSlot, MatchOperation, RowCardinality, ThingKind,
    };

    use crate::match_request::validation::validate_match_request;
    use crate::session::backend::{
        AnswerConsumer, AnswerControl, AnswerItem, MAX_ERROR_DRAIN_ITEMS,
    };
    use crate::session::backend::{
        BoundedAnswerReader, BoundedAnswerStats, BoxFuture, DriverBackend, QueryResult,
        TransactionOps,
    };
    use crate::value::AttributeValue;
    use tokio::sync::Notify;

    type RecordedAnswers = Arc<Mutex<VecDeque<Result<Vec<AnswerItem>, String>>>>;

    #[test]
    fn executor_and_lowerer_source_contain_no_raw_or_dynamic_escape_hatch() {
        let executor = include_str!("selected_result_executor.rs");
        let lowerer = include_str!("../query_v2_compatibility.rs");
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
        use crate::match_request::result_validation::canonicalize_provider_attribute_value;

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
    }

    #[test]
    fn execution_limit_builders_only_tighten_every_semantic_and_statement_ceiling() {
        let limits = MatchExecutionLimits::tightened(
            7,
            4096,
            Duration::from_secs(1),
            AnswerCancellation::default(),
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
            .expect_err("owned execution refuses the order before provider work");
        assert_released_error(&owned);
        assert_eq!(events.lock().unwrap().opens, 0);

        let context = database.transaction_context(TxType::Read).await.unwrap();
        let borrowed = context
            .execute_match(&registry, &validated)
            .await
            .expect_err("borrowed execution refuses the order before provider work");
        assert_released_error(&borrowed);
        {
            let events = events.lock().unwrap();
            assert_eq!(events.opens, 1);
            assert!(events.solution_statements.is_empty());
            assert!(events.hydration_statements.is_empty());
        }
        context.close().await.unwrap();
    }

    /// A generated package's projection, with or without the verified schema
    /// authority its package embeds.
    fn installed_person_registry(with_authority: bool) -> Arc<DescriptorRegistry> {
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::managed_scope::ManagedScopeId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{
            ManagedDeltaContext, SchemaDocumentSet, build_schema_authority, normalize_documents,
            project, resolve, schema_authority_capability_vocabulary,
        };

        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("direct-authority.yaml").expect("document"),
            "format: typebridge.schema/v2\nattributes:\n  score: { value: integer }\n  rank: { value: integer }\nentities:\n  person:\n    owns:\n      score: { key: true }\n      rank: { card: 1 }\n",
        )])
        .expect("schema documents");
        let declared = normalize_documents(&documents).expect("normalized schema");
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("profile");
        let resolved = resolve(&declared, &profile).expect("resolved schema");
        let mut installed = crate::InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v1()],
                &[],
            )
            .expect("runtime projection"),
        )
        .expect("installed projection");
        if with_authority {
            let context = ManagedDeltaContext::new(
                ManagedScopeId::new("direct-authority").expect("scope"),
                profile,
                schema_authority_capability_vocabulary(),
            );
            let authority =
                build_schema_authority(&declared, declared.required_capabilities(), &context)
                    .expect("verified schema authority");
            installed = installed.with_schema_authority(&authority);
        }
        Arc::new(installed.match_registry().expect("match registry"))
    }

    /// One terminal over persons under a predicate really nested thirty-eight
    /// levels deep, which the pipeline refuses at the canonical encoding
    /// depth while adapting, before any provider work.
    fn deeply_nested(
        registry: &Arc<DescriptorRegistry>,
        terminal: impl FnOnce(
            &crate::match_request::QueryHandle,
            &crate::match_request::BindingHandle,
        ) -> crate::error::Result<ValidatedMatchRequest>,
    ) -> ValidatedMatchRequest {
        let session = crate::match_request::SessionHandle::new(Arc::clone(registry));
        let person = session.exact("person").expect("person");
        let term = |minimum: i64| {
            person.field("score").expect("score").compare_value(
                crate::ComparisonOp::GreaterThanOrEqual,
                crate::AttributeValue::Long(minimum),
            )
        };
        let mut predicate = term(0);
        for depth in 1..38 {
            predicate = if depth % 2 == 0 {
                predicate.and(&term(depth)).expect("and")
            } else {
                predicate.or(&term(depth)).expect("or")
            };
        }
        let query = session
            .query(session.positional([person.one()]).expect("shape"))
            .expect("query")
            .where_predicate(predicate)
            .expect("where");
        terminal(&query, &person).expect("valid request")
    }

    fn deeply_nested_count(registry: &Arc<DescriptorRegistry>) -> ValidatedMatchRequest {
        deeply_nested(registry, |query, person| query.validate_count_by(person))
    }

    #[tokio::test]
    async fn a_generated_package_with_its_authority_executes_directly_on_the_shared_pipeline() {
        let registry = installed_person_registry(true);
        let validated = deeply_nested_count(&registry);
        let (database, events) = database(CapabilitySet::all(), Vec::new(), Vec::new());

        let error = database
            .execute_match(&registry, &validated)
            .await
            .expect_err("the shared pipeline refuses a plan past the encoding depth");
        let error = match_error(&error);
        assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), "canonical_json_too_deep");
        assert_eq!(
            error.details().get("limit"),
            Some(&crate::MatchErrorDetailValue::Text(
                "canonical_depth".into()
            ))
        );
        // Refused during adaptation, before any provider work: the same
        // outcome a proxy gives for this query.
        assert_eq!(events.lock().unwrap().opens, 0);
    }

    /// Every released terminal of an authority-backed package takes the
    /// shared pipeline, in an owned and in a borrowed transaction.
    #[tokio::test]
    async fn every_terminal_of_an_authority_backed_package_takes_the_shared_pipeline() {
        use crate::match_request::model::Reduction;

        let registry = installed_person_registry(true);
        let window = Window {
            offset: 0,
            limit: 10,
        };
        let terminals: Vec<(&str, ValidatedMatchRequest)> = vec![
            (
                "rows",
                deeply_nested(&registry, |query, _| {
                    query.validate_fetch_rows(&[], window, RowCardinality::BoundedMany)
                }),
            ),
            (
                "exactly one",
                deeply_nested(&registry, |query, _| {
                    query.validate_fetch_rows(
                        &[],
                        Window {
                            offset: 0,
                            limit: 1,
                        },
                        RowCardinality::ExactlyOne,
                    )
                }),
            ),
            (
                "complete rows",
                deeply_nested(&registry, |query, _| {
                    query.validate_fetch_rows(&[], window, RowCardinality::Complete)
                }),
            ),
            (
                "page",
                deeply_nested(&registry, |query, person| {
                    query.validate_page_by(person, &[], window, true)
                }),
            ),
            ("count", deeply_nested_count(&registry)),
            (
                "exists",
                deeply_nested(&registry, |query, person| query.validate_exists_by(person)),
            ),
            (
                "reduce",
                deeply_nested(&registry, |query, person| {
                    query.validate_reduce_by(person, None, &[(Reduction::Count, None)])
                }),
            ),
            (
                "reduce by field",
                deeply_nested(&registry, |query, person| {
                    let score = person.field("score").expect("score");
                    query.validate_reduce_by_field(person, &score, &[(Reduction::Count, None)])
                }),
            ),
            (
                "reduce by fields",
                deeply_nested(&registry, |query, person| {
                    let score = person.field("score").expect("score");
                    let rank = person.field("rank").expect("rank");
                    query.validate_reduce_by_fields(
                        person,
                        &[&score, &rank],
                        &[(Reduction::Count, None)],
                    )
                }),
            ),
        ];

        for (terminal, validated) in &terminals {
            let (database, events) = database(CapabilitySet::all(), Vec::new(), Vec::new());
            let owned = database
                .execute_match(&registry, validated)
                .await
                .expect_err("the shared pipeline refuses a plan past the encoding depth");
            assert_eq!(
                match_error(&owned).code().as_str(),
                "canonical_json_too_deep",
                "{terminal}, owned"
            );
            assert_eq!(events.lock().unwrap().opens, 0, "{terminal}, owned");

            let context = database.transaction_context(TxType::Read).await.unwrap();
            let borrowed = context
                .execute_match(&registry, validated)
                .await
                .expect_err("the shared pipeline refuses a plan past the encoding depth");
            assert_eq!(
                match_error(&borrowed).code().as_str(),
                "canonical_json_too_deep",
                "{terminal}, borrowed"
            );
            {
                let events = events.lock().unwrap();
                assert!(
                    events.solution_statements.is_empty(),
                    "{terminal}, borrowed"
                );
                assert!(
                    events.hydration_statements.is_empty(),
                    "{terminal}, borrowed"
                );
            }
            context.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_generated_package_without_its_authority_is_refused_before_provider_work() {
        let registry = installed_person_registry(false);
        let validated = deeply_nested_count(&registry);
        let (database, events) = database(CapabilitySet::all(), Vec::new(), Vec::new());

        let error = database
            .execute_match(&registry, &validated)
            .await
            .expect_err("there is no schema context to adapt under");
        let error = match_error(&error);
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(error.code().as_str(), "schema_authority_required");
        assert_eq!(events.lock().unwrap().opens, 0);
    }

    #[test]
    fn an_adaptation_refusal_keeps_its_limit_details_on_the_direct_path() {
        let refusal =
            type_bridge_contract::limits::QueryLimits::CANONICAL.annotate(Diagnostic::new(
                DiagnosticCategory::ResourceLimit,
                type_bridge_contract::diagnostic::DiagnosticCode::new("canonical_json_too_deep")
                    .expect("static code"),
                "canonical JSON exceeds the nesting-depth ceiling",
            ));
        let OrmError::Match(error) = adapter_diagnostic(refusal) else {
            panic!("an adaptation refusal is a match error");
        };
        assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(error.code().as_str(), "canonical_json_too_deep");
        assert_eq!(
            error.details().get("limit"),
            Some(&crate::MatchErrorDetailValue::Text(
                "canonical_depth".into()
            ))
        );
        assert_eq!(
            error.details().get("maximum"),
            Some(&crate::MatchErrorDetailValue::Unsigned(64))
        );
        assert_eq!(
            error.details().get("source"),
            Some(&crate::MatchErrorDetailValue::Text("default".into()))
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
            let answer_limit = query.targets.iter().try_fold(0_u64, |total, target| {
                total
                    .checked_add(u64::try_from(target.concept_ids.len()).unwrap_or(u64::MAX))
                    .ok_or_else(|| OrmError::QueryExecution("hydration batch overflow".into()))
            });
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
        StatementPanic,
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
                    PendingProviderPhase::Statement
                        | PendingProviderPhase::StatementPanic
                        | PendingProviderPhase::StatementThenClose
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
                if state.phase == PendingProviderPhase::StatementPanic {
                    state.entered.notify_one();
                    panic!("provider panic after owned transaction open");
                }
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

        let role_players = crate::query_v2::failure(
            DiagnosticCategory::ResourceLimit,
            "query_v2_model_role_player_limit",
            "mapping probe",
        );
        assert_eq!(
            released_execution_diagnostic(&role_players),
            (
                MatchErrorCategory::ResourceLimit,
                "hydrated_role_player_limit",
                "provider result exceeded the hydrated role-player ceiling",
            )
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
            assert_eq!(observed.opens, 0);
            assert!(observed.solution_statements.is_empty());
            assert!(observed.tuple_statements.is_empty());
            assert_eq!(observed.closes, 0);
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
        // The byte ceiling of the tuple proof is the executor's own; read it
        // from an accepted run instead of recomputing it here.
        let (accepted, accepted_events) = database_with_tuple_response(
            CapabilitySet::all(),
            vec![Ok(vec![solution(&[(0, "0x01"), (1, "0x10")], &[])])],
            vec![Ok(vec![
                person_hydration(0, "0x01", "Alice"),
                person_hydration(1, "0x10", "Witness"),
            ])],
            Ok(vec![solution(&[(0, "0x01")], &[])]),
        );
        accepted.execute_match(&registry, &validated).await.unwrap();
        let [(2, proof_limit)] = accepted_events.lock().unwrap().tuple_limits[..] else {
            panic!("expected one two-row tuple proof")
        };
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

    #[tokio::test]
    async fn owned_cancellation_interrupts_pending_transaction_open() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, state) = pending_provider_database(PendingProviderPhase::Open);
        let cancellation = AnswerCancellation::default();
        let trigger = cancellation.clone();
        let execution = tokio::spawn(async move {
            database
                .execute_match_with_limits(
                    &registry,
                    &validated,
                    MatchExecutionLimits::tightened(10, 4096, Duration::from_secs(5), cancellation),
                )
                .await
        });

        state.entered.notified().await;
        trigger.cancel();
        tokio::task::yield_now().await;
        assert!(execution.is_finished());
        let error = execution.await.unwrap().unwrap_err();
        assert_eq!(match_code(&error), "provider_cancelled");
        assert_eq!(
            match_error(&error).category(),
            MatchErrorCategory::Cancelled
        );
        assert_eq!(state.opens.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 0);
    }

    #[tokio::test]
    async fn owned_compatible_execution_closes_once_then_resumes_provider_panic() {
        let registry = person_registry();
        let validated = validate_match_request(
            &registry,
            person_root_request(&registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap();
        let (database, state) = pending_provider_database(PendingProviderPhase::StatementPanic);
        let execution = tokio::spawn(async move {
            database
                .execute_match_with_limits(&registry, &validated, MatchExecutionLimits::default())
                .await
        });

        state.entered.notified().await;
        let panic = execution.await.expect_err("execution panic must resume");
        assert!(panic.is_panic());
        assert_eq!(state.statements.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(state.closes.load(AtomicOrdering::SeqCst), 1);
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
