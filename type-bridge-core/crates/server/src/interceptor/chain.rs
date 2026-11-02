#![allow(deprecated)] // Legacy node bridges are removed in 3.0.
#[cfg(feature = "v2-query")]
use futures::future::join_all;

use super::traits::{InterceptError, Interceptor, RequestContext};
#[cfg(feature = "v2-query")]
use super::traits::{StagedWriteAuthority, V3CommitOutcome, V3PolicyOutcome, V3PolicyPlan};

/// Ordered request interceptor chain with reverse-order response hooks.
pub struct InterceptorChain {
    interceptors: Vec<Box<dyn Interceptor>>,
}

impl InterceptorChain {
    /// Construct a chain whose request order matches the supplied vector.
    pub fn new(interceptors: Vec<Box<dyn Interceptor>>) -> Self {
        Self { interceptors }
    }

    /// Fail closed if any configured policy has not explicitly declared
    /// staged V3 coverage.
    ///
    #[cfg(feature = "v2-query")]
    pub fn ensure_v3_coverage(&self) -> Result<(), InterceptError> {
        if let Some(interceptor) = self
            .interceptors
            .iter()
            .find(|interceptor| !interceptor.supports_v3())
        {
            return Err(InterceptError::ValidationFailed {
                reason: format!(
                    "interceptor {:?} does not declare staged V3 coverage",
                    interceptor.name(),
                ),
            });
        }
        Ok(())
    }

    /// Run header/identity policy before decoding an untrusted staged envelope.
    #[cfg(feature = "v2-query")]
    pub async fn execute_v3_transport(
        &self,
        ctx: &mut RequestContext,
    ) -> Result<(), InterceptError> {
        for interceptor in &self.interceptors {
            interceptor.on_v3_transport(ctx).await?;
        }
        Ok(())
    }

    /// Admit a staged V3 write: any error refuses, at least one policy must
    /// vouch for the caller.
    #[cfg(feature = "v2-query")]
    pub async fn execute_v3_write_authority(
        &self,
        ctx: &RequestContext,
    ) -> Result<bool, InterceptError> {
        let mut admitted = false;
        for interceptor in &self.interceptors {
            admitted |= interceptor.on_v3_write_authority(ctx).await?
                == StagedWriteAuthority::Authenticated;
        }
        Ok(admitted)
    }

    /// Run staged plan policy in order; the first refusal wins. Each policy
    /// sees row values only if it asked for them.
    #[cfg(feature = "v2-query")]
    pub async fn execute_v3_plan(
        &self,
        index: usize,
        plan: V3PolicyPlan<'_>,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        for interceptor in &self.interceptors {
            let view = plan.with_rows_visible(interceptor.wants_v3_plan_rows());
            interceptor.on_v3_plan(index, &view, ctx).await?;
        }
        Ok(())
    }

    /// Run every staged commit observer concurrently; errors are logged only.
    #[cfg(feature = "v2-query")]
    pub async fn execute_v3_commit(&self, outcome: &V3CommitOutcome<'_>, ctx: &RequestContext) {
        let results = join_all(
            self.interceptors
                .iter()
                .map(|interceptor| interceptor.on_v3_commit(outcome, ctx)),
        )
        .await;
        for (interceptor, result) in self.interceptors.iter().zip(results) {
            if result.is_err() {
                tracing::warn!(
                    interceptor = interceptor.name(),
                    "staged commit observer failed"
                );
            }
        }
    }

    /// Run every staged response hook even when one rejects, so audit evidence is
    /// not skipped by interceptor ordering. The first error remains decisive.
    #[cfg(feature = "v2-query")]
    pub async fn execute_v3_response(
        &self,
        outcome: &V3PolicyOutcome<'_>,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        // Poll every hook concurrently. A stuck policy can still consume the
        // transport's bounded audit grace, but cannot prevent later audit
        // hooks from being invoked at all.
        let results = join_all(
            self.interceptors
                .iter()
                .rev()
                .map(|interceptor| interceptor.on_v3_response(outcome, ctx)),
        )
        .await;
        results
            .into_iter()
            .find_map(Result::err)
            .map_or(Ok(()), Err)
    }

    /// Return configured interceptor names in request order.
    pub fn interceptor_names(&self) -> Vec<&str> {
        self.interceptors.iter().map(|i| i.name()).collect()
    }
}
