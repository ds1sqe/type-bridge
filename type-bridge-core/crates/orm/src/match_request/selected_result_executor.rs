//! End-to-end selected-row execution over bounded typed provider statements.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::task::Poll;
use std::time::{Duration, Instant};

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticDetailValue};

use super::capability::{Capability, CapabilitySet};
use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
#[cfg(test)]
use super::ids::{DescriptorId, FieldId};
use super::plan_adapter::{AdaptedMatchRequest, adapt_match_request};
#[cfg(test)]
use super::plan_diagnostic::released_execution_diagnostic;
pub(crate) use super::plan_diagnostic::released_plan_execution_error;
use super::provider_order::check_provider_orders;
use super::result::ValidatedMatchResult;
use super::result_validation::validated_match_result_from_plan_stack;
use super::validation::ValidatedMatchRequest;
use crate::_registry::DescriptorRegistry;
use crate::error::OrmError;
use crate::query_execution_limits::{
    AnswerTransport, QueryExecutionDeadline, QueryExecutionResourceLimits,
};
#[cfg(test)]
use crate::query_v2::QueryV2ExecutionError;
use crate::session::backend::{AnswerCancellation, BoundedAnswerLimits, TxType};
use crate::session::{Database, OwnedTransaction as Transaction, TransactionContext};

const OWNED_TRANSACTION_CLOSE_GRACE: Duration = Duration::from_secs(1);

/// One invocation's answer policy with the deadline and cancellation it
/// runs under.
///
/// The eight budget dimensions are [`QueryExecutionResourceLimits`]: the
/// executor has no ceilings of its own, so a direct execution and a proxy
/// execution of the same request run under the same values.
#[derive(Debug, Clone)]
pub struct MatchExecutionLimits {
    policy: QueryExecutionResourceLimits,
    absolute_deadline: Option<QueryExecutionDeadline>,
    cancellation: AnswerCancellation,
}

impl MatchExecutionLimits {
    /// Run under one answer policy.
    #[must_use]
    pub fn from_policy(
        policy: QueryExecutionResourceLimits,
        cancellation: AnswerCancellation,
    ) -> Self {
        Self {
            policy: policy.effective(),
            absolute_deadline: None,
            cancellation,
        }
    }

    /// Run under the default answer policy with tighter items, bytes, and
    /// timeout.
    pub fn tightened(
        max_items: u64,
        max_bytes: u64,
        timeout: Duration,
        cancellation: AnswerCancellation,
    ) -> Self {
        let default = QueryExecutionResourceLimits::default();
        Self::from_policy(
            QueryExecutionResourceLimits {
                statements: None,
                ..QueryExecutionResourceLimits::tightened(
                    u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                    max_items,
                    max_bytes,
                    default.graph_nodes,
                    default.attribute_values,
                    default.collection_members,
                    default.role_players,
                    default.statement_limit(),
                )
            },
            cancellation,
        )
    }

    /// Bind execution to one deadline captured before semantic construction.
    #[doc(hidden)]
    #[must_use]
    pub fn with_absolute_deadline(mut self, deadline: QueryExecutionDeadline) -> Self {
        self.absolute_deadline = Some(deadline);
        self
    }

    /// Tighten these limits by an enclosing transaction's bounded answer
    /// limits: the smaller row and byte caps, and the earlier deadline.
    /// The enclosing cancellation is raced by that transaction's owner.
    pub(crate) fn bounded_by(mut self, enclosing: &BoundedAnswerLimits) -> Self {
        self.policy.items = self.policy.items.min(enclosing.max_items);
        self.policy.bytes = self.policy.bytes.min(enclosing.max_bytes);
        let deadline = match (self.deadline(), enclosing.deadline) {
            (Some(own), Some(enclosing)) => Some(own.min(enclosing)),
            (own, enclosing) => own.or(enclosing),
        };
        self.absolute_deadline = deadline.map(QueryExecutionDeadline::from_instant);
        self
    }

    pub(crate) fn capture_deadline(&mut self, deadline: Instant) {
        self.absolute_deadline = Some(QueryExecutionDeadline::from_instant(deadline));
    }

