//! Helpers build `query-plan/v3` programs; they never format TypeQL.
//!
//! A builder declares bindings and caller values on a [`PlanDraft`], then
//! validates the finished program against the installed schema authority.
//! Caller values become `given` input columns when the transaction can carry
//! a `given` stage, and plan literals otherwise (custom backends without given transport), so the
//! adaptor renders them in both cases.

pub(crate) mod authority;
pub(crate) mod batch;
pub(crate) mod batch_native;
pub(crate) mod bulk_write;
pub(crate) mod input;
pub(crate) mod insert;
pub(crate) mod manager;
pub(crate) mod put;
pub(crate) mod reference;
pub(crate) mod write;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::v3::{Operation, PlanSpec, QueryPlanV3, Stage};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput,
};
use type_bridge_contract::value::CanonicalValue;

use authority::PlanAuthority;

/// How caller values reach TypeDB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValueTransport {
    /// One `given` row; the plan references input columns.
    Given,
    /// Plan literals, for connections without the `given` stage.
    Literal,
}

/// Bindings, input columns and one input row of a plan under construction.
pub(crate) struct PlanDraft {
    transport: ValueTransport,
    bindings: Vec<AssertionBinding>,
    inputs: Vec<InputColumn>,
    row: Vec<Option<CanonicalValue>>,
}

impl PlanDraft {
    pub(crate) const fn new(transport: ValueTransport) -> Self {
        Self {
            transport,
            bindings: Vec::new(),
            inputs: Vec::new(),
            row: Vec::new(),
        }
    }

    /// Declare the next binding. Names are `<stem><ordinal>`, so they never
    /// collide with each other or with the `in<ordinal>` input columns.
    pub(crate) fn bind(&mut self, stem: &str) -> Result<BindingId, Diagnostic> {
        let ordinal = u16::try_from(self.bindings.len()).unwrap_or(u16::MAX);
        let id = BindingId::new(ordinal)?;
        self.bindings.push(AssertionBinding::new(
            id,
            QueryVariable::new(format!("{stem}{ordinal}"))?,
        ));
        Ok(id)
    }

    /// Declare one column of a homogeneous native batch.
    pub(crate) fn input(
        &mut self,
        name: &str,
        domain: type_bridge_contract::value::ValueTypeTag,
        nullable: bool,
    ) -> Result<QueryOperand, Diagnostic> {
        let column = InputColumnId::new(u16::try_from(self.inputs.len()).unwrap_or(u16::MAX));
        self.inputs.push(InputColumn::new(
            column,
            QueryVariable::new(name)?,
            domain,
            nullable,
        ));
        Ok(QueryOperand::Input { column })
    }

    /// Carry one caller value as an input column or a plan literal.
    ///
    /// A literal string with a control character is refused: the adaptor
    /// would have to escape it into query text, and TypeQL string escapes
    /// are not a contract the plan can rely on. `given` rows carry any text.
    pub(crate) fn value(&mut self, value: CanonicalValue) -> Result<QueryOperand, Diagnostic> {
        if self.transport == ValueTransport::Literal {
            if let CanonicalValue::String(text) = &value
                && text.as_str().chars().any(char::is_control)
            {
                return Err(Diagnostic::new(
                    DiagnosticCategory::InvalidContract,
                    DiagnosticCode::new(LITERAL_CONTROL_CHARACTER).expect("static diagnostic code"),
                    "a plan literal cannot carry a control character",
                ));
            }
            return Ok(QueryOperand::Literal { value });
        }
        let ordinal = u16::try_from(self.inputs.len()).unwrap_or(u16::MAX);
        let column = InputColumnId::new(ordinal);
        self.inputs.push(InputColumn::new(
            column,
            QueryVariable::new(format!("in{ordinal}"))?,
            value.value_type(),
            false,
        ));
        self.row.push(Some(value));
        Ok(QueryOperand::Input { column })
    }

    /// Validate the program against the installed authority.
    pub(crate) fn finish(
        mut self,
        authority: &PlanAuthority,
        pipeline: Vec<Stage>,
        operation: Operation,
        output: QueryOutput,
    ) -> Result<BuiltPlan, Diagnostic> {
        let given_rows =
            (!self.inputs.is_empty()).then(|| vec![InputRow::new(std::mem::take(&mut self.row))]);
        self.finish_rows(authority, pipeline, operation, output, given_rows)
    }

