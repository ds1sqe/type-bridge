//! Native data plans over the canonical legacy migration-state schema.
use super::schema::{labels::*, migration_state_schema};
use crate::{MigrationError, Result};
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    AttachItem, DeleteItem, InsertThing, Operation, Pattern, PlanSpec, QueryPlan, Stage,
    ThingDocument, WriteReturns,
};
use type_bridge_contract::query_plan::{QueryOperand, QueryOutput, QueryPattern};
use type_bridge_contract::schema::{DeclaredSchema, DocumentId, OwnsFactId};
use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
use type_bridge_contract::value::{CanonicalString, CanonicalValue};

fn diagnostic(error: Diagnostic) -> MigrationError {
    MigrationError::State {
        message: error.to_string(),
    }
}
pub(super) fn text(value: &str) -> Result<CanonicalValue> {
    Ok(CanonicalValue::String(
        CanonicalString::new(value).map_err(diagnostic)?,
    ))
}
pub(super) fn datetime(value: &str) -> Result<CanonicalValue> {
    Ok(CanonicalValue::DateTime(
        type_bridge_schema::parse_provider_datetime(value).map_err(diagnostic)?,
    ))
}
fn binding(index: u16) -> BindingId {
    BindingId::new(index).expect("bounded migration bindings")
}
fn operand(value: CanonicalValue) -> QueryOperand {
    QueryOperand::Literal { value }
}
fn owns(kind: &str, field: &str) -> Result<OwnsFactId> {
    OwnsFactId::new(
        TypeId::new(TypeKind::Entity, kind).map_err(diagnostic)?,
        AttributeId::new(field).map_err(diagnostic)?,
    )
    .map_err(diagnostic)
}
static AUTHORITY: std::sync::LazyLock<
    std::result::Result<(DeclaredSchema, ManagedSemanticSchemaFingerprint), String>,
