use crate::error::PipelineError;
use crate::interceptor::{Interceptor, InterceptorChain, RequestContext};

use crate::interceptor::{CommitOutcome, PolicyOutcome, PolicyPlan};
use std::collections::HashMap;

pub(crate) const UNAUTHENTICATED_CLIENT_ID: &str = "unknown";

/// Policy pipeline for staged requests; execution belongs to the staged executor.
pub struct QueryPipeline {
    interceptor_chain: InterceptorChain,
    default_database: String,
}

impl QueryPipeline {
    /// Construct the policy context shared by typed and raw requests.
    pub fn request_context(&self, metadata: HashMap<String, serde_json::Value>) -> RequestContext {
        RequestContext {
            request_id: uuid::Uuid::new_v4().to_string(),
            client_id: UNAUTHENTICATED_CLIENT_ID.to_owned(),
            database: self.default_database.clone(),
            transaction_type: "read".to_owned(),
            metadata,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Authenticate and rate-limit staged transport metadata before envelope
    /// decoding or other attacker-controlled contract work.
    pub async fn begin_request(&self, context: &mut RequestContext) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_transport(context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Apply host raw-query policies before dispatch, preserving typed refusals.
    pub async fn authorize_raw(
        &self,
        query: &str,
        kind: type_bridge_orm::session::staged::TransactionKind,
        ctx: &mut RequestContext,
    ) -> Result<(), crate::interceptor::InterceptError> {
        self.interceptor_chain.execute_raw(query, kind, ctx).await
    }

    /// Whether the configured policies admit a staged write for this
    /// caller. An interceptor error refuses.
    pub async fn authorize_write(&self, context: &RequestContext) -> Result<bool, PipelineError> {
        self.interceptor_chain
            .execute_write_authority(context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Run the staged plan policies for plan `index`. An interceptor error
    /// refuses the request.
    pub async fn authorize_plan(
        &self,
        index: usize,
        plan: PolicyPlan<'_>,
        context: &RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_plan(index, plan, context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Let every policy observe a staged transaction's outcome.
    pub async fn observe_commit(&self, outcome: &CommitOutcome<'_>, context: &RequestContext) {
        self.interceptor_chain
            .execute_commit(outcome, context)
            .await;
    }

    /// Run configured response and audit policies for every staged outcome.
    pub async fn finish_request(
        &self,
        outcome: &PolicyOutcome<'_>,
        context: &RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_response(outcome, context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }
}

/// Builder for a staged request policy pipeline.
#[derive(Default)]
pub struct PipelineBuilder {
    interceptors: Vec<Box<dyn Interceptor>>,
    default_database: String,
}

impl PipelineBuilder {
    /// Start with no interceptors and no database override.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one policy in request order.
    pub fn with_interceptor(mut self, interceptor: impl Interceptor + 'static) -> Self {
        self.interceptors.push(Box::new(interceptor));
        self
    }

    /// Set the database exposed to request policies.
    pub fn with_default_database(mut self, database: impl Into<String>) -> Self {
        self.default_database = database.into();
        self
    }

    /// Build the policy chain.
    pub fn build(self) -> Result<QueryPipeline, PipelineError> {
        Ok(QueryPipeline {
            interceptor_chain: InterceptorChain::new(self.interceptors),
            default_database: self.default_database,
        })
    }
}
