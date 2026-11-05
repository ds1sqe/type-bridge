//! Schema-bound native migration data programs.

use serde::{Deserialize, Deserializer, Serialize};
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::query_plan::v3::{QueryPlanV3, decode_query_plan_v3};
use type_bridge_contract::schema::{DeclaredSchema, decode_declared_schema};
use type_bridge_orm::session::TransactionContext;
use type_bridge_orm::{
    AnswerCancellation, OrmError, QueryExecutionDeadline, QueryExecutionResourceLimits,
};

use crate::plan::{ExecutionStep, MigrationAction, OperationKind};

/// Validated forward/reverse V3 programs and their explicit planning authority.
///
/// Persisted programs are revalidated against the carried declared schema when
/// decoded. Execution uses these plans directly in the migration transaction.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NativeDataStep {
    schema: DeclaredSchema,
    forward: QueryPlanV3,
    reverse: Option<QueryPlanV3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    counts: Option<[QueryPlanV3; 2]>,
}

impl NativeDataStep {
    /// Validate canonical V3 program objects against explicit planning authority.
    pub fn new(
        schema: &DeclaredSchema,
        forward: &serde_json::Value,
        reverse: Option<&serde_json::Value>,
    ) -> Result<Self, Diagnostic> {
        let decode = |value| decode_query_plan_v3(&to_canonical_json(value)?, schema);
        Ok(Self {
            schema: schema.clone(),
            forward: decode(forward)?,
            reverse: reverse.map(decode).transpose()?,
            counts: None,
        })
    }

    pub(crate) fn from_copy_plans(
        schema: &DeclaredSchema,
        forward: QueryPlanV3,
        reverse: Option<QueryPlanV3>,
        counts: [QueryPlanV3; 2],
    ) -> Self {
        Self {
            schema: schema.clone(),
            forward,
            reverse,
            counts: Some(counts),
        }
    }

    pub(crate) fn counts(&self) -> Option<&[QueryPlanV3; 2]> {
        self.counts.as_ref()
    }

    /// Select a validated program without changing the transaction or authority.
    pub fn program(&self, action: MigrationAction) -> Option<&QueryPlanV3> {
        match action {
            MigrationAction::Apply => Some(&self.forward),
            MigrationAction::Rollback => self.reverse.as_ref(),
        }
    }
}

impl<'de> Deserialize<'de> for NativeDataStep {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema: serde_json::Value,
            forward: serde_json::Value,
            reverse: Option<serde_json::Value>,
            counts: Option<[serde_json::Value; 2]>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let schema = to_canonical_json(&wire.schema)
            .and_then(|bytes| decode_declared_schema(&bytes))
            .map_err(serde::de::Error::custom)?;
        let mut data = Self::new(&schema, &wire.forward, wire.reverse.as_ref())
            .map_err(serde::de::Error::custom)?;
        if let Some(counts) = wire.counts {
            let decode_count = |value: &serde_json::Value| {
                let plan = decode_query_plan_v3(&to_canonical_json(value)?, &schema)?;
                crate::copy_plan::validate_count(&plan)?;
                Ok::<_, Diagnostic>(plan)
            };
            data.counts = Some([
                decode_count(&counts[0]).map_err(serde::de::Error::custom)?,
                decode_count(&counts[1]).map_err(serde::de::Error::custom)?,
            ]);
        }
        Ok(data)
    }
}

pub(crate) fn missing_authority() -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("migration_data_schema_authority_required").expect("static code"),
        "native migration data steps require explicit declared schema authority at planning",
    )
}

impl ExecutionStep {
    /// Whether the selected execution payload has a reverse program.
    pub fn has_reverse(&self) -> bool {
        self.data.as_ref().map_or_else(
            || self.reverse.is_some(),
            |data| data.program(MigrationAction::Rollback).is_some(),
        )
    }

