//! Shared unchecked-query lifecycle for direct and proxy SDK connections.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use super::backend::{BoxFuture, TransactionOps, TxType};
use crate::CommitFailureCertainty;

/// An unchecked provider error or a raw transaction lifecycle refusal.
#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct RawQueryError {
    /// Provider code or stable TypeBridge refusal code.
    pub code: String,
    /// Provider message, retained verbatim when available.
    pub message: String,
    /// HTTP status supplied by the provider or proxy.
    pub status: u16,
    /// Original provider code when the public code describes unknown durability.
    pub provider_code: Option<String>,
    /// Whether a failed commit is known to have aborted.
    pub commit_outcome: Option<CommitFailureCertainty>,
}

impl RawQueryError {
    pub(crate) fn refusal(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            status: 400,
            provider_code: None,
            commit_outcome: None,
        }
    }

    pub(crate) fn unsupported() -> Self {
        Self::refusal(
            "raw_unsupported",
            "this backend does not support unchecked raw queries",
        )
    }

    pub(crate) fn unknown(mut self) -> Self {
        if self.code != "commit_outcome_unknown" {
            self.provider_code = Some(self.code.clone());
        }
        self.code = "commit_outcome_unknown".into();
        self.commit_outcome = Some(CommitFailureCertainty::Unknown);
        self
    }
}

#[cfg(feature = "typedb")]
impl From<type_bridge_typedb_runtime::raw::RawQueryError> for RawQueryError {
    fn from(error: type_bridge_typedb_runtime::raw::RawQueryError) -> Self {
        use type_bridge_typedb_runtime::CommitFailureCertainty as RuntimeCertainty;
        let mut result = Self {
            code: error.code,
            message: error.message,
            status: error.status,
            provider_code: None,
            commit_outcome: error.commit_outcome.map(|certainty| match certainty {
                RuntimeCertainty::DefinitelyAborted => CommitFailureCertainty::DefinitelyAborted,
                RuntimeCertainty::Unknown => CommitFailureCertainty::Unknown,
            }),
        };
        if result.commit_outcome == Some(CommitFailureCertainty::Unknown) {
            result = result.unknown();
        }
        result
    }
}

/// Caller-owned exchange for one `/v3/raw` request, returning its exact answer.
pub type RawExchangeFn =
    Arc<dyn Fn(Vec<u8>) -> BoxFuture<'static, Result<Vec<u8>, RawQueryError>> + Send + Sync>;

enum Route {
    Direct(Option<Box<dyn TransactionOps>>),
    Proxy {
        exchange: RawExchangeFn,
        used: bool,
        admission_milliseconds: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    Committed,
    Closed,
    Failed,
    Unknown,
}

/// An eager direct transaction or a single-query proxy transaction.
/// Direct queries share one real transaction until the caller commits or closes.
/// A proxy query completes on the host; no server transaction handle is retained.
pub struct RawTransaction {
    route: Route,
    kind: TxType,
    state: State,
}

impl RawTransaction {
    pub(super) fn direct(transaction: Box<dyn TransactionOps>, kind: TxType) -> Self {
        Self {
            route: Route::Direct(Some(transaction)),
            kind,
            state: State::Open,
        }
    }

