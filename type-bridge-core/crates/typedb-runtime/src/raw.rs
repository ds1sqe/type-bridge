//! Raw, unchecked TypeQL with TypeDB's HTTP query-answer JSON encoding.

use futures::TryStreamExt;
use serde_json::{Value, json};
use typedb_driver::answer::{QueryAnswer, QueryType};
use typedb_driver::concept::Concept;

use crate::{
    RuntimeTransaction, RuntimeTransactionInner, ScalarJsonEncoding, TxType, TypeDBRuntime,
    document_to_json_b9, value_to_json_b9,
};

/// Provider error returned by an unchecked raw query or its transaction.
#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct RawQueryError {
    /// TypeDB's diagnostic code, without display brackets.
    pub code: String,
    /// TypeDB's diagnostic message, without TypeBridge rewriting.
    pub message: String,
    /// Suggested HTTP response status for the provider failure.
    pub status: u16,
    /// Durability certainty when the provider rejected or lost a commit.
    /// `None` means the error occurred before commit was sent.
    pub commit_outcome: Option<crate::CommitFailureCertainty>,
}

impl RawQueryError {
    fn driver(error: typedb_driver::Error) -> Self {
        let status = match &error {
            typedb_driver::Error::Server(_) | typedb_driver::Error::Query(_) => 400,
            typedb_driver::Error::Connection(_) => 503,
            _ => 500,
        };
        Self {
            code: error.code().trim_matches(['[', ']']).to_owned(),
            message: error.message(),
            status,
            commit_outcome: None,
        }
    }

    fn commit(error: typedb_driver::Error) -> Self {
        let failure = crate::failure::classify_commit(
            &error.code(),
            &error.to_string(),
            matches!(&error, typedb_driver::Error::Server(_)),
        );
        let certainty = if failure.retry_safe() {
            crate::CommitFailureCertainty::DefinitelyAborted
        } else {
            crate::CommitFailureCertainty::Unknown
        };
        let mut error = Self::driver(error);
        error.commit_outcome = Some(certainty);
        error
    }

    fn runtime(error: crate::RuntimeError) -> Self {
        Self {
            code: "runtime_error".into(),
            message: error.to_string(),
            status: 503,
            commit_outcome: None,
        }
    }
}

/// Optional host bounds for unchecked TypeQL. They do not bound item count.
#[derive(Clone, Copy, Debug, Default)]
pub struct RawExecutionOptions {
    /// Absolute deadline for opening, querying and preparing to commit.
    /// A commit already sent is always awaited to its actual outcome.
    pub deadline: Option<std::time::Instant>,
    /// Maximum complete HTTP-answer JSON bytes, checked before any commit.
    pub max_answer_bytes: Option<usize>,
}

fn deadline_error() -> RawQueryError {
    RawQueryError {
        code: "transaction_deadline_exceeded".into(),
        message: "raw execution deadline expired before commit".into(),
        status: 504,
        commit_outcome: None,
    }
}

fn check_deadline(deadline: Option<std::time::Instant>) -> Result<(), RawQueryError> {
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        Err(deadline_error())
    } else {
        Ok(())
    }
}

async fn before_deadline<T>(
    deadline: Option<std::time::Instant>,
    work: impl std::future::Future<Output = Result<T, RawQueryError>>,
) -> Result<T, RawQueryError> {
    check_deadline(deadline)?;
    match deadline {
        Some(deadline) => tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), work)
            .await
            .map_err(|_| deadline_error())?,
        None => work.await,
    }
}

impl TypeDBRuntime {
    /// Raw, unchecked TypeQL: the escape hatch. Sends the TypeQL text to
    /// TypeDB as written and returns its HTTP query-answer JSON bytes.
    /// TypeBridge does not parse, validate, rewrite, limit or hydrate it,
    /// runs no hooks, and makes no promise about it: it can run any query
    /// in any transaction kind, including schema changes that break your
    /// generated models. Use a typed call or a plan for anything TypeBridge
    /// should check. Query structure is disabled; column bindings are retained.
    pub async fn execute_raw(
        &self,
        database: &str,
        query: &str,
        kind: TxType,
    ) -> Result<Vec<u8>, RawQueryError> {
        self.execute_raw_with_options(database, query, kind, RawExecutionOptions::default())
            .await
    }

    /// Open an eager unchecked transaction, preserving TypeDB error details.
    /// The caller may execute multiple raw queries before committing or closing.
    /// No client deadline, schema gate or query limit is added.
    pub async fn open_raw_transaction(
        &self,
        database: &str,
        kind: TxType,
    ) -> Result<RuntimeTransaction, RawQueryError> {
        self.open_raw_with_options(database, kind, &crate::TransactionOptions::new())
            .await
    }

