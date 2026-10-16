//! Helpers build `query-plan/v3` programs; they never format TypeQL.
//!
//! A builder declares bindings and caller values on a [`PlanDraft`], then
//! validates the finished program against the installed schema authority.
//! Caller values become `given` input columns when the transaction can carry
//! a `given` stage, and plan literals otherwise (band-8 connections), so the
//! adaptor renders them in both cases.

pub(crate) mod authority;
pub(crate) mod insert;
pub(crate) mod put;
pub(crate) mod reference;

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
        self,
        authority: &PlanAuthority,
        pipeline: Vec<Stage>,
        operation: Operation,
        output: QueryOutput,
    ) -> Result<BuiltPlan, Diagnostic> {
        let given_rows = (!self.inputs.is_empty()).then(|| vec![InputRow::new(self.row)]);
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
        let (query, given) = lower(plan.local()).unwrap();
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
            type_id: &doctor,
            keys: &keys,
            fields: if fields { &on_call } else { &[] },
            cleared: if fields { &[] } else { &cleared },
        };
        let plan = keyed_entity_put(&authority(), ValueTransport::Given, &put).unwrap();
        let (query, given) = lower(plan.local()).unwrap();
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