    /// Create a proxy context. Its first query performs exactly one exchange.
    /// The admission window bounds request delivery, not provider execution.
    pub fn proxy(
        exchange: RawExchangeFn,
        kind: TxType,
        admission_milliseconds: std::num::NonZeroU64,
    ) -> Self {
        Self {
            route: Route::Proxy {
                exchange,
                used: false,
                admission_milliseconds: admission_milliseconds.get(),
            },
            kind,
            state: State::Open,
        }
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
    /// A proxy context accepts one query; writes and schema changes commit on
    /// the host before returning, and cannot subsequently be rolled back.
    pub async fn execute_query(&mut self, query: &str) -> Result<Vec<u8>, RawQueryError> {
        if matches!(self.route, Route::Proxy { used: true, .. }) {
            return Err(RawQueryError::refusal(
                "raw_transaction_single_query",
                "a proxy raw transaction accepts only one query",
            ));
        }
        self.require_open()?;
        match &mut self.route {
            Route::Direct(transaction) => {
                transaction
                    .as_mut()
                    .expect("open raw transaction retains its provider")
                    .query_raw(query)
                    .await
            }
            Route::Proxy {
                exchange,
                used,
                admission_milliseconds,
            } => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .and_then(|now| u64::try_from(now.as_millis()).ok())
                    .and_then(|now| now.checked_add(*admission_milliseconds))
                    .ok_or_else(|| {
                        RawQueryError::refusal(
                            "raw_request_expiry",
                            "raw request expiry cannot be represented",
                        )
                    })?;
                let kind = match self.kind {
                    TxType::Read => "read",
                    TxType::Write => "write",
                    TxType::Schema => "schema",
                };
                let request = serde_json::to_vec(&serde_json::json!({
                    "query": query, "kind": kind,
                    "nonce": uuid::Uuid::new_v4().simple().to_string(),
                    "expires_at_unix_ms": now,
                }))
                .expect("raw request contains only JSON primitives");
                // Mark before dispatch: retrying after a lost reply could duplicate writes.
                *used = true;
                self.state = if self.kind == TxType::Read {
                    State::Failed
                } else {
                    State::Unknown
                };
                let result = exchange(request).await;
                self.state = match &result {
                    Ok(_) if self.kind != TxType::Read => State::Committed,
                    Ok(_) => State::Open,
                    Err(error) if error.commit_outcome == Some(CommitFailureCertainty::Unknown) => {
                        State::Unknown
                    }
                    Err(_) => State::Failed,
                };
                result
            }
        }
    }

    /// Decode a native SDK answer while preserving proxy write uncertainty.
    /// A successful proxy exchange may already have committed before decoding.
    pub fn decode_answer(&mut self, bytes: &[u8]) -> Result<serde_json::Value, RawQueryError> {
        serde_json::from_slice(bytes).map_err(|error| {
            let error = RawQueryError {
                code: "raw_answer_encoding".into(),
                message: error.to_string(),
                status: 502,
                provider_code: None,
                commit_outcome: None,
            };
            if matches!(self.route, Route::Proxy { used: true, .. }) && self.kind != TxType::Read {
                self.state = State::Unknown;
                error.unknown()
            } else {
                error
            }
        })
    }

    /// Commit direct writes. After a successful proxy query this is a no-op.
    pub async fn commit(&mut self) -> Result<(), RawQueryError> {
        if self.state == State::Committed
            || (self.kind == TxType::Read && self.state == State::Closed)
        {
            return Ok(());
        }
        self.require_open()?;
        match &mut self.route {
            Route::Direct(transaction) => {
                let mut transaction = transaction
                    .take()
                    .expect("open raw transaction retains its provider");
                // Cancellation after sending commit cannot prove an abort.
                self.state = if self.kind == TxType::Read {
                    State::Failed
                } else {
                    State::Unknown
                };
                let result = if self.kind == TxType::Read {
                    transaction.close_raw().await
                } else {
                    transaction.commit_raw().await
                };
                self.state = match &result {
                    Ok(()) if self.kind == TxType::Read => State::Closed,
                    Ok(()) => State::Committed,
                    Err(error) if error.commit_outcome == Some(CommitFailureCertainty::Unknown) => {
                        State::Unknown
                    }
                    Err(_) => State::Failed,
                };
                result
            }
            Route::Proxy { .. } => {
                self.state = if self.kind == TxType::Read {
                    State::Closed
                } else {
                    State::Committed
                };
                Ok(())
            }
        }
    }

    /// Roll back direct queries. A committed proxy write cannot be rolled back.
    pub async fn rollback(&mut self) -> Result<(), RawQueryError> {
        if self.state == State::Committed {
            return Err(RawQueryError::refusal(
                "raw_transaction_already_committed",
                "the raw transaction has already committed",
            ));
        }
        if self.state == State::Unknown {
            return self.require_open();
        }
        self.close().await
    }