> = std::sync::LazyLock::new(|| {
    let schema_text = migration_state_schema()
        .to_typeql()
        .map_err(|error| error.to_string())?;
    let schema = type_bridge_schema_compat::typeql_to_declared(
        DocumentId::new("migration-state-plan.typeql").map_err(|error| error.to_string())?,
        &schema_text,
    )
    .map_err(|error| error.to_string())?;
    let semantics = ManagedSemanticSchemaFingerprint::compute(
        SemanticProfileId::new("typedb-3.12.1/v1").map_err(|error| error.to_string())?,
        schema_text.as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    Ok((schema, semantics))
});

fn build(
    bindings: Vec<AssertionBinding>,
    pipeline: Vec<Stage>,
    operation: Operation,
) -> Result<QueryPlan> {
    let (schema, semantics) = AUTHORITY
        .as_ref()
        .map_err(|message| MigrationError::State {
            message: message.clone(),
        })?;
    QueryPlan::new(
        PlanSpec {
            bindings,
            inputs: vec![],
            given_rows: None,
            operation,
            pipeline,
            output: QueryOutput::Rows { columns: vec![] },
            managed_semantics: semantics.clone(),
            functions: vec![],
        },
        schema,
    )
    .map_err(diagnostic)
}
fn root() -> AssertionBinding {
    AssertionBinding::new(
        binding(0),
        QueryVariable::new("migration").expect("static variable"),
    )
}
fn matches(kind: &str, fixed: &[(&str, &str)]) -> Result<(Vec<AssertionBinding>, Vec<Pattern>)> {
    let mut bindings = vec![root()];
    let mut patterns = vec![
        QueryPattern::Isa {
            binding: binding(0),
            type_id: TypeId::new(TypeKind::Entity, kind).map_err(diagnostic)?,
            include_subtypes: false,
        }
        .into(),
    ];
    for (index, (field, value)) in fixed.iter().enumerate() {
        let slot = u16::try_from(index + 1).map_err(|_| MigrationError::State {
            message: "migration predicate limit".into(),
        })?;
        bindings.push(AssertionBinding::new(
            binding(slot),
            QueryVariable::new(format!("predicate{slot}")).map_err(diagnostic)?,
        ));
        patterns.extend([
            QueryPattern::Has {
                attribute: binding(slot),
                owner: binding(0),
                attribute_id: AttributeId::new(*field).map_err(diagnostic)?,
            }
            .into(),
            QueryPattern::Value {
                comparator: ValueComparator::Equal,
                left: QueryOperand::Binding {
                    binding: binding(slot),
                },
                right: operand(text(value)?),
            }
            .into(),
        ]);
    }
    Ok((bindings, patterns))
}
pub(super) fn read(kind: &str, fixed: &[(&str, &str)]) -> Result<QueryPlan> {
    let (bindings, patterns) = matches(kind, fixed)?;
    build(
        bindings,
        vec![
            Stage::Match { patterns },
            Stage::Fetch(ThingDocument::new(binding(0), vec![])),
        ],
        Operation::Rows,
    )
}
pub(super) fn delete(kind: &str, fixed: &[(&str, &str)]) -> Result<QueryPlan> {
    let (bindings, patterns) = matches(kind, fixed)?;
    build(
        bindings,
        vec![
            Stage::Match { patterns },
            Stage::Delete {
                items: vec![DeleteItem::Thing {
                    binding: binding(0),
                }],
            },
        ],
        Operation::Write {
            returns: WriteReturns::Matched,
        },
    )
}
pub(super) fn write(
    kind: &str,
    fields: Vec<(&str, CanonicalValue)>,
    replace: bool,
) -> Result<QueryPlan> {
    let type_id = TypeId::new(TypeKind::Entity, kind).map_err(diagnostic)?;
    let key = if kind == APPLIED_ENTITY {
        MIGRATION_ID
    } else {
        RUN_ID
    };
    let (keys, attributes): (Vec<_>, Vec<_>) =
        fields.into_iter().partition(|(field, _)| *field == key);
    let convert = |fields: Vec<(&str, CanonicalValue)>| {
        fields
            .into_iter()
            .map(|(field, value)| Ok((owns(kind, field)?, operand(value))))
            .collect::<Result<Vec<_>>>()
    };
    let mut pipeline = vec![if replace {
        Stage::Put {
            include_subtypes: false,
            things: vec![InsertThing {
                binding: binding(0),
                type_id: type_id.clone(),
                fields: convert(keys)?,
                players: vec![],
            }],
        }
    } else {
        let mut fields = keys;
        fields.extend(attributes.clone());
        Stage::Insert {
            things: vec![InsertThing {
                binding: binding(0),
                type_id: type_id.clone(),
                fields: convert(fields)?,
                players: vec![],
            }],
        }
    }];
    if replace {
        let owned = &migration_state_schema().entities[kind].owned_attributes;
        pipeline.push(Stage::Delete {
            items: owned
                .iter()
                .filter(|field| field.attr_name != key)
                .map(|field| {
                    Ok(DeleteItem::Has {
                        owner: binding(0),
                        field: owns(kind, &field.attr_name)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        });
        pipeline.push(Stage::Attach {
            items: convert(attributes)?
                .into_iter()
                .map(|(field, operand)| AttachItem::Has {
                    optional: false,
                    owner: binding(0),
                    field,
                    operand,
                })
                .collect(),
        });
    }
    build(
        vec![root()],
        pipeline,
        Operation::Write {
            returns: WriteReturns::Matched,
        },
    )
}

pub(super) fn run(record: &super::MigrationRunRecord) -> Result<QueryPlan> {
    let mut fields = vec![
        (RUN_ID, text(&record.run_id)?),
        (APP_LABEL, text(&record.app_label)?),
        (NAME, text(&record.name)?),
        (CHECKSUM, text(&record.checksum)?),
        (DIRECTION, text(&record.direction)?),
        (STATUS, text(&record.status)?),
        (STARTED_AT, datetime(&record.started_at)?),
    ];
    if let Some(value) = &record.finished_at {
        fields.push((FINISHED_AT, datetime(value)?));
    }
    for (name, value) in [
        (ERROR, &record.error),
        (EXECUTOR_IP, &record.executor_ip),
        (EXECUTOR_MAC, &record.executor_mac),
    ] {
        if let Some(value) = value {
            fields.push((name, text(value)?));
        }
    }
    write(RUN_ENTITY, fields, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::typedb::TypeDbStateStore;
    use std::sync::Arc;
    use type_bridge_orm::{
        AnswerCancellation, ConnectOptions, Database, QueryExecutionDeadline,
        QueryExecutionResourceLimits, TxType,
    };

    #[tokio::test]
    #[ignore = "requires an isolated TypeDB server"]
    async fn native_state_plans_preserve_live_timestamp_and_schema_transaction() {
        let address = std::env::var("TYPEDB_ADDRESS").unwrap();
        let options = ConnectOptions {
            http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
            ..ConnectOptions::default()
        };
        let database_name = format!("tb270_06c_state_{}", uuid::Uuid::new_v4().simple());
        type_bridge_orm::ensure_database_exists(
            &address,
            &database_name,
            "admin",
            "password",
            options,
        )
        .await
        .unwrap();
        let database = Arc::new(
            Database::connect_with_options(&address, &database_name, "admin", "password", options)
                .await
                .unwrap(),
        );
        let test_database = database.clone();
        let result = tokio::spawn(async move {
            let database = test_database;
            let transaction = database.transaction_context(TxType::Schema).await.unwrap();
            transaction
                .query(&migration_state_schema().to_typeql().unwrap())
                .await
                .unwrap();
            let plan = write(
                APPLIED_ENTITY,
                vec![
                    (MIGRATION_ID, text("app:initial").unwrap()),
                    (APP_LABEL, text("app").unwrap()),
                    (NAME, text("initial").unwrap()),
                    (
                        APPLIED_AT,
                        datetime("2026-08-14T10:30:00.123456000").unwrap(),
                    ),
                    (CHECKSUM, text("checksum").unwrap()),
                ],
                false,
            )
            .unwrap();
            let policy = QueryExecutionResourceLimits::default();
            transaction
                .execute_plans(
                    &[plan],
                    &policy,
                    &AnswerCancellation::default(),
                    QueryExecutionDeadline::for_limits(policy),
                )
                .await
                .unwrap();
            transaction.commit().await.unwrap();
            let mut read = database.read_transaction().await.unwrap();
            let query = format!(
                "match $r isa {APPLIED_ENTITY}, has {APPLIED_AT} $at; fetch {{\"applied\":$at}};"
            );
            let baseline = read.query(&query).await.unwrap();
            let type_bridge_orm::session::backend::QueryResult::Documents(baseline) = baseline
            else {
                panic!("raw baseline document")
            };
            let native = TypeDbStateStore::load_applied_in_transaction(&mut read)
                .await
                .unwrap();
            read.close().await.unwrap();
            eprintln!(
                "legacy timestamp baseline={} native={:?}",
                baseline[0]["applied"], native[0].applied_at
            );
            assert_eq!(
                native[0].applied_at.as_deref(),
                baseline[0]["applied"].as_str()
            );
            assert_eq!(native[0].checksum, "checksum");
            let transaction = database.transaction_context(TxType::Schema).await.unwrap();
            transaction
                .query("define entity state-rollback-marker;")
                .await
                .unwrap();
            let plan = write(
                APPLIED_ENTITY,
                vec![
                    (MIGRATION_ID, text("app:initial").unwrap()),
                    (APP_LABEL, text("app").unwrap()),
                ],
                false,
            )
            .unwrap();
            assert!(
                transaction
                    .execute_plans(
                        &[plan],
                        &policy,
                        &AnswerCancellation::default(),
                        QueryExecutionDeadline::for_limits(policy)
                    )
                    .await
                    .is_err()
            );
            assert!(transaction.commit().await.is_err());
            transaction.rollback().await.unwrap();
            transaction.close().await.unwrap();
            assert!(
                !database
                    .schema_text()
                    .await
                    .unwrap()
                    .contains("state-rollback-marker")
            );
        })
        .await;
        database.delete_database().await.unwrap();
        result.unwrap();
    }
}
