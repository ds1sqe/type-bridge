use crate::error::PipelineError;
use crate::interceptor::{Interceptor, InterceptorChain, RequestContext};
#[cfg(feature = "v2-query")]
use crate::interceptor::{V3CommitOutcome, V3PolicyOutcome, V3PolicyPlan};
use std::collections::HashMap;

pub(crate) const UNAUTHENTICATED_CLIENT_ID: &str = "unknown";

/// Policy pipeline for staged requests; execution belongs to the staged executor.
pub struct QueryPipeline {
    interceptor_chain: InterceptorChain,
    default_database: String,
}

impl QueryPipeline {
    /// Construct one staged policy context after proving every configured
    /// interceptor explicitly supports typed plans.
    #[cfg(feature = "v2-query")]
    pub fn v3_request_context(
        &self,
        metadata: HashMap<String, serde_json::Value>,
    ) -> Result<RequestContext, PipelineError> {
        self.validate_v3_coverage()?;
        Ok(RequestContext {
            request_id: uuid::Uuid::new_v4().to_string(),
            client_id: UNAUTHENTICATED_CLIENT_ID.to_owned(),
            database: self.default_database.clone(),
            transaction_type: "read".to_owned(),
            metadata,
            timestamp: chrono::Utc::now(),
        })
    }

    /// Prove every configured policy explicitly covers staged V3 requests.
    ///
    /// Request admission checks coverage before running any policy hooks.
    #[cfg(feature = "v2-query")]
    pub fn validate_v3_coverage(&self) -> Result<(), PipelineError> {
        self.interceptor_chain
            .ensure_v3_coverage()
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Authenticate and rate-limit staged transport metadata before envelope
    /// decoding or other attacker-controlled contract work.
    #[cfg(feature = "v2-query")]
    pub async fn begin_v3_request(
        &self,
        context: &mut RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_v3_transport(context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Whether the configured policies admit a staged V3 write for this
    /// caller. An interceptor error refuses.
    #[cfg(feature = "v2-query")]
    pub async fn authorize_v3_write(
        &self,
        context: &RequestContext,
    ) -> Result<bool, PipelineError> {
        self.interceptor_chain
            .execute_v3_write_authority(context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Run the staged plan policies for plan `index`. An interceptor error
    /// refuses the request.
    #[cfg(feature = "v2-query")]
    pub async fn authorize_v3_plan(
        &self,
        index: usize,
        plan: V3PolicyPlan<'_>,
        context: &RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_v3_plan(index, plan, context)
            .await
            .map_err(|error| PipelineError::Interceptor(error.to_string()))
    }

    /// Let every policy observe a staged transaction's outcome.
    #[cfg(feature = "v2-query")]
    pub async fn observe_v3_commit(&self, outcome: &V3CommitOutcome<'_>, context: &RequestContext) {
        self.interceptor_chain
            .execute_v3_commit(outcome, context)
            .await;
    }

    /// Run configured response and audit policies for every staged outcome.
    #[cfg(feature = "v2-query")]
    pub async fn finish_v3_request(
        &self,
        outcome: &V3PolicyOutcome<'_>,
        context: &RequestContext,
    ) -> Result<(), PipelineError> {
        self.interceptor_chain
            .execute_v3_response(outcome, context)
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