    /// Release this context without committing pending direct writes.
    pub async fn close(&mut self) -> Result<(), RawQueryError> {
        if !matches!(self.state, State::Committed | State::Unknown) {
            self.state = State::Closed;
        }
        match &mut self.route {
            Route::Direct(transaction) => match transaction.take() {
                Some(mut transaction) => transaction.close_raw().await,
                None => Ok(()),
            },
            Route::Proxy { .. } => Ok(()),
        }
    }

    fn require_open(&self) -> Result<(), RawQueryError> {
        match self.state {
            State::Open => Ok(()),
            State::Unknown => Err(RawQueryError::refusal(
                "commit_outcome_unknown",
                "the raw transaction outcome is unknown",
            )
            .unknown()),
            _ => Err(RawQueryError::refusal(
                "raw_transaction_closed",
                "the raw transaction is no longer open",
            )),
        }
    }
}

impl super::Database {
    /// Open a real eager raw transaction, with no schema gate or client limits.
    pub async fn raw_transaction(&self, kind: TxType) -> Result<RawTransaction, RawQueryError> {
        self.backend
            .open_raw_transaction(self.database_name(), kind)
            .await
            .map(|transaction| RawTransaction::direct(transaction, kind))
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
    pub async fn execute_query(&self, query: &str, kind: TxType) -> Result<Vec<u8>, RawQueryError> {
        let mut transaction = self.raw_transaction(kind).await?;
        let result: Result<Vec<u8>, RawQueryError> = async {
            let answer = transaction.execute_query(query).await?;
            transaction.commit().await?;
            Ok(answer)
        }
        .await;
        let closed = transaction.close().await;
        let answer = result?;
        closed?;
        Ok(answer)
    }
}

/// Decode only the proxy error envelope; successful raw bytes stay untouched.
pub(super) fn proxy_answer(
    response: type_bridge_core_lib::proxy_http::RawHttpResponse,
    kind: TxType,
) -> Result<Vec<u8>, RawQueryError> {
    if response.status == 200 {
        return Ok(response.body);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null);
    let code = value.get("error").and_then(serde_json::Value::as_str);
    let mut error = match code {
        Some("typedb_error") => {
            let provider = value.get("typedb");
            match (
                provider
                    .and_then(|p| p.get("code"))
                    .and_then(serde_json::Value::as_str),
                provider
                    .and_then(|p| p.get("message"))
                    .and_then(serde_json::Value::as_str),
            ) {
                (Some(code), Some(message)) => RawQueryError::refusal(code, message),
                _ => invalid_proxy_answer(kind),
            }
        }
        Some("commit_outcome_unknown") => RawQueryError::refusal(
            "commit_outcome_unknown",
            "the proxy cannot establish whether the raw query committed",
        )
        .unknown(),
        Some(code) => RawQueryError::refusal(code, code),
        None => invalid_proxy_answer(kind),
    };
    error.status = response.status;
    Err(error)
}

fn invalid_proxy_answer(kind: TxType) -> RawQueryError {
    let error = RawQueryError::refusal(
        "raw_proxy_response_invalid",
        "the raw proxy returned an invalid error response",
    );
    if kind == TxType::Read {
        error
    } else {
        error.unknown()
    }
}

#[cfg(test)]
mod tests {
    use super::super::backend::QueryResult;
    use super::*;
    use crate::OrmError;
    use std::sync::Mutex;

