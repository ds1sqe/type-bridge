use futures::future::join_all;

use super::traits::{InterceptError, Interceptor, RequestContext};

use super::traits::{CommitOutcome, PolicyOutcome, PolicyPlan, StagedWriteAuthority};

/// Ordered request interceptor chain with reverse-order response hooks.
pub struct InterceptorChain {
    interceptors: Vec<Box<dyn Interceptor>>,
}

impl InterceptorChain {
    /// Construct a chain whose request order matches the supplied vector.
    pub fn new(interceptors: Vec<Box<dyn Interceptor>>) -> Self {
        Self { interceptors }
    }

    /// Run header/identity policy before decoding an untrusted staged envelope.
    pub async fn execute_transport(&self, ctx: &mut RequestContext) -> Result<(), InterceptError> {
        for interceptor in &self.interceptors {
            interceptor.on_transport(ctx).await?;
        }
        Ok(())
    }

    /// Run every raw policy so auditing survives a refusal; the first error wins.
    pub async fn execute_raw(
        &self,
        query: &str,
        kind: type_bridge_orm::session::staged::TransactionKind,
        ctx: &mut RequestContext,
    ) -> Result<(), InterceptError> {
        let mut refusal = None;
        for interceptor in &self.interceptors {
            if let Err(error) = interceptor.on_raw(query, kind, ctx).await
                && refusal.is_none()
            {
                refusal = Some(error);
            }
        }
        refusal.map_or(Ok(()), Err)
    }

    /// Admit a staged write: any error refuses, at least one policy must
    /// vouch for the caller.
    pub async fn execute_write_authority(
        &self,
        ctx: &RequestContext,
    ) -> Result<bool, InterceptError> {
        let mut admitted = false;
        for interceptor in &self.interceptors {
            admitted |=
                interceptor.on_write_authority(ctx).await? == StagedWriteAuthority::Authenticated;
        }
        Ok(admitted)
    }

    /// Run staged plan policy in order; the first refusal wins. Each policy
    /// sees row values only if it asked for them.
    pub async fn execute_plan(
        &self,
        index: usize,
        plan: PolicyPlan<'_>,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        for interceptor in &self.interceptors {
            let view = plan.with_rows_visible(interceptor.wants_plan_rows());
            interceptor.on_plan(index, &view, ctx).await?;
        }
        Ok(())
    }

    /// Run every staged commit observer concurrently; errors are logged only.
    pub async fn execute_commit(&self, outcome: &CommitOutcome<'_>, ctx: &RequestContext) {
        let results = join_all(
            self.interceptors
                .iter()
                .map(|interceptor| interceptor.on_commit(outcome, ctx)),
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
    pub async fn execute_response(
        &self,
        outcome: &PolicyOutcome<'_>,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        // Poll every hook concurrently. A stuck policy can still consume the
        // transport's bounded audit grace, but cannot prevent later audit
        // hooks from being invoked at all.
        let results = join_all(
            self.interceptors
                .iter()
                .rev()
                .map(|interceptor| interceptor.on_response(outcome, ctx)),
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