    pub(crate) fn finish_rows(
        self,
        authority: &PlanAuthority,
        pipeline: Vec<Stage>,
        operation: Operation,
        output: QueryOutput,
        given_rows: Option<Vec<InputRow>>,
    ) -> Result<BuiltPlan, Diagnostic> {
        let plan = QueryPlanV3::new(
            PlanSpec {
                bindings: self.bindings,
                inputs: self.inputs,
                given_rows,
                operation,
                pipeline,
                output,
                managed_semantics: authority.managed_semantics().clone(),
                compatibility: None,
                functions: Vec::new(),
            },
            authority.declared(),
        )?;
        Ok(BuiltPlan {
            plan,
            verified: authority.verified(),
        })
    }
}

/// Code of the refusal of a literal string with a control character.
pub(crate) const LITERAL_CONTROL_CHARACTER: &str = "query_plan_literal_control_character";

/// A helper plan and whether its authority was verified.
#[derive(Debug)]
pub(crate) struct BuiltPlan {
    plan: QueryPlanV3,
    verified: bool,
}

impl BuiltPlan {
    /// The plan for local execution.
    pub(crate) const fn local(&self) -> &QueryPlanV3 {
        &self.plan
    }

    /// The plan for remote staging, which requires a verified authority: a
    /// proxy validates against its package authority, and declarations
    /// rebuilt from a projection are not that authority.
    // The proxy stages helper plans once helpers stage (phase H).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn remote(&self) -> Result<&QueryPlanV3, Diagnostic> {
        if self.verified {
            Ok(&self.plan)
        } else {
            Err(Diagnostic::new(
                DiagnosticCategory::InvalidContract,
                DiagnosticCode::new("write_plan_authority_unverified")
                    .expect("static diagnostic code"),
                "a plan built from projection-rebuilt declarations cannot be staged remotely",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
    use type_bridge_contract::schema::OwnsFactId;
    use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};
    use type_bridge_core_lib::__internal as tql;

    use super::ValueTransport;
    use super::authority::PlanAuthority;
    use super::insert::relation_insert;
    use super::put::{KeyedPut, keyed_entity_put};
    use super::reference::{Reference, ReferenceIdentity};
    use crate::query_plan_exec::{lower, race};

    fn authority() -> PlanAuthority {
        PlanAuthority::new(
            race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"plan-builders",
            )
            .unwrap(),
        )
    }

    #[test]
    fn put_explain_records_matrix_or_sequential_input_strategy_and_canonical_cuts() {
        use super::bulk_write::{WriteKind, WriteRow, WriteShape, write_many};
        let authority = authority();
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let key =
            OwnsFactId::new(account.clone(), AttributeId::new("account-id").unwrap()).unwrap();
        let balance =
            OwnsFactId::new(account.clone(), AttributeId::new("balance").unwrap()).unwrap();
        let first = vec![
            (key.clone(), text("private-first")),
            (balance.clone(), CanonicalValue::Long(1)),
        ];
        let second = vec![
            (key.clone(), text("private-second")),
            (balance.clone(), CanonicalValue::Long(2)),
        ];
        for duplicate in [false, true] {
            let rows = [
                WriteRow {
                    iid: None,
                    fields: &first,
                    players: &[],
                },
                WriteRow {
                    iid: None,
                    fields: if duplicate { &first } else { &second },
                    players: &[],
                },
            ];
            let mut program = write_many(
                &authority,
                WriteKind::PartialPut,
                WriteShape {
                    type_id: &account,
                    keys: std::slice::from_ref(&key),
                    cleared_fields: std::slice::from_ref(&balance),
                    cleared_roles: &[],
                    document_roles: vec![],
                    include_subtypes: true,
                    return_documents: false,
                },
                &rows,
            )
            .unwrap();
            assert_eq!(program.statement_count(), 1);
            let step = program.next().unwrap().unwrap();
            let explanation = crate::plan_explain::explain(step.plan.local()).unwrap();
            assert_eq!(explanation.statements.len(), 1);
            assert_eq!(explanation.write_statements, 1);
            assert_eq!(explanation.given.row_count, if duplicate { 1 } else { 2 });
            assert_eq!(
                step.plan
                    .local()
                    .pipeline()
                    .iter()
                    .filter(|stage| matches!(stage, super::Stage::Put { .. }))
                    .count(),
                if duplicate { 2 } else { 1 }
            );
            assert!(!explanation.statements[0].contains("private-first"));
        }
        let fields = (0..130)
            .map(|index| {
                vec![
                    (key.clone(), text("same-private-key")),
                    (balance.clone(), CanonicalValue::Long(index)),
                ]
            })
            .collect::<Vec<_>>();
        let rows = fields
            .iter()
            .map(|fields| WriteRow {
                iid: None,
                fields,
                players: &[],
            })
            .collect::<Vec<_>>();
        let program = write_many(
            &authority,
            WriteKind::PartialPut,
            WriteShape {
                type_id: &account,
                keys: std::slice::from_ref(&key),
                cleared_fields: &[],
                cleared_roles: &[],
                document_roles: vec![],
                include_subtypes: true,
                return_documents: false,
            },
            &rows,
        )
        .unwrap();
        assert_eq!(program.statement_count(), 2);
        let ordinals = program
            .identity_layout()
            .unwrap()
            .into_iter()
            .flatten()
            .map(|(ordinal, _)| ordinal)
            .collect::<Vec<_>>();
        assert_eq!(ordinals, (0..130).collect::<Vec<_>>());
    }

    #[test]
    fn bulk_writes_keep_one_input_row_per_thing_and_refuse_empty_relation_players() {
        use super::bulk_write::{WriteKind, WriteRow, WriteShape, write_many};
        let authority = authority();
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let key =
            OwnsFactId::new(account.clone(), AttributeId::new("account-id").unwrap()).unwrap();
        let first = [(
            key.clone(),
            CanonicalValue::String(CanonicalString::new("first").unwrap()),
        )];
        let second = [(
            key.clone(),
            CanonicalValue::String(CanonicalString::new("second").unwrap()),
        )];
        let rows = [
            WriteRow {
                iid: None,
                fields: &first,
                players: &[],
            },
            WriteRow {
                iid: None,
                fields: &second,
                players: &[],
            },
        ];
        let plan = write_many(
            &authority,
            WriteKind::Insert,
            WriteShape {
                include_subtypes: false,
                return_documents: true,
                type_id: &account,
                keys: &[key],
                cleared_fields: &[],
                cleared_roles: &[],
                document_roles: vec![],
            },
            &rows,
        )
        .unwrap();
        assert_eq!(plan.statement_count(), 1);
        let super::bulk_write::WriteProgram::Compact(plan) = plan else {
            panic!("compact fixture")
        };
        let plan = &plan[0];
        assert_eq!(plan.local().given_rows().unwrap().len(), 2);
        let literal = crate::query_plan_exec::staged_literal_query(plan.local()).unwrap();
        assert!(literal.rows.is_none());
        assert!(!literal.typeql.contains("given "));
        assert!(literal.typeql.contains("first") && literal.typeql.contains("second"));
        let redacted = literal.redacted_typeql.as_deref().unwrap();
        assert!(!redacted.contains("first") && !redacted.contains("second"));
        tql::parse(&literal.typeql).unwrap();
        assert_eq!(
            plan.local()
                .pipeline()
                .iter()
                .filter(|stage| matches!(
                    stage,
                    type_bridge_contract::query_plan::v3::Stage::Insert { .. }
                ))
                .count(),
            1
        );
        let relation = TypeId::new(TypeKind::Relation, "purchase").unwrap();
        let error = write_many(
            &authority,
            WriteKind::Insert,
            WriteShape {
                include_subtypes: false,
                return_documents: true,
                type_id: &relation,
                keys: &[],
                cleared_fields: &[],
                cleared_roles: &[],
                document_roles: vec![],
            },
            &[WriteRow {
                iid: None,
                fields: &[],
                players: &[],
            }],
        )
        .err()
        .expect("playerless relation refused");
        assert_eq!(error.code().as_str(), "query_plan_insert_relation_players");
    }

    #[test]
    fn manager_player_guards_are_pinned_only_for_multi_player_roles() {
        use sha2::{Digest, Sha256};
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::query_plan::QueryPattern;
        use type_bridge_contract::query_plan::v3::{Expr, Pattern};
        let declared = race::schema();
        let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
        let resolved = type_bridge_schema::resolve(&declared, &profile).unwrap();
        let installed = crate::InstalledRuntimeProjection::try_new(
            type_bridge_schema::project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v1()],
                &[],
            )
            .unwrap(),
        )
        .unwrap();
        let context = type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("manager-guard-test").unwrap(),
            profile,
            type_bridge_schema::schema_authority_capability_vocabulary(),
        );
        let authority = type_bridge_schema::build_schema_authority(
            &declared,
            declared.required_capabilities(),
            &context,
        )
        .unwrap();
        let installed = installed.with_schema_authority(&authority);
        let plans = super::manager::ManagerPlans::new(std::sync::Arc::new(
            installed.match_registry().unwrap(),
        ));
        let player =
            |role: &str, type_name: &str, iid: &str| crate::_dynamic::DynamicRolePlayerInput {
                role_name: role.into(),
                player_type_name: type_name.into(),
                iid: Some(iid.into()),
                key: None,
            };
        let cases = [
            vec![player("item", "item", "0x10")],
            vec![
                player("item", "item", "0x10"),
                player("buyer", "account", "0x11"),
            ],
            vec![
                player("item", "item", "0x10"),
                player("item", "item", "0x12"),
            ],
        ];
        let actual = cases.into_iter().map(|players| {
            let plan = plans.update("purchase", Some("0x20"), &vec![], &players, true).unwrap().unwrap();
            let patterns = match &plan.local().pipeline()[0] { type_bridge_contract::query_plan::v3::Stage::Match { patterns } => patterns, _ => panic!("initial match") };
            let iid_lets = patterns.iter().filter(|pattern| matches!(pattern, Pattern::Let { expr: Expr::Iid { .. }, .. })).count();
            let guards = patterns.iter().filter(|pattern| matches!(pattern, Pattern::Read(QueryPattern::Value { comparator: type_bridge_contract::migration_assertion::ValueComparator::NotEqual, .. }))).count();
            (format!("{:x}", Sha256::digest(plan.local().canonical_bytes().unwrap())), iid_lets, guards)
        }).collect::<Vec<_>>();
        assert_eq!(
            actual,
            [
                (
                    "9496a23bb3fcae34a05cbe9b8808bd165ec6e2a61d2355fd05a701b729a69ac0".into(),
                    0,
                    0
                ),
                (
                    "779f4b2d08e6f29e924870ac33e815e0d2c5514b8823c7c35208aec35ef58ebf".into(),
                    0,
                    0
                ),
                (
                    "ee6b240ab9274cd507f7294961590bece1e0e5a302b5a4ac7a65b59826cf5cb1".into(),
                    2,
                    1
                )
            ]
        );
        let scalar_update = plans
            .update(
                "doctor",
                Some("0x20"),
                &vec![("on-call".into(), crate::AttributeValue::Boolean(true))],
                &[],
                false,
            )
            .unwrap()
            .unwrap();
        let select = scalar_update
            .local()
            .pipeline()
            .iter()
            .rev()
            .find_map(|stage| match stage {
                type_bridge_contract::query_plan::v3::Stage::Select { bindings } => Some(bindings),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            select.as_slice(),
            &[type_bridge_contract::migration_assertion::BindingId::new(0).unwrap()],
            "replacement values leave scope after attachment"
        );
    }

    #[test]
    fn fetch_terminal_matches_orm_insert_after_variable_normalization() {
        use type_bridge_contract::codec::to_canonical_json;
        use type_bridge_contract::query_plan::QueryOperand;
        use type_bridge_contract::query_plan::v3::PlanBuilder;
        let authority = authority();
        let doctor = TypeId::new(TypeKind::Entity, "doctor").unwrap();
        let field = OwnsFactId::new(doctor.clone(), AttributeId::new("name").unwrap()).unwrap();
        let value = CanonicalValue::String(CanonicalString::new("Ada").unwrap());
        let helper = super::write::entity_insert(
            &authority,
            ValueTransport::Literal,
            &doctor,
            &[(field.clone(), value.clone())],
        )
        .unwrap();
        let mut builder = PlanBuilder::new();
        let thing = builder.insert(doctor);
        builder
            .thing_field(thing, field, QueryOperand::Literal { value })
            .fetch(thing, vec![]);
        let built = builder
            .build(authority.declared(), authority.managed_semantics().clone())
            .unwrap();
        let mut helper_json: serde_json::Value =
            serde_json::from_slice(&helper.local().canonical_bytes().unwrap()).unwrap();
        // ORM helpers call their root thing0; public builders derive its type label.
        helper_json["bindings"][0]["variable"] = built.bindings()[0].variable().as_str().into();
        assert_eq!(
            built.canonical_bytes().unwrap(),
            to_canonical_json(&helper_json).unwrap()
        );
    }

    #[test]
    fn delete_returning_is_opt_in_and_preserves_the_plain_delete_statement() {
        use type_bridge_contract::query_plan::v3::{Operation, WriteReturns};
        let authority = authority();
        let doctor = TypeId::new(TypeKind::Entity, "doctor").unwrap();
        let iid = "0x1E00000000000000000001";
        let plain = super::write::delete_by_iid(&authority, &doctor, iid).unwrap();
        let returning =
            super::write::delete_by_iid_returning(&authority, &doctor, iid, vec![]).unwrap();
        assert_eq!(
            plain.local().operation(),
            Operation::Write {
                returns: WriteReturns::Matched
            }
        );
        assert_eq!(
            returning.local().operation(),
            Operation::Write {
                returns: WriteReturns::Rows
            }
        );
        let plain = lower(plain.local()).unwrap();
        let returning = lower(returning.local()).unwrap();
        assert!(plain.snapshot.is_none());
        assert!(plain.given.is_none());
        assert!(
            tql::equivalent(
                &tql::render(&plain.query).unwrap(),
                &format!("match $thing0 isa! doctor; $thing0 iid {iid}; delete $thing0;")
            )
            .unwrap()
        );
        let snapshot = tql::render(returning.snapshot.as_ref().unwrap()).unwrap();
        let deletion = tql::render(&returning.query).unwrap();
        for statement in [&snapshot, &deletion] {
            assert!(!statement.contains(iid));
            assert!(statement.contains("given $delete_snapshot_iid0: string;"));
            assert!(statement.contains("iid($thing0)"));
            assert!(statement.contains("$delete_snapshot_iid1 == $delete_snapshot_iid0"));
        }
        let given = returning.given.unwrap();
        assert_eq!(given.variables, ["delete_snapshot_iid0"]);
        assert_eq!(
            given.rows,
            [vec![crate::session::backend::GivenValue::String(
                iid.to_ascii_lowercase()
            )]]
        );
    }

    #[test]
    fn snapshot_iid_transport_preserves_existing_rows_and_avoids_name_collisions() {
        use crate::session::backend::GivenValue;
        use type_bridge_contract::migration_assertion::QueryVariable;
        use type_bridge_contract::query_plan::v3::{
            DeleteItem, Operation, Pattern, Stage, ThingDocument, WriteReturns,
        };
        use type_bridge_contract::query_plan::{
            InputColumn, InputColumnId, InputRow, QueryOutput, QueryPattern,
        };
        use type_bridge_contract::value::ValueTypeTag;

        let authority = authority();
        let mut draft = super::PlanDraft::new(ValueTransport::Given);
        let thing = draft.bind("delete_snapshot_iid").unwrap();
        draft.value(text("first")).unwrap();
        draft.inputs[0] = InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("delete_snapshot_iid1").unwrap(),
            ValueTypeTag::String,
            false,
        );
        let iid = "0x1e00000000000000000001";
        let built = draft
            .finish(
                &authority,
                vec![
                    Stage::Match {
                        patterns: vec![
                            QueryPattern::Isa {
                                binding: thing,
                                include_subtypes: false,
                                type_id: TypeId::new(TypeKind::Entity, "doctor").unwrap(),
                            }
                            .into(),
                            Pattern::Iid {
                                binding: thing,
                                iid: iid.to_owned(),
                            },
                        ],
                    },
                    Stage::Fetch(ThingDocument::new(thing, vec![])),
                    Stage::Delete {
                        items: vec![DeleteItem::Thing { binding: thing }],
                    },
                ],
                Operation::Write {
                    returns: WriteReturns::Rows,
                },
                QueryOutput::Rows { columns: vec![] },
            )
            .unwrap();
        for values in [vec!["first"], vec!["first", "second"]] {
            let plan = built
                .local()
                .with_given_rows(
                    values
                        .iter()
                        .map(|value| InputRow::new(vec![Some(text(value))]))
                        .collect(),
                    authority.declared(),
                )
                .unwrap();
            let lowered = lower(&plan).unwrap();
            let given = lowered.given.unwrap();
            assert_eq!(
                given.variables,
                ["delete_snapshot_iid1", "delete_snapshot_iid2"]
            );
            assert_eq!(
                given.rows,
                values
                    .iter()
                    .map(|value| vec![
                        GivenValue::String((*value).to_owned()),
                        GivenValue::String(iid.to_owned()),
                    ])
                    .collect::<Vec<_>>()
            );
            for query in [lowered.query, lowered.snapshot.unwrap()] {
                let statement = tql::render(&query).unwrap();
                assert!(!statement.contains(iid));
                assert!(statement.contains("$delete_snapshot_iid3 == $delete_snapshot_iid2"));
            }
        }
    }

    fn text(value: &str) -> CanonicalValue {
        CanonicalValue::String(CanonicalString::new(value).unwrap())
    }

    fn purchase(transport: ValueTransport) -> (String, Option<Vec<String>>) {
        purchase_order(transport, "O1").unwrap()
    }

    fn purchase_order(
        transport: ValueTransport,
        order: &str,
    ) -> Result<(String, Option<Vec<String>>), type_bridge_contract::diagnostic::Diagnostic> {
        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let item = TypeId::new(TypeKind::Entity, "item").unwrap();
        let purchase = TypeId::new(TypeKind::Relation, "purchase").unwrap();
        let key = AttributeId::new("account-id").unwrap();
        let accounts = [account];
        let items = [item];
        let plan = relation_insert(
            &authority(),
            transport,
            &purchase,
            &[(
                OwnsFactId::new(purchase.clone(), AttributeId::new("order-id").unwrap()).unwrap(),
                text(order),
            )],
            &[
                (
                    RoleId::new("purchase", "buyer").unwrap(),
                    Reference {
                        candidates: &accounts,
                        identity: ReferenceIdentity::Key {
                            attribute: &key,
                            value: text("A1"),
                        },
                    },
                ),
                (
                    RoleId::new("purchase", "item").unwrap(),
                    Reference {
                        candidates: &items,
                        identity: ReferenceIdentity::Iid("0x1e00000000000000000001"),
                    },
                ),
            ],
            Vec::new(),
        )?;
        let lowered = lower(plan.local()).unwrap();
        let (query, given) = (lowered.query, lowered.given);
        Ok((
            tql::render(&query).unwrap(),
            given.map(|given| given.variables),
        ))
    }

    #[test]
    fn relation_insert_resolves_references_in_its_own_match() {
        let (typeql, variables) = purchase(ValueTransport::Given);
        let expected = "given $in0: string, $in1: string;
match
  $player1 isa! account;
  $player1 has account-id $key2; $key2 isa! account-id;
  $key2 == $in0;
  $player3 isa! item;
  $player3 iid 0x1e00000000000000000001;
insert
  $relation0 isa purchase, links (buyer: $player1, item: $player3), has order-id == $in1;
match $relation0 isa! $PlanFetchType;
fetch { \"_iid\": iid($relation0), \"_type\": label($PlanFetchType), \"attributes\": { $relation0.* } };";
        assert!(
            tql::equivalent(&typeql, expected).unwrap(),
            "lowered:\n{typeql}\nexpected:\n{expected}"
        );
        assert_eq!(variables.unwrap(), ["in0", "in1"]);
        // Caller values travel only as given rows.
        assert!(!typeql.contains("\"A1\"") && !typeql.contains("\"O1\""));
    }

    #[test]
    fn connections_without_given_carry_values_as_adaptor_literals() {
        let (typeql, variables) = purchase(ValueTransport::Literal);
        let expected = "match
  $player1 isa! account;
  $player1 has account-id $key2; $key2 isa! account-id;
  $key2 == \"A1\";
  $player3 isa! item;
  $player3 iid 0x1e00000000000000000001;
insert
  $relation0 isa purchase, links (buyer: $player1, item: $player3), has order-id \"O1\";
match $relation0 isa! $PlanFetchType;
fetch { \"_iid\": iid($relation0), \"_type\": label($PlanFetchType), \"attributes\": { $relation0.* } };";
        assert!(
            tql::equivalent(&typeql, expected).unwrap(),
            "lowered:\n{typeql}\nexpected:\n{expected}"
        );
        assert!(variables.is_none());
    }

    fn doctor_put(fields: bool) -> String {
        let doctor = TypeId::new(TypeKind::Entity, "doctor").unwrap();
        let owns =
            |name: &str| OwnsFactId::new(doctor.clone(), AttributeId::new(name).unwrap()).unwrap();
        let keys = [(owns("name"), text("alice"))];
        let on_call = [(owns("on-call"), CanonicalValue::Boolean(true))];
        let cleared = [owns("on-call")];
        let put = KeyedPut {
            include_subtypes: false,
            return_document: true,
            type_id: &doctor,
            keys: &keys,
            fields: if fields { &on_call } else { &[] },
            cleared: if fields { &[] } else { &cleared },
            attached: &[],
        };
        let plan = keyed_entity_put(&authority(), ValueTransport::Given, &put).unwrap();
        let lowered = lower(plan.local()).unwrap();
        let (query, given) = (lowered.query, lowered.given);
        assert!(given.is_some());
        tql::render(&query).unwrap()
    }

    #[test]
    fn keyed_put_puts_the_key_then_replaces_the_other_fields() {
        let updated = doctor_put(true);
        let expected = "given $in0: string, $in1: boolean;
put $thing0 isa! doctor, has name == $in0;
update $thing0 has on-call == $in1;
select $thing0; distinct;
match $thing0 isa! $PlanFetchType;
fetch { \"_iid\": iid($thing0), \"_type\": label($PlanFetchType), \"attributes\": { $thing0.* } };";
        assert!(
            tql::equivalent(&updated, expected).unwrap(),
            "lowered:\n{updated}\nexpected:\n{expected}"
        );
        let cleared = doctor_put(false);
        let expected = "given $in0: string;
put $thing0 isa! doctor, has name == $in0;
match try { $thing0 has on-call $PlanHas1x0; $PlanHas1x0 isa! on-call; };
delete try { has $PlanHas1x0 of $thing0; };
select $thing0; distinct;
match $thing0 isa! $PlanFetchType;
fetch { \"_iid\": iid($thing0), \"_type\": label($PlanFetchType), \"attributes\": { $thing0.* } };";
        assert!(
            tql::equivalent(&cleared, expected).unwrap(),
            "lowered:\n{cleared}\nexpected:\n{expected}"
        );
    }

    #[test]
    fn literal_strings_escape_quotes_and_refuse_control_characters() {
        let (typeql, _) = purchase_order(ValueTransport::Literal, "say \"hi\" \\ back").unwrap();
        let expected = "match
  $player1 isa! account;
  $player1 has account-id $key2; $key2 isa! account-id;
  $key2 == \"A1\";
  $player3 isa! item;
  $player3 iid 0x1e00000000000000000001;
insert
  $relation0 isa purchase, links (buyer: $player1, item: $player3),
    has order-id \"say \\\"hi\\\" \\\\ back\";
match $relation0 isa! $PlanFetchType;
fetch { \"_iid\": iid($relation0), \"_type\": label($PlanFetchType), \"attributes\": { $relation0.* } };";
        assert!(
            tql::equivalent(&typeql, expected).unwrap(),
            "lowered:\n{typeql}\nexpected:\n{expected}"
        );
        for control in ["line\nbreak", "tab\there", "nul\u{0}", "bell\u{7}"] {
            let error = purchase_order(ValueTransport::Literal, control).unwrap_err();
            assert_eq!(error.code().as_str(), super::LITERAL_CONTROL_CHARACTER);
        }
        // `given` rows carry any text.
        assert!(purchase_order(ValueTransport::Given, "line\nbreak").is_ok());
    }

    #[test]
    fn plans_from_unverified_authority_cannot_be_staged_remotely() {
        let insert = |authority: &PlanAuthority| {
            let purchase = TypeId::new(TypeKind::Relation, "purchase").unwrap();
            let items = [TypeId::new(TypeKind::Entity, "item").unwrap()];
            relation_insert(
                authority,
                ValueTransport::Given,
                &purchase,
                &[],
                &[(
                    RoleId::new("purchase", "item").unwrap(),
                    Reference {
                        candidates: &items,
                        identity: ReferenceIdentity::Iid("0x1e00000000000000000001"),
                    },
                )],
                Vec::new(),
            )
            .unwrap()
        };
        let verified = insert(&authority());
        assert!(verified.remote().is_ok());
        let unverified = insert(&authority().unverified());
        assert_eq!(
            unverified.remote().unwrap_err().code().as_str(),
            "write_plan_authority_unverified"
        );
        assert_eq!(unverified.local(), verified.local());
    }
}