    struct Direct {
        events: Arc<Mutex<Vec<String>>>,
        unknown_commit: bool,
        pending_close: bool,
    }
    impl Drop for Direct {
        fn drop(&mut self) {
            self.events.lock().unwrap().push("drop".into());
        }
    }
    impl TransactionOps for Direct {
        fn query(&mut self, _: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            panic!("typed query path used")
        }
        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            panic!("typed commit path used")
        }
        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            panic!("typed rollback path used")
        }
        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            panic!("typed close path used")
        }
        fn query_raw<'a>(
            &'a mut self,
            query: &'a str,
        ) -> BoxFuture<'a, Result<Vec<u8>, RawQueryError>> {
            self.events.lock().unwrap().push(query.to_owned());
            Box::pin(async move { Ok(query.as_bytes().to_vec()) })
        }
        fn commit_raw(&mut self) -> BoxFuture<'_, Result<(), RawQueryError>> {
            self.events.lock().unwrap().push("commit".into());
            Box::pin(async move {
                if self.unknown_commit {
                    Err(RawQueryError::refusal("CONNECTION", "lost commit reply").unknown())
                } else {
                    Ok(())
                }
            })
        }
        fn close_raw(&mut self) -> BoxFuture<'_, Result<(), RawQueryError>> {
            self.events.lock().unwrap().push("close".into());
            Box::pin(async move {
                if self.pending_close {
                    std::future::pending::<()>().await;
                }
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn cancelled_close_releases_provider_and_leaves_context_closed() {
        let events = Arc::new(Mutex::new(vec![]));
        let mut transaction = RawTransaction::direct(
            Box::new(Direct {
                events: events.clone(),
                unknown_commit: false,
                pending_close: true,
            }),
            TxType::Write,
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), transaction.close())
                .await
                .is_err()
        );
        assert_eq!(events.lock().unwrap().as_slice(), ["close", "drop"]);
        assert_eq!(
            transaction.execute_query("again").await.unwrap_err().code,
            "raw_transaction_closed"
        );
        transaction.close().await.unwrap();
    }

    #[tokio::test]
    async fn direct_queries_are_eager_and_rollback_never_commits() {
        for kind in [TxType::Read, TxType::Write, TxType::Schema] {
            let events = Arc::new(Mutex::new(vec![]));
            let mut transaction = RawTransaction::direct(
                Box::new(Direct {
                    events: events.clone(),
                    unknown_commit: false,
                    pending_close: false,
                }),
                kind,
            );
            for query in ["  insert $x isa raw;  ", "define entity raw;"] {
                assert_eq!(
                    transaction.execute_query(query).await.unwrap(),
                    query.as_bytes()
                );
            }
            assert_eq!(events.lock().unwrap().len(), 2);
            transaction.rollback().await.unwrap();
            assert_eq!(
                events.lock().unwrap().as_slice(),
                [
                    "  insert $x isa raw;  ",
                    "define entity raw;",
                    "close",
                    "drop"
                ]
            );
            assert_eq!(
                transaction.execute_query("again").await.unwrap_err().code,
                "raw_transaction_closed"
            );
        }
    }

    #[tokio::test]
    async fn direct_commit_preserves_unknown_outcome_and_read_does_not_commit() {
        for kind in [TxType::Read, TxType::Write, TxType::Schema] {
            let events = Arc::new(Mutex::new(vec![]));
            let mut transaction = RawTransaction::direct(
                Box::new(Direct {
                    events: events.clone(),
                    unknown_commit: true,
                    pending_close: false,
                }),
                kind,
            );
            let result = transaction.commit().await;
            if kind == TxType::Read {
                result.unwrap();
                assert_eq!(events.lock().unwrap().as_slice(), ["close", "drop"]);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.code, "commit_outcome_unknown");
                assert_eq!(error.provider_code.as_deref(), Some("CONNECTION"));
                assert_eq!(error.message, "lost commit reply");
                assert_eq!(error.commit_outcome, Some(CommitFailureCertainty::Unknown));
                assert_eq!(
                    transaction.rollback().await.unwrap_err().code,
                    "commit_outcome_unknown"
                );
                assert_eq!(events.lock().unwrap().as_slice(), ["commit", "drop"]);
            }
        }
    }

    #[tokio::test]
    async fn proxy_runs_once_with_exact_query_and_no_completion_io() {
        for (kind, wire_kind) in [
            (TxType::Read, "read"),
            (TxType::Write, "write"),
            (TxType::Schema, "schema"),
        ] {
            let requests = Arc::new(Mutex::new(vec![]));
            let captured = requests.clone();
            let exchange: RawExchangeFn = Arc::new(move |request| {
                captured.lock().unwrap().push(request);
                Box::pin(async { Ok(b"exact provider answer".to_vec()) })
            });
            let mut transaction =
                RawTransaction::proxy(exchange, kind, std::num::NonZeroU64::new(60_000).unwrap());
            let query = "  insert $x isa raw;\n";
            assert_eq!(
                transaction.execute_query(query).await.unwrap(),
                b"exact provider answer"
            );
            assert_eq!(
                transaction.execute_query("second").await.unwrap_err().code,
                "raw_transaction_single_query"
            );
            if kind == TxType::Read {
                transaction.commit().await.unwrap();
                transaction.rollback().await.unwrap();
            } else {
                transaction.commit().await.unwrap();
                assert_eq!(
                    transaction.rollback().await.unwrap_err().code,
                    "raw_transaction_already_committed"
                );
            }
            transaction.close().await.unwrap();
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let request: serde_json::Value = serde_json::from_slice(&requests[0]).unwrap();
            assert_eq!(request.as_object().unwrap().len(), 4);
            assert_eq!(request["query"], query);
            assert_eq!(request["kind"], wire_kind);
            assert_eq!(request["nonce"].as_str().unwrap().len(), 32);
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let expiry = request["expires_at_unix_ms"].as_u64().unwrap();
            assert!(expiry > now && expiry <= now + 60_000);
        }
    }

    #[tokio::test]
    async fn undecodable_success_preserves_proxy_write_uncertainty() {
        for kind in [TxType::Read, TxType::Write, TxType::Schema] {
            let exchange: RawExchangeFn = Arc::new(move |_| {
                Box::pin(async move {
                    proxy_answer(
                        type_bridge_core_lib::proxy_http::RawHttpResponse {
                            status: 200,
                            body: b"truncated{".to_vec(),
                        },
                        kind,
                    )
                })
            });
            let mut proxy =
                RawTransaction::proxy(exchange, kind, std::num::NonZeroU64::new(60_000).unwrap());
            let bytes = proxy.execute_query("unchanged text").await.unwrap();
            let error = proxy.decode_answer(&bytes).unwrap_err();
            assert_eq!(error.status, 502);
            if kind == TxType::Read {
                assert_eq!(error.code, "raw_answer_encoding");
                assert_eq!(error.commit_outcome, None);
            } else {
                assert_eq!(error.code, "commit_outcome_unknown");
                assert_eq!(error.provider_code.as_deref(), Some("raw_answer_encoding"));
                assert_eq!(error.commit_outcome, Some(CommitFailureCertainty::Unknown));
                assert_eq!(
                    proxy.commit().await.unwrap_err().code,
                    "commit_outcome_unknown"
                );
                assert_eq!(
                    proxy.rollback().await.unwrap_err().code,
                    "commit_outcome_unknown"
                );
            }
            assert_eq!(
                proxy.execute_query("retry").await.unwrap_err().code,
                "raw_transaction_single_query"
            );
            let mut direct = RawTransaction::direct(
                Box::new(Direct {
                    events: Arc::new(Mutex::new(vec![])),
                    unknown_commit: false,
                    pending_close: false,
                }),
                kind,
            );
            let error = direct.decode_answer(&bytes).unwrap_err();
            assert_eq!(error.code, "raw_answer_encoding");
            assert_eq!(error.commit_outcome, None);
        }
    }

    #[tokio::test]
    async fn proxy_failure_and_cancelled_dispatch_cannot_be_retried() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for cancelled in [false, true] {
            let called = calls.clone();
            let exchange: RawExchangeFn = Arc::new(move |_| {
                called.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Box::pin(async move {
                    if cancelled {
                        std::future::pending::<()>().await;
                    }
                    Err(RawQueryError::refusal("raw_disabled", "raw_disabled"))
                })
            });
            let mut transaction = RawTransaction::proxy(
                exchange,
                TxType::Write,
                std::num::NonZeroU64::new(60_000).unwrap(),
            );
            if cancelled {
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(10),
                        transaction.execute_query("write")
                    )
                    .await
                    .is_err()
                );
                assert_eq!(
                    transaction.rollback().await.unwrap_err().code,
                    "commit_outcome_unknown"
                );
            } else {
                assert_eq!(
                    transaction.execute_query("write").await.unwrap_err().code,
                    "raw_disabled"
                );
                transaction.rollback().await.unwrap();
            }
            assert_eq!(
                transaction.execute_query("again").await.unwrap_err().code,
                "raw_transaction_single_query"
            );
        }
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn proxy_errors_keep_provider_details_and_durability() {
        use type_bridge_core_lib::proxy_http::RawHttpResponse;
        for (body, code, message, certainty) in [
            (
                r#"{"error":"raw_disabled"}"#,
                "raw_disabled",
                "raw_disabled",
                None,
            ),
            (
                r#"{"error":"typedb_error","typedb":{"code":"TQL01","message":"exact TypeDB message"}}"#,
                "TQL01",
                "exact TypeDB message",
                None,
            ),
            (
                r#"{"error":"commit_outcome_unknown"}"#,
                "commit_outcome_unknown",
                "the proxy cannot establish whether the raw query committed",
                Some(CommitFailureCertainty::Unknown),
            ),
        ] {
            let error = proxy_answer(
                RawHttpResponse {
                    status: 503,
                    body: body.as_bytes().to_vec(),
                },
                TxType::Write,
            )
            .unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(error.message, message);
            assert_eq!(error.commit_outcome, certainty);
            assert_eq!(error.status, 503);
        }
        let malformed = proxy_answer(
            RawHttpResponse {
                status: 502,
                body: b"bad gateway".to_vec(),
            },
            TxType::Write,
        )
        .unwrap_err();
        assert_eq!(malformed.code, "commit_outcome_unknown");
        assert_eq!(
            malformed.commit_outcome,
            Some(CommitFailureCertainty::Unknown)
        );
    }
}