    pub(crate) async fn execute_program(
        &self,
        transaction: &TransactionContext,
        action: MigrationAction,
    ) -> Result<(), OrmError> {
        if let Some(data) = &self.data {
            let program = data.program(action).ok_or_else(|| {
                OrmError::Transaction("native migration step has no reverse plan".into())
            })?;
            let limits = QueryExecutionResourceLimits::default();
            transaction
                .execute_plans(
                    std::slice::from_ref(program),
                    &limits,
                    &AnswerCancellation::default(),
                    QueryExecutionDeadline::for_limits(limits),
                )
                .await?;
            return Ok(());
        }
        if matches!(
            self.operation_kind,
            OperationKind::RunPlan | OperationKind::CopyAttribute
        ) {
            return Err(OrmError::Transaction(
                "native migration step is missing its validated program".into(),
            ));
        }
        let text = match action {
            MigrationAction::Apply => &self.forward,
            MigrationAction::Rollback => self.reverse.as_ref().ok_or_else(|| {
                OrmError::Transaction("migration step has no reverse TypeQL".into())
            })?,
        };
        transaction.query(text).await?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::spec::{MigrationGraph, MigrationSpec, OperationSpec};
    use crate::testing::{MockEvent, MockMigrationBackend};
    use type_bridge_orm::session::backend::QueryResult;
    use type_bridge_orm::{Database, TxType};

    pub(crate) fn fixture() -> (DeclaredSchema, MigrationGraph) {
        let schema = decode_declared_schema(include_bytes!(
            "../../../../tests/contracts/plan_v3/schema.json"
        ))
        .unwrap();
        let forward = serde_json::from_slice(include_bytes!(
            "../../../../tests/contracts/plan_v3/insert_first.json"
        ))
        .unwrap();
        let reverse = serde_json::from_slice(include_bytes!(
            "../../../../tests/contracts/plan_v3/update.json"
        ))
        .unwrap();
        (
            schema,
            MigrationGraph {
                migrations: vec![MigrationSpec {
                    app_label: "app".into(),
                    name: "0001_native".into(),
                    dependencies: vec![],
                    checksum: Some("native-checksum".into()),
                    source_sha256: None,
                    reversible: true,
                    operations: vec![OperationSpec::RunPlan {
                        forward,
                        reverse: Some(reverse),
                    }],
                }],
            },
        )
    }

    pub(crate) fn answer() -> QueryResult {
        use type_bridge_contract::id::{TypeId, TypeKind};
        use type_bridge_contract::query_remote::RemoteValue;
        QueryResult::Rows(vec![serde_json::json!({"account": RemoteValue::Thing {
            iid: "0x00000000000000000000000000000001".to_owned(),
            type_id: TypeId::new(TypeKind::Entity, "account").unwrap(),
        }})])
    }

    #[test]
    fn planning_requires_authority_and_persisted_native_steps_are_revalidated() {
        let (schema, graph) = fixture();
        let error = crate::plan(&graph, &[], None, None).unwrap_err();
        let crate::MigrationError::NativePlan(error) = error else {
            panic!("native error")
        };
        assert_eq!(
            error.code().as_str(),
            "migration_data_schema_authority_required"
        );
        let plan = crate::plan(&graph, &[], None, Some(&schema)).unwrap();
        assert!(plan.to_apply[0].reversible);
        assert!(plan.to_apply[0].steps[0].forward.is_empty());
        let mut encoded = serde_json::to_value(&plan).unwrap();
        let decoded: crate::ExecutionPlan = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(decoded, plan);
        encoded["to_apply"][0]["steps"][0]["data"]["forward"]["pipeline"][0]["things"][0]["type_id"]
            ["label"] = serde_json::json!("missing");
        assert!(serde_json::from_value::<crate::ExecutionPlan>(encoded).is_err());
    }

    #[tokio::test]
    async fn native_forward_and_reverse_use_one_write_statement_and_caller_commit() {
        let (schema, graph) = fixture();
        let plan = crate::plan(&graph, &[], None, Some(&schema)).unwrap();
        for (action, marker) in [
            (MigrationAction::Apply, "insert"),
            (MigrationAction::Rollback, "update"),
        ] {
            let (backend, log) = MockMigrationBackend::with_responses(vec![answer()]);
            let db = Database::with_backend(Box::new(backend), "native-migration");
            let mut migration = plan.to_apply[0].clone();
            migration.action = action;
            let result = crate::execute_migration(&db, &migration).await;
            assert!(result.success, "{result:?}");
            let events = log.lock().unwrap();
            let queries = events
                .iter()
                .filter_map(|event| match event {
                    MockEvent::Query(kind, text) => Some((kind, text)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(queries.len(), 1);
            assert_eq!(*queries[0].0, TxType::Write);
            assert!(queries[0].1.contains(marker));
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(event, MockEvent::Commit))
                    .count(),
                1
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, MockEvent::Rollback))
            );
        }
    }
    #[tokio::test]
    async fn historical_copy_text_and_filters_apply_and_rollback_without_counts() {
        let (_, base) = crate::copy_plan::tests::fixture();
        for field in ["forward", "reverse", "filter"] {
            let mut wire = serde_json::to_value(&base).unwrap();
            if field == "forward" {
                wire["forward"] =
                    serde_json::json!("  match $x isa person; insert $x has new-name 'custom';\n");
                wire["reverse"] = serde_json::json!(
                    " match $x isa person, has new-name $v; delete has $v of $x; "
                );
                wire["owner"] = serde_json::Value::Null;
                wire["mode"] = serde_json::Value::Null;
            } else if field == "reverse" {
                wire["reverse"] = serde_json::json!(
                    " match $x isa person, has new-name $v; delete has $v of $x; "
                );
            } else {
                wire["filter"] = serde_json::json!("$v != \"skip\";");
            }
            let operation: OperationSpec = serde_json::from_value(wire).unwrap();
            let (forward, reverse) = crate::spec::copy_attribute_typeql(&operation).unwrap();
            let (_, mut graph) = fixture();
            graph.migrations[0].operations = vec![operation];
            let mut initial = graph.migrations[0].clone();
            initial.name = "0000_initial".into();
            initial.checksum = Some("initial-checksum".into());
            initial.operations.clear();
            graph.migrations[0].dependencies = vec![crate::spec::MigrationDependencySpec {
                app_label: "app".into(),
                migration_name: "0000_initial".into(),
            }];
            graph.migrations.insert(0, initial);
            let planned = crate::plan(&graph, &[], None, None).unwrap();
            let applied = graph
                .migrations
                .iter()
                .map(|migration| crate::graph::AppliedMigrationRecord {
                    app_label: migration.app_label.clone(),
                    name: migration.name.clone(),
                    checksum: migration.checksum.clone().unwrap(),
                    applied_at: None,
                })
                .collect::<Vec<_>>();
            let rollback = crate::plan(&graph, &applied, Some("0000_initial"), None).unwrap();
            assert_eq!(rollback.to_rollback.len(), 1);
            let step = &planned.to_apply[1].steps[0];
            assert_eq!(step.operation_kind, OperationKind::RunTypeql);
            assert!(step.data.is_none());
            assert_eq!(step.forward, forward);
            assert_eq!(step.reverse, reverse);
            for (action, expected) in [
                (MigrationAction::Apply, forward.as_str()),
                (MigrationAction::Rollback, reverse.as_deref().unwrap()),
            ] {
                let (backend, log) =
                    MockMigrationBackend::with_responses(vec![QueryResult::Rows(vec![])]);
                let db = Database::with_backend(Box::new(backend), "historical-copy");
                let migration = match action {
                    MigrationAction::Apply => &planned.to_apply[1],
                    MigrationAction::Rollback => &rollback.to_rollback[0],
                };
                let result = crate::execute_migration(&db, migration).await;
                assert!(result.success, "{field}: {result:?}");
                assert!(result.backfill.is_none());
                let events = log.lock().unwrap();
                let queries = events
                    .iter()
                    .filter_map(|event| match event {
                        MockEvent::Query(kind, text) => Some((kind, text.as_str())),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(queries, vec![(&TxType::Write, expected)]);
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event, MockEvent::Commit))
                        .count(),
                    1
                );
            }
        }
    }
}