    async fn open_raw_with_options(
        &self,
        database: &str,
        kind: TxType,
        options: &crate::TransactionOptions,
    ) -> Result<RuntimeTransaction, RawQueryError> {
        self.open_transaction_preserving_error(database, kind, options)
            .await
            .map_err(|error| match error {
                crate::TransactionOpenError::Runtime(error) => RawQueryError::runtime(error),
                crate::TransactionOpenError::Driver(error) => RawQueryError::driver(error),
            })
    }

    /// Execute unchecked TypeQL with host deadline and answer-byte bounds.
    /// Open/query timeouts and oversized answers close without committing.
    /// No deadline can cancel a commit already sent to the provider.
    pub async fn execute_raw_with_options(
        &self,
        database: &str,
        query: &str,
        kind: TxType,
        options: RawExecutionOptions,
    ) -> Result<Vec<u8>, RawQueryError> {
        check_deadline(options.deadline)?;
        let timeout = options.deadline.map(|deadline| {
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .max(std::time::Duration::from_millis(1))
        });
        let transaction_options = crate::TransactionOptions {
            transaction_timeout: timeout,
            schema_lock_acquire_timeout: timeout,
        };
        let open = self.open_raw_with_options(database, kind, &transaction_options);
        let mut transaction = before_deadline(options.deadline, open).await?;
        let result = async {
            let answer = before_deadline(
                options.deadline,
                transaction.query_raw_bounded(query, options.max_answer_bytes),
            )
            .await?;
            check_deadline(options.deadline)?;
            if kind != TxType::Read {
                transaction.commit_raw().await?;
            }
            Ok(answer)
        }
        .await;
        let closed = transaction.close_raw().await;
        let answer = result?;
        closed?;
        Ok(answer)
    }
}

impl RuntimeTransaction {
    /// Raw, unchecked TypeQL in this transaction, returning TypeDB HTTP
    /// query-answer JSON bytes. No parsing, validation, rewriting, limits,
    /// hydration or hooks run. The caller owns correctness and transaction
    /// completion, including schema changes that break generated models.
    pub async fn query_raw(&mut self, query: &str) -> Result<Vec<u8>, RawQueryError> {
        self.query_raw_bounded(query, None).await
    }

    async fn query_raw_bounded(
        &mut self,
        query: &str,
        maximum: Option<usize>,
    ) -> Result<Vec<u8>, RawQueryError> {
        self.ensure_driver_open().map_err(RawQueryError::runtime)?;
        let RuntimeTransactionInner::B9(transaction) = &self.inner;
        let transaction = transaction.as_ref().ok_or_else(|| RawQueryError {
            code: "transaction_closed".into(),
            message: "transaction already consumed".into(),
            status: 400,
            commit_outcome: None,
        })?;
        let answer = transaction
            .query_with_options(
                query,
                typedb_driver::QueryOptions::new().include_query_structure(false),
            )
            .await
            .map_err(RawQueryError::driver)?;
        http_answer_bytes(answer, maximum).await
    }

    /// Close without committing, preserving the provider error. Repeated close is harmless.
    pub async fn close_raw(&mut self) -> Result<(), RawQueryError> {
        let RuntimeTransactionInner::B9(transaction) = &mut self.inner;
        let Some(transaction) = transaction.take() else {
            return Ok(());
        };
        if Self::driver_shutdown_started(&self.driver_lease) {
            drop(transaction);
            return Ok(());
        }
        transaction.close().await.map_err(RawQueryError::driver)
    }

    /// Commit unchecked writes, preserving provider details and durability certainty.
    pub async fn commit_raw(&mut self) -> Result<(), RawQueryError> {
        self.ensure_driver_open().map_err(RawQueryError::runtime)?;
        let RuntimeTransactionInner::B9(transaction) = &mut self.inner;
        let transaction = transaction.take().ok_or_else(|| RawQueryError {
            code: "transaction_closed".into(),
            message: "transaction already consumed".into(),
            status: 400,
            commit_outcome: None,
        })?;
        transaction.commit().await.map_err(RawQueryError::commit)
    }
}

struct AnswerBytes {
    bytes: Vec<u8>,
    maximum: usize,
    exceeded: bool,
}