    pub(crate) fn cancellation(&self) -> &AnswerCancellation {
        &self.cancellation
    }

    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.absolute_deadline
            .map(QueryExecutionDeadline::instant)
            .or_else(|| {
                tokio::time::Instant::now()
                    .checked_add(Duration::from_millis(self.policy.timeout_milliseconds))
                    .map(tokio::time::Instant::into_std)
            })
    }

    pub(crate) const fn policy(&self) -> QueryExecutionResourceLimits {
        self.policy
    }

    /// Further tighten the hydrated-thing ceiling, including nested role players.
    pub fn with_max_hydrated_things(mut self, max_hydrated_things: u64) -> Self {
        self.policy.graph_nodes = max_hydrated_things.min(self.policy.graph_nodes);
        self
    }

    /// Further tighten the hydrated attribute-value ceiling.
    pub fn with_max_attribute_values(mut self, max_attribute_values: u64) -> Self {
        self.policy.attribute_values = max_attribute_values.min(self.policy.attribute_values);
        self
    }

    /// Further tighten the page collection-concept ceiling.
    pub fn with_max_collected_concepts(mut self, max_collected_concepts: u64) -> Self {
        self.policy.collection_members = max_collected_concepts.min(self.policy.collection_members);
        self
    }

    /// Further tighten the hydrated relation-role-player reference ceiling.
    pub fn with_max_role_players(mut self, max_role_players: u64) -> Self {
        self.policy.role_players = max_role_players.min(self.policy.role_players);
        self
    }

    /// Further tighten the number of provider statements in one execution.
    pub fn with_max_statements(mut self, max_statements: u8) -> Self {
        self.policy.statements = Some(u32::from(max_statements).min(self.policy.statement_limit()));
        self
    }

    /// Name the ceiling behind an answer-limit refusal: which dimension, its
    /// value, and whether that value was the default or configured.
    fn annotate_refusal(&self, error: OrmError) -> OrmError {
        let OrmError::Match(error) = error else {
            return error;
        };
        if error.details().contains_key("limit") {
            return error.into();
        }
        match self
            .policy
            .refusal_details(AnswerTransport::Direct, error.code().as_str())
        {
            Some((limit, maximum, source)) => error
                .with_detail("limit", limit)
                .with_detail("maximum", maximum)
                .with_detail("source", source)
                .into(),
            None => error.into(),
        }
    }

    #[cfg(test)]
    pub(crate) const fn resource_dimensions(&self) -> (u64, u64, u64, u64, u8) {
        (
            self.policy.graph_nodes,
            self.policy.attribute_values,
            self.policy.collection_members,
            self.policy.role_players,
            self.policy.statement_limit() as u8,
        )
    }
}

impl Default for MatchExecutionLimits {
    fn default() -> Self {
        Self::from_policy(
            QueryExecutionResourceLimits::default(),
            AnswerCancellation::default(),
        )
    }
}

/// The deadline and cancellation one invocation's transaction lifecycle
/// runs under.
pub(crate) struct ExecutionBudget {
    pub(crate) deadline: Option<Instant>,
    pub(crate) cancellation: AnswerCancellation,
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

    pub(crate) async fn await_provider<T>(
        &self,
        future: impl Future<Output = Result<T, OrmError>>,
    ) -> Result<T, OrmError> {
        self.check_before_await()?;
        let result = self.race_provider(future).await;
        self.check_before_await()?;
        result
    }

