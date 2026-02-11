use type_bridge_core_lib::ast::Clause;

use super::traits::{InterceptError, Interceptor, RequestContext};

pub struct InterceptorChain {
    interceptors: Vec<Box<dyn Interceptor>>,
}

impl InterceptorChain {
    pub fn new(interceptors: Vec<Box<dyn Interceptor>>) -> Self {
        Self { interceptors }
    }

    /// Run all request interceptors in order. Each can transform or reject the query.
    pub async fn execute_request(
        &self,
        mut clauses: Vec<Clause>,
        ctx: &mut RequestContext,
    ) -> Result<Vec<Clause>, InterceptError> {
        for interceptor in &self.interceptors {
            clauses = interceptor.on_request(clauses, ctx).await?;
        }
        Ok(clauses)
    }

    /// Run all response interceptors in reverse order (middleware pattern).
    pub async fn execute_response(
        &self,
        result: &serde_json::Value,
        ctx: &RequestContext,
    ) -> Result<(), InterceptError> {
        for interceptor in self.interceptors.iter().rev() {
            interceptor.on_response(result, ctx).await?;
        }
        Ok(())
    }

    pub fn interceptor_names(&self) -> Vec<&str> {
        self.interceptors.iter().map(|i| i.name()).collect()
    }
}