impl std::io::Write for AnswerBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.maximum.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("raw answer byte ceiling exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl AnswerBytes {
    fn new(maximum: Option<usize>) -> Self {
        Self {
            bytes: Vec::new(),
            maximum: maximum.unwrap_or(usize::MAX),
            exceeded: false,
        }
    }
    fn error(&self, message: String) -> RawQueryError {
        RawQueryError {
            code: if self.exceeded {
                "raw_answer_byte_limit"
            } else {
                "raw_answer_encoding"
            }
            .into(),
            message,
            status: if self.exceeded { 413 } else { 500 },
            commit_outcome: None,
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Result<(), RawQueryError> {
        std::io::Write::write_all(self, bytes).map_err(|error| self.error(error.to_string()))
    }
    fn json(&mut self, value: &Value) -> Result<(), RawQueryError> {
        serde_json::to_writer(&mut *self, value).map_err(|error| self.error(error.to_string()))
    }
}

async fn http_answer_bytes(
    answer: QueryAnswer,
    maximum: Option<usize>,
) -> Result<Vec<u8>, RawQueryError> {
    let query_type = match answer.get_query_type() {
        QueryType::ReadQuery => "read",
        QueryType::WriteQuery => "write",
        QueryType::SchemaQuery => "schema",
    };
    let mut output = AnswerBytes::new(maximum);
    let mut first = true;
    match answer {
        QueryAnswer::Ok(_) => {
            output.json(&json!({"queryType":query_type,"answerType":"ok","answers":null,"query":null,"warning":null}))?;
            return Ok(output.bytes);
        }
        QueryAnswer::ConceptRowStream(header, mut stream) => {
            output.push(br#"{"answerType":"conceptRows","answers":["#)?;
            while let Some(row) = stream.try_next().await.map_err(RawQueryError::driver)? {
                let data = header
                    .column_names
                    .iter()
                    .zip(&row.row)
                    .map(|(name, concept)| {
                        let value = concept
                            .as_ref()
                            .map(http_concept)
                            .transpose()?
                            .unwrap_or(Value::Null);
                        Ok((name.clone(), value))
                    })
                    .collect::<Result<serde_json::Map<_, _>, RawQueryError>>()?;
                if !first {
                    output.push(b",")?;
                }
                first = false;
                output.json(&json!({"data":data,"involvedBlocks":null}))?;
            }
        }
        QueryAnswer::ConceptDocumentStream(_, mut stream) => {
            output.push(br#"{"answerType":"conceptDocuments","answers":["#)?;
            let mut members = 0;
            while let Some(document) = stream.try_next().await.map_err(RawQueryError::driver)? {
                let document = document_to_json_b9(
                    &document,
                    &mut members,
                    u64::MAX,
                    ScalarJsonEncoding::DriverDisplay,
                )
                .map_err(RawQueryError::runtime)?;
                if !first {
                    output.push(b",")?;
                }
                first = false;
                output.json(&document)?;
            }
        }
    }
    output.push(br#"],"query":null,"queryType":""#)?;
    output.push(query_type.as_bytes())?;
    output.push(br#"","warning":null}"#)?;
    Ok(output.bytes)
}

#[cfg(test)]
async fn http_answer(answer: QueryAnswer) -> Result<Value, RawQueryError> {
    let bytes = http_answer_bytes(answer, None).await?;
    Ok(serde_json::from_slice(&bytes).expect("raw answer is complete JSON"))
}

fn http_concept(concept: &Concept) -> Result<Value, RawQueryError> {
    let type_value = |kind: &str, label: &str| json!({"kind":kind,"label":label});
    let attribute_type = |attribute: &typedb_driver::concept::AttributeType| json!({"kind":"attributeType","label":attribute.label,"valueType":attribute.value_type.as_ref().map(|value|value.name())});
    let value = match concept {
        Concept::EntityType(t) => type_value("entityType", &t.label),
        Concept::RelationType(t) => type_value("relationType", &t.label),
        Concept::RoleType(t) => type_value("roleType", &t.label),
        Concept::AttributeType(t) => attribute_type(t),
        Concept::Entity(entity) => {
            json!({"kind":"entity","iid":entity.iid.to_string(),"type":entity.type_.as_ref().map(|t|type_value("entityType",&t.label))})
        }
        Concept::Relation(relation) => {
            json!({"kind":"relation","iid":relation.iid.to_string(),"type":relation.type_.as_ref().map(|t|type_value("relationType",&t.label))})
        }
        Concept::Attribute(attribute) => {
            json!({"kind":"attribute","iid":attribute.iid.to_string(),"type":attribute.type_.as_ref().map(attribute_type),"valueType":attribute.value.get_type_name(),"value":value_to_json_b9(&attribute.value,ScalarJsonEncoding::DriverDisplay).map_err(RawQueryError::runtime)?})
        }
        Concept::Value(value) => {
            json!({"kind":"value","valueType":value.get_type_name(),"value":value_to_json_b9(value,ScalarJsonEncoding::DriverDisplay).map_err(RawQueryError::runtime)?})
        }
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use typedb_driver::answer::concept_row::{ConceptRow, ConceptRowHeader};
    use typedb_driver::concept::{Entity, EntityType};

    #[tokio::test]
    async fn precommit_deadline_drops_pending_work() {
        struct DropNotice(std::sync::Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropNotice {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let notice = DropNotice(std::sync::Arc::clone(&dropped));
        let work = async move {
            let _notice = notice;
            std::future::pending::<Result<(), RawQueryError>>().await
        };
        let result = before_deadline(
            Some(std::time::Instant::now() + std::time::Duration::from_millis(10)),
            work,
        )
        .await
        .unwrap_err();
        assert_eq!(result.code, "transaction_deadline_exceeded");
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn raw_answer_byte_ceiling_counts_the_complete_json_without_truncation() {
        let full = http_answer_bytes(QueryAnswer::Ok(QueryType::SchemaQuery), None)
            .await
            .unwrap();
        assert_eq!(
            http_answer_bytes(QueryAnswer::Ok(QueryType::SchemaQuery), Some(full.len()))
                .await
                .unwrap(),
            full
        );
        let error = http_answer_bytes(
            QueryAnswer::Ok(QueryType::SchemaQuery),
            Some(full.len() - 1),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "raw_answer_byte_limit");
        assert_eq!(error.commit_outcome, None);
        let mut output = AnswerBytes::new(Some(20));
        output
            .json(&json!({"x": "012345678901234567890123456789"}))
            .unwrap_err();
        assert!(output.bytes.len() <= 20);
    }

    #[tokio::test]
    async fn answer_stream_stops_at_the_byte_ceiling_without_an_item_limit() {
        use futures::StreamExt;
        let consumed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let make_answer = || {
            let header = Arc::new(ConceptRowHeader {
                column_names: vec!["x".into()],
                query_type: QueryType::ReadQuery,
                query_structure: None,
            });
            let row_header = Arc::clone(&header);
            let consumed = Arc::clone(&consumed);
            QueryAnswer::ConceptRowStream(
                header,
                Box::pin(
                    futures::stream::iter(0..1000)
                        .map(move |n| {
                            consumed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            ConceptRow::new(
                                Arc::clone(&row_header),
                                vec![Some(Concept::Value(
                                    typedb_driver::concept::Value::Integer(n),
                                ))],
                                None,
                            )
                        })
                        .map(Ok),
                ),
            )
        };
        let error = http_answer_bytes(make_answer(), Some(256))
            .await
            .unwrap_err();
        assert_eq!(error.code, "raw_answer_byte_limit");
        assert!(consumed.load(std::sync::atomic::Ordering::SeqCst) < 1000);
        let unlimited = http_answer_bytes(make_answer(), None).await.unwrap();
        let unlimited: Value = serde_json::from_slice(&unlimited).unwrap();
        assert_eq!(unlimited["answers"].as_array().unwrap().len(), 1000);
    }

    #[test]
    fn connection_dropped_while_committing_keeps_unknown_durability() {
        let driver = typedb_driver::Error::from(tonic::Status::unavailable(
            "connection dropped after commit send",
        ));
        let code = driver.code();
        let message = driver.message();
        let error = RawQueryError::commit(driver);
        assert_eq!(
            error.commit_outcome,
            Some(crate::CommitFailureCertainty::Unknown)
        );
        assert_eq!(error.code, code);
        assert_eq!(error.message, message);
    }

    #[tokio::test]
    async fn http_answer_matches_documented_shape_without_normalizing_concepts() {
        // https://typedb.com/docs/reference/typedb-http-api/#one-shot-query
        let header = Arc::new(ConceptRowHeader {
            column_names: vec!["person".into(), "value".into(), "missing".into()],
            query_type: QueryType::ReadQuery,
            query_structure: None,
        });
        let row = ConceptRow::new(
            Arc::clone(&header),
            vec![
                Some(Concept::Entity(Entity {
                    iid: vec![0x1e, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1].into(),
                    type_: Some(EntityType {
                        label: "person".into(),
                    }),
                })),
                Some(Concept::Value(typedb_driver::concept::Value::Integer(
                    9_007_199_254_740_993,
                ))),
                None,
            ],
            None,
        );
        let answer = http_answer(QueryAnswer::ConceptRowStream(
            header,
            Box::pin(futures::stream::iter([Ok(row)])),
        ))
        .await
        .unwrap();
        assert_eq!(
            answer,
            json!({"queryType":"read","answerType":"conceptRows","answers":[{"data":{"person":{"kind":"entity","iid":"0x1e00000000000000000001","type":{"kind":"entityType","label":"person"}},"value":{"kind":"value","valueType":"integer","value":9_007_199_254_740_993_i64},"missing":null},"involvedBlocks":null}],"query":null,"warning":null})
        );
        assert_eq!(
            http_answer(QueryAnswer::Ok(QueryType::SchemaQuery))
                .await
                .unwrap(),
            json!({"queryType":"schema","answerType":"ok","answers":null,"query":null,"warning":null})
        );
    }
}
