//! Unchecked TypeQL at database and explicit raw-transaction scope.

pub use type_bridge_orm::CommitFailureCertainty;
pub use type_bridge_orm::session::raw::RawQueryError;
pub use type_bridge_orm::session::staged::TransactionKind;

/// A real eager direct transaction or one single-query proxy context.
pub struct RawTransaction {
    inner: type_bridge_orm::session::raw::RawTransaction,
}

impl RawTransaction {
    pub(crate) fn new(inner: type_bridge_orm::session::raw::RawTransaction) -> Self {
        Self { inner }
    }

    /// Raw, unchecked TypeQL: the escape hatch. The TypeQL text is sent
    /// to TypeDB as written, and TypeDB's output comes back as it
    /// answered, in this SDK's native data types. TypeBridge adds no
    /// validation, rewriting, hydration, hooks, limits or extra
    /// queries. It runs in read, write and schema transactions and
    /// accepts any TypeQL, schema-breaking changes included. The caller
    /// owns its correctness; TypeBridge makes no promise about its
    /// effects. On a proxy connection it posts to the server's opt-in
    /// `/v3/raw` endpoint, which is off by default.
    ///
    /// On a proxy connection a raw transaction holds one query, because
    /// `/v3/raw` runs each request as its own transaction.
    pub async fn execute_query(&mut self, query: &str) -> Result<serde_json::Value, RawQueryError> {
        let answer = self.inner.execute_query(query).await?;
        serde_json::from_slice(&answer).map_err(|error| RawQueryError {
            code: "raw_answer_encoding".into(),
            message: error.to_string(),
            status: 502,
            provider_code: None,
            commit_outcome: None,
        })
    }

    /// Commit direct writes; after a successful proxy write this is a no-op.
    pub async fn commit(&mut self) -> Result<(), RawQueryError> {
        self.inner.commit().await
    }

    /// Roll back pending direct writes. Committed proxy writes return
    /// `raw_transaction_already_committed`.
    pub async fn rollback(&mut self) -> Result<(), RawQueryError> {
        self.inner.rollback().await
    }

    /// Release the context without committing pending direct writes.
    pub async fn close(&mut self) -> Result<(), RawQueryError> {
        self.inner.close().await
    }

    pub(crate) async fn execute_one(
        mut self,
        query: &str,
    ) -> Result<serde_json::Value, RawQueryError> {
        let result: Result<_, RawQueryError> = async {
            let answer = self.execute_query(query).await?;
            self.commit().await?;
            Ok(answer)
        }
        .await;
        let closed = self.close().await;
        let answer = result?;
        closed?;
        Ok(answer)
    }
}
