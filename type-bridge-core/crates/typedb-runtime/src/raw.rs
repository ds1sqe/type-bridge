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
        }
    }

    fn runtime(error: crate::RuntimeError) -> Self {
        Self {
            code: "runtime_error".into(),
            message: error.to_string(),
            status: 503,
        }
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
        let mut transaction = self
            .open_transaction_preserving_error(
                database,
                kind,
                &crate::TransactionOptions::default(),
            )
            .await
            .map_err(|error| match error {
                crate::TransactionOpenError::Runtime(error) => RawQueryError::runtime(error),
                crate::TransactionOpenError::Driver(error) => RawQueryError::driver(error),
            })?;
        let result = async {
            let answer = transaction.query_raw(query).await?;
            if kind != TxType::Read {
                transaction.commit_raw().await?;
            }
            Ok(answer)
        }
        .await;
        let closed = transaction.close().await;
        let answer = result?;
        closed.map_err(RawQueryError::runtime)?;
        Ok(answer)
    }
}

impl RuntimeTransaction {
    /// Raw, unchecked TypeQL in this transaction, returning TypeDB HTTP
    /// query-answer JSON bytes. No parsing, validation, rewriting, limits,
    /// hydration or hooks run. The caller owns correctness and transaction
    /// completion, including schema changes that break generated models.
    pub async fn query_raw(&mut self, query: &str) -> Result<Vec<u8>, RawQueryError> {
        self.ensure_driver_open().map_err(RawQueryError::runtime)?;
        let RuntimeTransactionInner::B9(transaction) = &self.inner;
        let transaction = transaction.as_ref().ok_or_else(|| RawQueryError {
            code: "transaction_closed".into(),
            message: "transaction already consumed".into(),
            status: 400,
        })?;
        let answer = transaction
            .query_with_options(
                query,
                typedb_driver::QueryOptions::new().include_query_structure(false),
            )
            .await
            .map_err(RawQueryError::driver)?;
        let answer = http_answer(answer).await?;
        serde_json::to_vec(&answer).map_err(|error| RawQueryError {
            code: "raw_answer_encoding".into(),
            message: error.to_string(),
            status: 500,
        })
    }

    async fn commit_raw(&mut self) -> Result<(), RawQueryError> {
        self.ensure_driver_open().map_err(RawQueryError::runtime)?;
        let RuntimeTransactionInner::B9(transaction) = &mut self.inner;
        let transaction = transaction.take().ok_or_else(|| RawQueryError {
            code: "transaction_closed".into(),
            message: "transaction already consumed".into(),
            status: 400,
        })?;
        transaction.commit().await.map_err(RawQueryError::driver)
    }
}

async fn http_answer(answer: QueryAnswer) -> Result<Value, RawQueryError> {
    let query_type = match answer.get_query_type() {
        QueryType::ReadQuery => "read",
        QueryType::WriteQuery => "write",
        QueryType::SchemaQuery => "schema",
    };
    let (answer_type, answers) = match answer {
        QueryAnswer::Ok(_) => ("ok", Value::Null),
        QueryAnswer::ConceptRowStream(header, mut stream) => {
            let mut answers = Vec::new();
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
                answers.push(json!({"data":data,"involvedBlocks":null}));
            }
            ("conceptRows", Value::Array(answers))
        }
        QueryAnswer::ConceptDocumentStream(_, mut stream) => {
            let mut answers = Vec::new();
            let mut members = 0;
            while let Some(document) = stream.try_next().await.map_err(RawQueryError::driver)? {
                answers.push(
                    document_to_json_b9(
                        &document,
                        &mut members,
                        u64::MAX,
                        ScalarJsonEncoding::DriverDisplay,
                    )
                    .map_err(RawQueryError::runtime)?,
                );
            }
            ("conceptDocuments", Value::Array(answers))
        }
    };
    Ok(
        json!({"queryType":query_type,"answerType":answer_type,"answers":answers,"query":null,"warning":null}),
    )
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