#[cfg(all(test, feature = "typedb"))]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires an explicitly named disposable live TypeDB database"]
    async fn raw_direct_live_multiple_queries_rollback_and_schema() {
        let database = std::env::var("TYPE_BRIDGE_RAW_TEST_DATABASE").expect("explicit database");
        let address = std::env::var("TYPEDB_ADDRESS").expect("explicit TypeDB address");
        let http_port = std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap();
        let db = super::super::Database::connect_with_options(
            &address,
            &database,
            "admin",
            "password",
            crate::ConnectOptions {
                http_port,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            !db.database_exists().await.unwrap(),
            "test database already exists"
        );
        db.create_database().await.unwrap();
        let result: Result<(), RawQueryError> = async {
            let schema = db
                .execute_query("define entity raw-test;", TxType::Schema)
                .await?;
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&schema).unwrap()["queryType"],
                "schema"
            );
            let mut transaction = db.raw_transaction(TxType::Write).await?;
            for _ in 0..2 {
                transaction.execute_query("insert $x isa raw-test;").await?;
            }
            let inside = transaction
                .execute_query("match $x isa raw-test; fetch { 'iid': iid($x) };")
                .await?;
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&inside).unwrap()["answers"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            transaction.rollback().await?;
            let outside = db
                .execute_query(
                    "match $x isa raw-test; fetch { 'iid': iid($x) };",
                    TxType::Read,
                )
                .await?;
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&outside).unwrap()["answers"],
                serde_json::json!([])
            );
            let inserted = db
                .execute_query("insert $x isa raw-test;", TxType::Write)
                .await?;
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&inserted).unwrap()["queryType"],
                "write"
            );
            let outside = db
                .execute_query(
                    "match $x isa raw-test; fetch { 'iid': iid($x) };",
                    TxType::Read,
                )
                .await?;
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&outside).unwrap()["answers"]
                    .as_array()
                    .unwrap()
                    .len(),
                1
            );
            Ok(())
        }
        .await;
        db.delete_database().await.unwrap();
        assert!(!db.database_exists().await.unwrap());
        result.unwrap();
    }
}