    pub(crate) async fn await_cleanup<T>(
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

pub(crate) async fn dispatch_failed_execution_close(
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

pub(crate) async fn catch_execution_unwind<F>(future: F) -> std::thread::Result<F::Output>
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

pub(crate) async fn close_after_execution_panic(transaction: Transaction) {
    match catch_execution_unwind(dispatch_failed_execution_close(transaction)).await {
        Ok(Some(Err(error))) => {
            tracing::warn!(%error, "owned match transaction cleanup failed after panic")
        }
        Err(_) => tracing::warn!("owned match transaction cleanup panicked after execution panic"),
        Ok(Some(Ok(())) | None) => {}
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
    /// Adapt under the registry's schema authority and execute the complete
    /// native plan stack on one read-only provider snapshot.
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
            .map_err(|error| self.limits.annotate_refusal(error))
    }

    async fn execute_compatible_owned_fenced(
        &self,
        database: &Database,
        validated: &ValidatedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let adapted = self.adapt_owned(validated, database.supports_given_stage())?;
        self.execute_adapted_owned(database, validated, &adapted)
            .await
    }

    /// Execute one validated request as a staged program on the commit's
    /// provider transaction.
    ///
    /// The registry is the snapshot the program took when it was staged.
    /// Any transaction kind but `Schema` (refused at staging) is allowed:
    /// commit is the only executor, so nothing interleaves the program's
    /// statements and its answer is published only after the commit.
    pub(crate) async fn execute_compatible_staged(
        &self,
        transaction: &mut Transaction,
        validated: &ValidatedMatchRequest,
        supports_given_stage: bool,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let result = async {
            let adapted = self.adapt_owned(validated, supports_given_stage)?;
            self.execute_adapted_transaction(
                transaction,
                validated,
                &adapted,
                self.limits.deadline(),
            )
            .await
        }
        .await;
        result.map_err(|error| self.limits.annotate_refusal(error))
    }

    /// Check and adapt one request for a transaction this executor owns.
    pub(crate) fn adapt_owned(
        &self,
        validated: &ValidatedMatchRequest,
        supports_given_stage: bool,
    ) -> Result<AdaptedMatchRequest, OrmError> {
        self.check_lifecycle_budget()?;
        validated.recheck_schema(self.registry)?;
        if validated
            .capabilities()
            .contains(Capability::SchemaFunctionCall)
            && (!self
                .available_capabilities
                .contains(Capability::SchemaFunctionCall)
                || !supports_given_stage)
        {
            return Err(super::capability::schema_function_given_rows_unsupported().into());
        }
        self.preflight_compatibility(validated)?;
        self.check_lifecycle_budget()?;
        self.require_schema_authority()?;
        self.check_lifecycle_budget()?;
        let stack = adapt_match_request(self.registry, validated).map_err(adapter_diagnostic)?;
        if self
            .limits
            .policy
            .statements
            .is_some_and(|limit| stack.plans.len() > limit as usize)
        {
            return Err(MatchError::new(
                MatchErrorCategory::ResourceLimit,
                "statement_count_limit",
                "plan stack exceeds the provider-statement ceiling",
            )
            .into());
        }
        Ok(stack)
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
            .map_err(|error| self.limits.annotate_refusal(error))
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
        self.check_lifecycle_budget()?;
        let adapted = adapt_match_request(self.registry, validated).map_err(adapter_diagnostic)?;
        self.execute_adapted_borrowed(context, validated, &adapted)
            .await
    }

    fn preflight_compatibility(&self, validated: &ValidatedMatchRequest) -> Result<(), OrmError> {
        validated.recheck_schema(self.registry)?;
        validated.require_capabilities(&self.available_capabilities)?;
        check_provider_orders(self.registry, validated)?;
        Ok(())
    }

    pub(crate) fn check_lifecycle_budget(&self) -> Result<(), OrmError> {
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
        if self.limits.policy.statement_limit() == 0 {
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
        let policy = self
            .limits
            .policy
            .for_adapter_statements(adapted.plans.len());
        let deadline = deadline
            .map(QueryExecutionDeadline::from_instant)
            .unwrap_or_else(|| QueryExecutionDeadline::for_limits(policy));
        let provider = transaction.provider_mut()?;
        let supports_given = provider.supports_given_rows();
        let answers = crate::query_plan_exec::execute_plans(
            provider,
            supports_given,
            &adapted.plans,
            policy,
            &self.limits.cancellation,
            deadline,
        )
        .await
        .map_err(plan_execution_error)?;
        validated_match_result_from_plan_stack(self.registry, released, adapted, &answers, policy)
            .map_err(OrmError::from)
    }

    async fn execute_adapted_borrowed(
        &self,
        context: &TransactionContext,
        released: &ValidatedMatchRequest,
        adapted: &AdaptedMatchRequest,
    ) -> Result<ValidatedMatchResult, OrmError> {
        let policy = self
            .limits
            .policy
            .for_adapter_statements(adapted.plans.len());
        let deadline = self
            .limits
            .deadline()
            .map(QueryExecutionDeadline::from_instant)
            .unwrap_or_else(|| QueryExecutionDeadline::for_limits(policy));
        let answers = context
            .execute_plans(&adapted.plans, &policy, &self.limits.cancellation, deadline)
            .await
            .map_err(plan_execution_error)?;
        validated_match_result_from_plan_stack(self.registry, released, adapted, &answers, policy)
            .map_err(OrmError::from)
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
    if error.failure().is_some() {
        return error;
    }
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
    adapter_match_error(diagnostic).into()
}

/// Map a V1-to-V2 adapter diagnostic onto the released match error.
pub(super) fn adapter_match_error(diagnostic: Diagnostic) -> MatchError {
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
    error
}

fn plan_execution_error(error: OrmError) -> OrmError {
    if let OrmError::Match(ref matched) = error {
        if matched.code().as_str().starts_with("query_plan_")
            || matched.code().as_str().starts_with("query_remote_v3_")
        {
            let Ok(code) =
                type_bridge_contract::diagnostic::DiagnosticCode::new(matched.code().as_str())
            else {
                return canonical_provider_error(
                    "malformed_solution_row",
                    "provider evidence failed canonical typed match decoding",
                );
            };
            let diagnostic =
                Diagnostic::new(DiagnosticCategory::Integrity, code, matched.message());
            return released_plan_execution_error(&diagnostic).into();
        }
        return error;
    }
    provider_statement_error(error)
}

#[cfg(test)]
fn compatibility_execution_error(error: QueryV2ExecutionError) -> OrmError {
    match error {
        QueryV2ExecutionError::Provider(error) => provider_statement_error(error),
        QueryV2ExecutionError::Validation(diagnostic) => {
            released_plan_execution_error(&diagnostic).into()
        }
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

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, OwnedAttributeDescriptor};

    use crate::_entity::Annotation;
    use crate::MatchErrorPath;
    use crate::match_request::ids::BindingId;
    use crate::match_request::ids::BoundFieldId;
    use crate::match_request::model::{
        BindingPair, MatchBinding, MatchMode, MatchOrder, MatchPlan, MatchRequest, MissingOrder,
        SortDirection, Window,
    };
    use crate::match_request::model::{
        FetchShape, FetchSlot, MatchOperation, RowCardinality, ThingKind,
    };

    use crate::match_request::validation::validate_match_request;
    use crate::session::backend::{BoxFuture, DriverBackend, QueryResult, TransactionOps};
    use crate::value::AttributeValue;
    use tokio::sync::Notify;

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
            .with_max_role_players(8)
            .with_max_role_players(9)
            .with_max_statements(1)
            .with_max_statements(u8::MAX);
        assert_eq!(limits.resource_dimensions(), (3, 4, 2, 8, 1));
    }

    #[test]
    fn default_execution_limits_are_the_common_answer_policy() {
        let default = QueryExecutionResourceLimits::default();
        let limits = MatchExecutionLimits::default();
        assert_eq!(
            (limits.policy.items, limits.policy.bytes),
            (100_000, 64 * 1024 * 1024)
        );
        assert_eq!(limits.policy, default);
        // A caller cannot widen the policy through the tightening constructor.
        let widened = MatchExecutionLimits::tightened(
            u64::MAX,
            u64::MAX,
            Duration::from_secs(3600),
            AnswerCancellation::default(),
        );
        assert_eq!(widened.policy, default);
    }

    #[cfg(feature = "typedb")]
    #[test]
    fn provider_statement_preserves_runtime_classification_for_sdk_lowering() {
        let error = provider_statement_error(OrmError::QueryExecution(
            "[CNT9] TypeDB query rejected: unique constraint".into(),
        ));
        assert_eq!(error.failure().unwrap().code(), Some("CNT9"));
        let diagnostic = crate::lower_execution_error(
            error,
            type_bridge_contract::sdk_diagnostic::SdkProviderOperation::Read,
        );
        assert_eq!(diagnostic.code().as_str(), "provider_operation_failed");
        let json = serde_json::to_value(diagnostic.host_json()).unwrap();
        assert_eq!(json["details"]["operation"]["value"], "read");
        assert_eq!(json["details"]["rejection"]["value"], "unique");
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
        let (database, events) = native_database(CapabilitySet::all(), vec![]);

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
            assert!(events.queries.is_empty());
            assert!(events.queries.is_empty());
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
        for depth in 1..46 {
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
        let (database, events) = native_database(CapabilitySet::all(), vec![]);

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
            let (database, events) = native_database(CapabilitySet::all(), vec![]);
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
                assert!(events.queries.is_empty(), "{terminal}, borrowed");
                assert!(events.queries.is_empty(), "{terminal}, borrowed");
            }
            context.close().await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_generated_package_without_its_authority_is_refused_before_provider_work() {
        let registry = installed_person_registry(false);
        let validated = deeply_nested_count(&registry);
        let (database, events) = native_database(CapabilitySet::all(), vec![]);

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
        let source = concat!(
            include_str!("../query_v2_model.rs"),
            include_str!("plan_hydration.rs")
        );
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
        fn query(&mut self, _: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
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
                Ok(count_answer(0))
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
    #[derive(Default)]
    struct Events {
        opens: usize,
        closes: usize,
        commits: usize,
        rollbacks: usize,
        queries: Vec<String>,
        answers: VecDeque<Result<QueryResult, String>>,
        close_failure: bool,
        pause_at: Option<usize>,
        barrier: Option<Arc<Barrier>>,
    }
    #[derive(Default)]
    struct Barrier {
        entered: Notify,
        resume: Notify,
    }
    struct NativeBackend {
        events: Arc<Mutex<Events>>,
        capabilities: CapabilitySet,
    }
    impl DriverBackend for NativeBackend {
        fn match_capabilities(&self) -> CapabilitySet {
            self.capabilities.clone()
        }
        fn open_transaction(
            &self,
            _: &str,
            _: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            self.events.lock().unwrap().opens += 1;
            let events = Arc::clone(&self.events);
            Box::pin(
                async move { Ok(Box::new(NativeTransaction(events)) as Box<dyn TransactionOps>) },
            )
        }
        fn is_open(&self) -> bool {
            true
        }
    }
    struct NativeTransaction(Arc<Mutex<Events>>);
    impl TransactionOps for NativeTransaction {
        fn supports_given_rows(&self) -> bool {
            true
        }
        fn query_with_rows(
            &mut self,
            query: &str,
            rows: crate::session::backend::GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            assert!(!rows.variables.is_empty());
            self.query(query)
        }
        fn query(&mut self, query: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            let (answer, barrier) = {
                let mut events = self.0.lock().unwrap();
                events.queries.push(query.to_owned());
                let barrier = (events.pause_at == Some(events.queries.len()))
                    .then(|| events.barrier.clone())
                    .flatten();
                (
                    events
                        .answers
                        .pop_front()
                        .expect("one native answer per statement"),
                    barrier,
                )
            };
            Box::pin(async move {
                if let Some(barrier) = barrier {
                    barrier.entered.notify_one();
                    barrier.resume.notified().await;
                }
                answer.map_err(OrmError::QueryExecution)
            })
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.0.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.0.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }
        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            let mut events = self.0.lock().unwrap();
            events.closes += 1;
            let fail = events.close_failure;
            Box::pin(async move {
                if fail {
                    Err(OrmError::QueryExecution(
                        "credential=top-secret close".into(),
                    ))
                } else {
                    Ok(())
                }
            })
        }
    }
    fn native_database(
        capabilities: CapabilitySet,
        answers: Vec<QueryResult>,
    ) -> (Database, Arc<Mutex<Events>>) {
        let events = Arc::new(Mutex::new(Events {
            answers: answers.into_iter().map(Ok).collect(),
            ..Default::default()
        }));
        (
            Database::with_backend(
                Box::new(NativeBackend {
                    events: Arc::clone(&events),
                    capabilities,
                }),
                "test",
            ),
            events,
        )
    }
    fn count_answer(value: i64) -> QueryResult {
        QueryResult::Rows(vec![
            serde_json::json!({"b1":{"category":"value","value_type":"integer","value":value}}),
        ])
    }
    fn person_document(iid: &str, name: &str) -> Value {
        serde_json::json!({"_iid":iid,"_type":"person","attributes":{"name":name}})
    }
    fn person_documents(names: &[(&str, &str)]) -> QueryResult {
        QueryResult::Documents(
            names
                .iter()
                .map(|(iid, name)| person_document(iid, name))
                .collect(),
        )
    }
    fn count_request(registry: &DescriptorRegistry) -> ValidatedMatchRequest {
        validate_match_request(
            registry,
            person_root_request(registry, |root| MatchOperation::CountBy { root }),
        )
        .unwrap()
    }
    fn hidden_answers() -> Vec<QueryResult> {
        vec![
            QueryResult::Rows(vec![serde_json::json!({
                "b0":{"category":"entity","label":"person","iid":"0x01"},
                "b1":{"category":"entity","label":"person","iid":"0x02"}
            })]),
            person_documents(&[("0x01", "Ada")]),
        ]
    }

    #[tokio::test]
    async fn native_owned_reads_close_once_without_committing_and_borrowed_reads_stay_reusable() {
        let registry = person_registry();
        let request = count_request(&registry);
        let (database, events) = native_database(
            CapabilitySet::all(),
            vec![count_answer(7), count_answer(8), count_answer(9)],
        );
        let result = database.execute_match(&registry, &request).await.unwrap();
        assert!(matches!(
            result.result(),
            super::super::result::MatchResult::Count { value: 7, .. }
        ));
        let context = database.transaction_context(TxType::Read).await.unwrap();
        for _ in 0..2 {
            context.execute_match(&registry, &request).await.unwrap();
        }
        {
            let events = events.lock().unwrap();
            assert_eq!(
                (
                    events.opens,
                    events.closes,
                    events.commits,
                    events.rollbacks
                ),
                (2, 1, 0, 0)
            );
            assert_eq!(events.queries.len(), 3);
        }
        context.close().await.unwrap();
        assert_eq!(events.lock().unwrap().closes, 2);
    }

    #[tokio::test]
    async fn native_fetch_cardinality_and_complete_window_fail_without_partial_results() {
        for (cardinality, docs, expected) in [
            (RowCardinality::ExactlyOne, vec![], "no_result"),
            (
                RowCardinality::ExactlyOne,
                vec![("0x01", "Ada"), ("0x02", "Grace")],
                "not_unique",
            ),
            (
                RowCardinality::Complete,
                vec![("0x01", "Ada"), ("0x02", "Grace")],
                "row_window_exceeded",
            ),
        ] {
            let registry = person_registry();
            let request =
                validate_match_request(&registry, person_request(&registry, cardinality, 1))
                    .unwrap();
            let (database, events) =
                native_database(CapabilitySet::all(), vec![person_documents(&docs)]);
            let error = database
                .execute_match(&registry, &request)
                .await
                .unwrap_err();
            assert_eq!(match_code(&error), expected);
            assert_eq!(events.lock().unwrap().closes, 1);
        }
    }

    #[tokio::test]
    async fn native_fetch_correlates_hidden_identity_and_detects_selected_window_drift() {
        let registry = person_registry();
        let request =
            validate_match_request(&registry, person_with_hidden_witness_request(&registry))
                .unwrap();
        let (database, events) = native_database(CapabilitySet::all(), hidden_answers());
        database.execute_match(&registry, &request).await.unwrap();
        assert_eq!(events.lock().unwrap().queries.len(), 2);
        for bad in [vec![], vec![("0x03", "Unexpected")]] {
            let mut answers = hidden_answers();
            answers[1] = person_documents(&bad);
            let (database, events) = native_database(CapabilitySet::all(), answers);
            assert!(database.execute_match(&registry, &request).await.is_err());
            assert_eq!(events.lock().unwrap().closes, 1);
        }
    }

    #[tokio::test]
    async fn native_hidden_predicate_operands_are_checked_without_hidden_hydration() {
        let registry = person_registry();
        let mut raw = person_with_hidden_witness_request(&registry);
        raw.plan.predicate = Some(crate::MatchExpr::FieldValue {
            field: BoundFieldId::new(
                BindingId::new(1),
                FieldId::new(registry.descriptor_id("person").unwrap(), "name"),
            ),
            operator: crate::ComparisonOp::Equal,
            value: AttributeValue::String("Grace".into()),
        });
        let request = validate_match_request(&registry, raw).unwrap();
        let stack =
            crate::match_request::plan_adapter::adapt_match_request(&registry, &request).unwrap();
        let crate::match_request::plan_adapter::Layout::Fetch { witnesses, .. } = &stack.layout
        else {
            panic!("fetch");
        };
        assert_eq!(stack.plans.len(), 2);
        assert_eq!(witnesses.len(), 1);
        let column = format!("b{}", witnesses[0].1.get());
        for (value, succeeds) in [
            (
                Some(
                    serde_json::json!({"category":"Attribute","label":"name","value_type":"string","value":"Grace"}),
                ),
                true,
            ),
            (
                Some(
                    serde_json::json!({"category":"Attribute","label":"name","value_type":"string","value":"Wrong"}),
                ),
                false,
            ),
            (Some(serde_json::Value::Null), false),
            (None, false),
        ] {
            let mut answers = hidden_answers();
            let QueryResult::Rows(rows) = &mut answers[0] else {
                panic!("mapping");
            };
            let expected_statements = if value.is_none() { 1 } else { 2 };
            if let Some(value) = value {
                rows[0][&column] = value;
            }
            let (database, events) = native_database(CapabilitySet::all(), answers);
            let result = database.execute_match(&registry, &request).await;
            assert_eq!(result.is_ok(), succeeds, "{result:?}");
            assert_eq!(events.lock().unwrap().queries.len(), expected_statements);
        }
    }

    #[tokio::test]
    async fn native_statement_item_byte_and_graph_limits_refuse_atomically() {
        let registry = person_registry();
        let request =
            validate_match_request(&registry, person_with_hidden_witness_request(&registry))
                .unwrap();
        for limits in [
            MatchExecutionLimits::default().with_max_statements(1),
            MatchExecutionLimits::tightened(
                0,
                4096,
                Duration::from_secs(1),
                AnswerCancellation::default(),
            ),
            MatchExecutionLimits::tightened(
                100,
                1,
                Duration::from_secs(1),
                AnswerCancellation::default(),
            ),
            MatchExecutionLimits::default().with_max_hydrated_things(1),
            MatchExecutionLimits::default().with_max_attribute_values(0),
        ] {
            let (database, events) = native_database(CapabilitySet::all(), hidden_answers());
            let error = database
                .execute_match_with_limits(&registry, &request, limits)
                .await
                .unwrap_err();
            assert_eq!(
                match_error(&error).category(),
                MatchErrorCategory::ResourceLimit,
                "{error}"
            );
            let events = events.lock().unwrap();
            assert_eq!(events.closes, events.opens);
            assert_eq!(events.commits, 0);
        }
    }

    #[tokio::test]
    async fn native_malformed_answers_and_provider_failures_preserve_first_error_over_close_failure()
     {
        let registry = person_registry();
        let request = count_request(&registry);
        for answer in [
            Ok(count_answer(-1)),
            Ok(QueryResult::Documents(vec![])),
            Err("credential=top-secret query".into()),
        ] {
            let (database, events) = native_database(CapabilitySet::all(), vec![]);
            {
                let mut events = events.lock().unwrap();
                events.answers.push_back(answer);
                events.close_failure = true;
            }
            let error = database
                .execute_match(&registry, &request)
                .await
                .unwrap_err();
            assert_ne!(match_code(&error), "provider_transaction_close_failed");
            assert!(!error.to_string().contains("top-secret"));
            assert_eq!(events.lock().unwrap().closes, 1);
        }
        let (database, events) = native_database(CapabilitySet::all(), vec![count_answer(1)]);
        events.lock().unwrap().close_failure = true;
        let error = database
            .execute_match(&registry, &request)
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "provider_transaction_close_failed");
    }

    #[tokio::test]
    async fn native_execution_fences_registry_snapshot_for_owned_and_borrowed_reads() {
        for borrowed in [false, true] {
            let registry = Arc::new(person_registry());
            let mut raw = person_request(&registry, RowCardinality::ExactlyOne, 1);
            raw.plan.bindings[0].match_mode = MatchMode::Subtypes;
            let request = Arc::new(validate_match_request(&registry, raw).unwrap());
            let (database, events) = native_database(
                CapabilitySet::all(),
                vec![person_documents(&[("0x01", "Ada")])],
            );
            let barrier = Arc::new(Barrier::default());
            {
                let mut events = events.lock().unwrap();
                events.pause_at = Some(1);
                events.barrier = Some(Arc::clone(&barrier));
            }
            let context = if borrowed {
                Some(database.transaction_context(TxType::Read).await.unwrap())
            } else {
                None
            };
            let execution_context = context.clone();
            let execution_registry = Arc::clone(&registry);
            let execution_request = Arc::clone(&request);
            let execution = tokio::spawn(async move {
                match execution_context {
                    Some(context) => {
                        context
                            .execute_match(&execution_registry, &execution_request)
                            .await
                    }
                    None => {
                        database
                            .execute_match(&execution_registry, &execution_request)
                            .await
                    }
                }
            });
            barrier.entered.notified().await;
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
                request
                    .recheck_schema(&registry)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "stale_schema"
            );
            barrier.resume.notify_one();
            execution.await.unwrap().unwrap();
            if let Some(context) = context {
                context.close().await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn native_borrowed_mutex_covers_the_whole_static_read_stack() {
        let registry = Arc::new(person_registry());
        let request = Arc::new(
            validate_match_request(&registry, person_with_hidden_witness_request(&registry))
                .unwrap(),
        );
        let mut answers = hidden_answers();
        answers.push(count_answer(0));
        let (database, events) = native_database(CapabilitySet::all(), answers);
        let barrier = Arc::new(Barrier::default());
        {
            let mut events = events.lock().unwrap();
            events.pause_at = Some(2);
            events.barrier = Some(Arc::clone(&barrier));
        }
        let context = database.transaction_context(TxType::Read).await.unwrap();
        let execution_context = context.clone();
        let execution =
            tokio::spawn(async move { execution_context.execute_match(&registry, &request).await });
        barrier.entered.notified().await;
        let other_context = context.clone();
        let other = tokio::spawn(async move {
            other_context
                .query("match $probe isa person; select $probe;")
                .await
        });
        tokio::task::yield_now().await;
        assert_eq!(events.lock().unwrap().queries.len(), 2);
        barrier.resume.notify_one();
        execution.await.unwrap().unwrap();
        other.await.unwrap().unwrap();
        {
            let events = events.lock().unwrap();
            assert_eq!(events.queries.len(), 3);
            assert!(events.queries[2].contains("$probe"));
        }
        context.close().await.unwrap();
    }
    #[tokio::test]
    async fn native_adapter_default_allows_four_plans_but_explicit_three_refuses_before_io() {
        let registry = person_registry();
        let mut raw = person_with_hidden_witness_request(&registry);
        raw.operation = MatchOperation::PageBy {
            root: BindingId::new(0),
            output: FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::Collect {
                        binding: BindingId::new(1),
                        distinct: true,
                        order: vec![],
                    },
                ],
            },
            order: vec![],
            window: Window {
                offset: 0,
                limit: 1,
            },
            include_total: true,
        };
        let request = validate_match_request(&registry, raw).unwrap();
        let mut answers = vec![QueryResult::Rows(vec![serde_json::json!({
            "b2":{"category":"value","value_type":"integer","value":1}
        })])];
        answers.extend(hidden_answers());
        answers.push(person_documents(&[("0x02", "Grace")]));
        let (database, events) = native_database(CapabilitySet::all(), answers);
        database.execute_match(&registry, &request).await.unwrap();
        assert_eq!(events.lock().unwrap().queries.len(), 4);
        let (database, events) = native_database(CapabilitySet::all(), vec![]);
        let error = database
            .execute_match_with_limits(
                &registry,
                &request,
                MatchExecutionLimits::default().with_max_statements(3),
            )
            .await
            .unwrap_err();
        assert_eq!(match_code(&error), "statement_count_limit");
        assert_eq!(events.lock().unwrap().opens, 0);
        let page = validate_match_request(&registry, person_page_request(&registry, true)).unwrap();
        let stack =
            crate::match_request::plan_adapter::adapt_match_request(&registry, &page).unwrap();
        assert_eq!(stack.plans.len(), 2);
    }
}
