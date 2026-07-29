//! Shared Rust V2 authoring state-machine coverage.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::id::{AttributeId, FunctionId, Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::{MAX_BINDINGS, MAX_BOOLEAN_TERMS, MAX_PREDICATE_NODES};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, OrderDirection, QueryOperand, QueryOutput, QueryPattern, QueryPlan,
    ReadStage,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement,
    FunctionReturnMode, FunctionSignature, OwnsFact, OwnsFactId, PlaysFact, PlaysFactId,
    RelatesFact, RelatesFactId, SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact,
    TypeFact, TypeReference, ValueFact, ValueFactId, encode_declared_schema,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration, CanonicalTime,
    TimeZoneDesignator,
};
use type_bridge_contract::value::{
    CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag,
};
use type_bridge_orm::query_v2_builder::{
    AuthoredQueryPlan, QUERY_PLAN_BUILDER_OPERATIONS, QueryFunctionTarget, QueryPlanBuilder,
};
use type_bridge_orm::query_v2_prepared::QueryAuthority;

const PROFILE: &str = "typedb-3.12.1/v1";
const SCOPE: &str = "query-v2-builder";

#[test]
fn frozen_operation_inventory_is_exhaustive_and_ordered() {
    assert_eq!(
        QUERY_PLAN_BUILDER_OPERATIONS,
        &[
            "binding",
            "input",
            "binding_operand",
            "literal_operand",
            "input_operand",
            "isa",
            "has",
            "links",
            "value",
            "not",
            "or",
            "try",
            "reachable",
            "function_call",
            "order",
            "reduce_assignment",
            "local_return",
            "local_function",
            "match",
            "select",
            "require",
            "distinct",
            "reduce",
            "sort",
            "offset",
            "limit",
            "document_binding",
            "document_attribute_list",
            "finalize_rows",
            "finalize_documents",
        ]
    );
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("fixture type id")
}

fn attribute(label: &str) -> AttributeId {
    AttributeId::new(label).expect("fixture attribute id")
}

fn binding_id(value: u16) -> BindingId {
    BindingId::new(value).expect("fixture binding id")
}

fn binding(value: u16, name: &str) -> AssertionBinding {
    AssertionBinding::new(
        binding_id(value),
        QueryVariable::new(name).expect("fixture query variable"),
    )
}

fn schema_authority() -> Arc<QueryAuthority> {
    let person = type_id(TypeKind::Entity, "person");
    let edge = type_id(TypeKind::Relation, "edge");
    let origin = RoleId::new("edge", "origin").expect("origin role");
    let destination = RoleId::new("edge", "destination").expect("destination role");

    let scalar_attributes = [
        ("name", ValueTypeTag::String),
        ("age", ValueTypeTag::Long),
        ("ratio", ValueTypeTag::Double),
        ("active", ValueTypeTag::Boolean),
        ("born", ValueTypeTag::Date),
        ("seen", ValueTypeTag::DateTime),
        ("zoned", ValueTypeTag::DateTimeTz),
        ("amount", ValueTypeTag::Decimal),
        ("elapsed", ValueTypeTag::Duration),
    ];

    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person.clone()).expect("person type")),
        SchemaFact::Type(TypeFact::new(edge.clone()).expect("edge type")),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), origin.clone()).expect("origin relates id"),
                None,
            )
            .expect("origin relates"),
        ),
        SchemaFact::Relates(
            RelatesFact::new(
                RelatesFactId::new(edge.clone(), destination.clone())
                    .expect("destination relates id"),
                None,
            )
            .expect("destination relates"),
        ),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), origin).expect("origin plays"),
        )),
        SchemaFact::Plays(PlaysFact::new(
            PlaysFactId::new(person.clone(), destination).expect("destination plays"),
        )),
    ];

    for (label, value_type) in scalar_attributes {
        let attribute = attribute(label);
        facts.push(SchemaFact::Type(
            TypeFact::new(type_id(TypeKind::Attribute, label)).expect("attribute type"),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(attribute.clone()),
            value_type,
        )));
        facts.push(SchemaFact::Owns(OwnsFact::new(
            OwnsFactId::new(person.clone(), attribute).expect("owns id"),
        )));
    }

    let name_owns = OwnsFactId::new(person.clone(), attribute("name")).expect("name owns");
    facts.push(SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(name_owns),
                AnnotationKindId::Unique,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("unique name annotation"),
    ));
    facts.push(SchemaFact::Function(FunctionFact::new(
        FunctionId::new("schema_name_count").expect("schema function id"),
        FunctionSignature::new(
            vec![FunctionParameter::new(
                Label::new("subject").expect("parameter label"),
                TypeReference::Schema(Label::new("person").expect("person label")),
            )],
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Value(ValueTypeTag::Long),
                false,
            )),
        )
        .expect("function signature"),
        FunctionBody::new("match $subject has name $name; return count($name);")
            .expect("function body"),
    )));

    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let byte = u64::try_from(index).expect("source byte");
        let line = u32::try_from(index + 1).expect("source line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("query-v2-builder-fixture").expect("document id"),
                byte,
                byte + 1,
                line,
                1,
                line,
                2,
            )
            .expect("source span"),
        )
    });
    let declared = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("declared schema");
    Arc::new(
        QueryAuthority::from_declared_bytes(
            &encode_declared_schema(&declared).expect("declared bytes"),
            SCOPE,
            PROFILE,
        )
        .expect("query authority"),
    )
}

fn basic_authored_plan(authority: Arc<QueryAuthority>, noise: bool) -> AuthoredQueryPlan {
    let mut builder = QueryPlanBuilder::new(authority);
    if noise {
        let unused = builder.binding("unused").expect("unused binding");
        let _unused_operand = builder
            .binding_operand(&unused)
            .expect("unused operand handle");
        let _unused_literal = builder
            .literal_operand(CanonicalValue::Long(99))
            .expect("unused literal handle");
    }
    let person = builder.binding("person").expect("person binding");
    if noise {
        let _another_unused = builder.binding("another_unused").expect("unused binding");
    }
    let name = builder.binding("name").expect("name binding");
    let wanted = builder
        .input("wanted_name", ValueTypeTag::String, false)
        .expect("input");
    let person_isa = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("isa");
    let name_has = builder.has(&person, &name, attribute("name")).expect("has");
    let name_operand = builder.binding_operand(&name).expect("binding operand");
    let wanted_operand = builder.input_operand(&wanted).expect("input operand");
    let comparison = builder
        .value(ValueComparator::Equal, &name_operand, &wanted_operand)
        .expect("value");
    builder
        .r#match(vec![person_isa, name_has, comparison])
        .expect("match");
    builder
        .select(vec![name.clone(), person.clone()])
        .expect("canonicalized select");
    builder
        .finalize_rows(vec![person, name])
        .expect("finalized plan")
}

#[test]
fn basic_builder_matches_direct_v2_bytes_fingerprint_and_capabilities() {
    let authority = schema_authority();
    let authored = basic_authored_plan(Arc::clone(&authority), false);
    let person = binding_id(0);
    let name = binding_id(1);
    let direct = QueryPlan::new_v2(
        vec![binding(0, "person"), binding(1, "name")],
        vec![InputColumn::new(
            InputColumnId::new(0),
            QueryVariable::new("wanted_name").expect("input variable"),
            ValueTypeTag::String,
            false,
        )],
        vec![
            ReadStage::Match {
                patterns: vec![
                    QueryPattern::Isa {
                        binding: person,
                        include_subtypes: false,
                        type_id: type_id(TypeKind::Entity, "person"),
                    },
                    QueryPattern::Has {
                        attribute: name,
                        attribute_id: attribute("name"),
                        owner: person,
                    },
                    QueryPattern::Value {
                        comparator: ValueComparator::Equal,
                        left: QueryOperand::Binding { binding: name },
                        right: QueryOperand::Input {
                            column: InputColumnId::new(0),
                        },
                    },
                ],
            },
            ReadStage::Select {
                bindings: vec![person, name],
            },
        ],
        QueryOutput::Rows {
            columns: vec![person, name],
        },
        authority
            .context()
            .managed_state()
            .managed_semantic_schema()
            .clone(),
    )
    .expect("direct V2 plan");

    assert_eq!(authored.format(), "typebridge.query-plan/v2");
    assert_eq!(
        authored.canonical_bytes(),
        direct.canonical_bytes().expect("direct canonical bytes")
    );
    assert_eq!(
        authored.fingerprint(),
        &direct.fingerprint().expect("direct fingerprint")
    );
    assert_eq!(
        authored.required_capabilities(),
        direct
            .required_capabilities()
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        authored
            .required_capabilities()
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    );
}

#[test]
fn irrelevant_handle_allocation_does_not_change_canonical_identity() {
    let authority = schema_authority();
    let plain = basic_authored_plan(Arc::clone(&authority), false);
    let noisy = basic_authored_plan(authority, true);
    assert_eq!(plain.canonical_bytes(), noisy.canonical_bytes());
    assert_eq!(plain.fingerprint(), noisy.fingerprint());
    assert_eq!(plain.required_capabilities(), noisy.required_capabilities());
}

#[test]
fn cross_builder_cross_authority_and_scope_failures_are_atomic() {
    let authority = schema_authority();
    let foreign_authority = schema_authority();
    let mut first = QueryPlanBuilder::new(Arc::clone(&authority));
    let mut same_authority = QueryPlanBuilder::new(authority);
    let mut other_authority = QueryPlanBuilder::new(foreign_authority);
    let person = first.binding("person").expect("person binding");
    let same_person = same_authority
        .binding("same_person")
        .expect("same-authority binding");
    let other_person = other_authority
        .binding("other_person")
        .expect("other-authority binding");

    assert_eq!(
        first
            .isa(&same_person, type_id(TypeKind::Entity, "person"), false)
            .expect_err("cross-builder handle")
            .code()
            .as_str(),
        "query_builder_cross_builder_handle"
    );
    assert_eq!(
        first
            .isa(&other_person, type_id(TypeKind::Entity, "person"), false)
            .expect_err("cross-authority handle")
            .code()
            .as_str(),
        "query_builder_cross_authority_handle"
    );

    let local_person = first.binding("local_person").expect("local person");
    let local_name = first.binding("local_name").expect("local name");
    let local_isa = first
        .isa(&local_person, type_id(TypeKind::Entity, "person"), false)
        .expect("local isa");
    let local_has = first
        .has(&local_person, &local_name, attribute("name"))
        .expect("local has");
    let local_return = first
        .local_return(
            type_bridge_contract::query_plan::Reducer::Count,
            &local_name,
            ValueTypeTag::Long,
        )
        .expect("local return");
    first
        .local_function(
            FunctionId::new("local_count").expect("local function id"),
            vec![local_name.clone(), local_person.clone()],
            vec![(
                local_person.clone(),
                Label::new("person").expect("parameter type"),
            )],
            vec![local_isa.clone(), local_has],
            &local_return,
        )
        .expect("local function");

    assert_eq!(
        first
            .r#match(vec![local_isa])
            .expect_err("local symbol cannot become root")
            .code()
            .as_str(),
        "query_builder_binding_already_local"
    );

    let root_isa = first
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("root isa");
    first
        .r#match(vec![root_isa])
        .expect("failed scope claim left builder usable");
    let root_return = first
        .local_return(
            type_bridge_contract::query_plan::Reducer::Count,
            &person,
            ValueTypeTag::Long,
        )
        .expect("root return handle");
    assert_eq!(
        first
            .local_function(
                FunctionId::new("invalid_root_reuse").expect("function id"),
                vec![person.clone()],
                vec![(
                    person.clone(),
                    Label::new("person").expect("parameter type"),
                )],
                vec![
                    first
                        .isa(&person, type_id(TypeKind::Entity, "person"), false)
                        .expect("root pattern")
                ],
                &root_return,
            )
            .expect_err("root binding cannot be consumed")
            .code()
            .as_str(),
        "query_builder_binding_already_root"
    );
    first
        .finalize_rows(vec![person])
        .expect("failed local claim left root plan usable");
}

#[test]
fn successful_finalization_is_terminal_and_preserves_the_first_value() {
    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let person = builder.binding("person").expect("binding");
    assert_eq!(
        builder
            .finalize_rows(vec![person.clone()])
            .expect_err("finalization requires an attached terminal pipeline")
            .code()
            .as_str(),
        "query_builder_match_required"
    );
    let pattern = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("pattern");
    builder.r#match(vec![pattern]).expect("match");
    let first = builder
        .finalize_rows(vec![person.clone()])
        .expect("first finalization");
    let first_bytes = first.canonical_bytes();

    assert_eq!(
        builder
            .finalize_rows(vec![person.clone()])
            .expect_err("repeat finalization")
            .code()
            .as_str(),
        "query_builder_finalized"
    );
    assert_eq!(
        builder
            .binding("late")
            .expect_err("use after finalize")
            .code()
            .as_str(),
        "query_builder_finalized"
    );
    assert_eq!(
        builder
            .finalized_plan()
            .expect("stored first finalized value")
            .canonical_bytes(),
        first_bytes
    );
}

#[test]
fn advanced_builder_covers_boolean_functions_reducers_and_every_stage() {
    use type_bridge_contract::query_plan::Reducer;

    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);

    let local_person = builder.binding("lp").expect("local person");
    let local_name = builder.binding("ln").expect("local name");
    let local_isa = builder
        .isa(&local_person, type_id(TypeKind::Entity, "person"), false)
        .expect("local isa");
    let local_has = builder
        .has(&local_person, &local_name, attribute("name"))
        .expect("local has");
    let local_return = builder
        .local_return(Reducer::Count, &local_name, ValueTypeTag::Long)
        .expect("local return");
    let local_function = builder
        .local_function(
            FunctionId::new("local_name_count").expect("function id"),
            vec![local_name.clone(), local_person.clone()],
            vec![(local_person, Label::new("person").expect("parameter type"))],
            vec![local_isa, local_has],
            &local_return,
        )
        .expect("local function");

    let sum_person = builder.binding("sum_person").expect("sum person");
    let sum_age = builder.binding("sum_age").expect("sum age");
    let sum_isa = builder
        .isa(&sum_person, type_id(TypeKind::Entity, "person"), false)
        .expect("sum isa");
    let sum_has = builder
        .has(&sum_person, &sum_age, attribute("age"))
        .expect("sum has");
    let sum_return = builder
        .local_return(Reducer::Sum, &sum_age, ValueTypeTag::Long)
        .expect("sum return");
    builder
        .local_function(
            FunctionId::new("local_age_sum").expect("function id"),
            vec![sum_age, sum_person.clone()],
            vec![(sum_person, Label::new("person").expect("parameter type"))],
            vec![sum_isa, sum_has],
            &sum_return,
        )
        .expect("sum local function");

    let person = builder.binding("person").expect("person");
    let name = builder.binding("name").expect("name");
    let age = builder.binding("age").expect("age");
    let optional_active = builder.binding("optional_active").expect("active");
    let schema_result = builder.binding("schema_result").expect("schema result");
    let local_result = builder.binding("local_result").expect("local result");
    let count = builder.binding("count_result").expect("count result");
    let maximum = builder.binding("max_result").expect("max result");
    let mean = builder.binding("mean_result").expect("mean result");
    let minimum = builder.binding("min_result").expect("min result");
    let sum = builder.binding("sum_result").expect("sum result");

    let person_isa = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), true)
        .expect("person isa");
    let name_has = builder
        .has(&person, &name, attribute("name"))
        .expect("name has");
    let age_has = builder
        .has(&person, &age, attribute("age"))
        .expect("age has");
    let name_operand = builder.binding_operand(&name).expect("name operand");
    let ada = builder
        .literal_operand(CanonicalValue::String(
            CanonicalString::new("ada").expect("canonical string"),
        ))
        .expect("literal");
    let equal = builder
        .value(ValueComparator::Equal, &name_operand, &ada)
        .expect("equal");
    let not_equal = builder
        .value(ValueComparator::NotEqual, &name_operand, &ada)
        .expect("not equal");
    let disjunction = builder
        .or(vec![vec![equal.clone()], vec![not_equal.clone()]])
        .expect("or");
    let negation = builder.not(vec![not_equal]).expect("not");
    let active_has = builder
        .has(&person, &optional_active, attribute("active"))
        .expect("optional has");
    let active_operand = builder
        .binding_operand(&optional_active)
        .expect("active operand");
    let yes = builder
        .literal_operand(CanonicalValue::Boolean(true))
        .expect("boolean literal");
    let active_value = builder
        .value(ValueComparator::Equal, &active_operand, &yes)
        .expect("active value");
    let optional = builder.r#try(vec![active_has, active_value]).expect("try");
    let person_operand = builder.binding_operand(&person).expect("person operand");
    let schema_call = builder
        .function_call(
            &schema_result,
            QueryFunctionTarget::Schema(
                FunctionId::new("schema_name_count").expect("schema function"),
            ),
            vec![person_operand.clone()],
        )
        .expect("schema call");
    let local_call = builder
        .function_call(
            &local_result,
            QueryFunctionTarget::Local(&local_function),
            vec![person_operand],
        )
        .expect("local call");

    builder
        .r#match(vec![
            person_isa,
            name_has,
            age_has,
            disjunction,
            negation,
            optional,
            schema_call,
            local_call,
        ])
        .expect("advanced match");
    builder
        .select(vec![
            local_result,
            age.clone(),
            name.clone(),
            person,
            schema_result,
        ])
        .expect("select");
    builder.require(vec![name.clone()]).expect("require");
    builder.distinct().expect("distinct");

    let assignments = vec![
        builder
            .reduce_assignment(&count, Reducer::Count, None)
            .expect("count"),
        builder
            .reduce_assignment(&maximum, Reducer::Max, Some(&age))
            .expect("max"),
        builder
            .reduce_assignment(&mean, Reducer::Mean, Some(&age))
            .expect("mean"),
        builder
            .reduce_assignment(&minimum, Reducer::Min, Some(&age))
            .expect("min"),
        builder
            .reduce_assignment(&sum, Reducer::Sum, Some(&age))
            .expect("sum"),
    ];
    builder
        .reduce(assignments, vec![name.clone()])
        .expect("reduce");
    let name_order = builder
        .order(&name, OrderDirection::Ascending)
        .expect("order");
    let count_order = builder
        .order(&count, OrderDirection::Descending)
        .expect("descending order");
    builder.sort(vec![name_order, count_order]).expect("sort");
    builder.offset(0).expect("offset");
    builder.limit(10).expect("limit");

    let authored = builder
        .finalize_rows(vec![name, count, maximum, mean, minimum, sum])
        .expect("advanced finalization");
    assert_eq!(
        authored.contract_plan().functions()[0]
            .bindings()
            .iter()
            .map(|binding| binding.variable().as_str())
            .collect::<Vec<_>>(),
        vec!["lp", "ln"],
        "explicit parameter order leads the private dense binding table",
    );
    let expected = [
        "query.function.local",
        "query.pattern.disjunction",
        "query.pattern.function-call",
        "query.pattern.negation",
        "query.pattern.try",
        "query.stage.distinct",
        "query.stage.limit",
        "query.stage.offset",
        "query.stage.reduce",
        "query.stage.require",
        "query.stage.select",
        "query.stage.sort",
    ];
    for capability in expected {
        assert!(
            authored
                .required_capabilities()
                .iter()
                .any(|actual| actual == capability),
            "missing {capability}"
        );
    }
}

#[test]
fn links_and_reachability_author_through_the_same_builder() {
    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let source = builder.binding("source").expect("source");
    let target = builder.binding("target").expect("target");
    let edge = builder.binding("edge").expect("edge");
    let source_isa = builder
        .isa(&source, type_id(TypeKind::Entity, "person"), false)
        .expect("source isa");
    let target_isa = builder
        .isa(&target, type_id(TypeKind::Entity, "person"), false)
        .expect("target isa");
    let links = builder
        .links(
            &edge,
            type_id(TypeKind::Relation, "edge"),
            vec![
                (
                    RoleId::new("edge", "origin").expect("origin"),
                    source.clone(),
                ),
                (
                    RoleId::new("edge", "destination").expect("destination"),
                    target.clone(),
                ),
            ],
        )
        .expect("links");
    let reachable = builder
        .reachable(
            &source,
            &target,
            type_id(TypeKind::Relation, "edge"),
            RoleId::new("edge", "origin").expect("origin"),
            RoleId::new("edge", "destination").expect("destination"),
            0,
            3,
        )
        .expect("reachable");
    builder
        .r#match(vec![source_isa, target_isa, links, reachable])
        .expect("match");
    let authored = builder
        .finalize_rows(vec![source, target, edge])
        .expect("finalize");
    for capability in ["query.pattern.links", "query.pattern.reachable"] {
        assert!(
            authored
                .required_capabilities()
                .iter()
                .any(|actual| actual == capability)
        );
    }
}

#[test]
fn documents_and_all_invocation_terminals_are_plan_bound() {
    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let person = builder.binding("person").expect("person");
    let name = builder.binding("name").expect("name");
    let person_isa = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("isa");
    let name_has = builder.has(&person, &name, attribute("name")).expect("has");
    builder.r#match(vec![person_isa, name_has]).expect("match");
    let scalar = builder
        .document_binding("primary_name", &name)
        .expect("document scalar");
    let list = builder
        .document_attribute_list("all_names", &person, attribute("name"))
        .expect("document list");
    let plan = builder
        .finalize_documents(vec![scalar, list])
        .expect("documents plan");

    let documents = plan.documents(Vec::new()).expect("documents invocation");
    let count = plan.count(Vec::new()).expect("count invocation");
    let exists = plan.exists(Vec::new()).expect("exists invocation");
    for invocation in [&documents, &count, &exists] {
        assert_eq!(invocation.plan_fingerprint(), plan.fingerprint());
        assert_eq!(invocation.authority_identity(), plan.authority_identity());
        assert!(!invocation.canonical_bytes().is_empty());
    }
    assert_eq!(
        plan.rows(Vec::new())
            .expect_err("row terminal on documents")
            .code()
            .as_str(),
        "query_builder_output_operation_mismatch"
    );
}

fn canonical_scalar_values() -> Vec<CanonicalValue> {
    let date = CanonicalDate::new(2026, 7, 24).expect("date");
    let time = CanonicalTime::new(12, 30, 45, 123).expect("time");
    let datetime = CanonicalDateTime::new(date, time);
    vec![
        CanonicalValue::String(CanonicalString::new("value").expect("string")),
        CanonicalValue::Long(42),
        CanonicalValue::Double(CanonicalDouble::new(1.5).expect("double")),
        CanonicalValue::Boolean(true),
        CanonicalValue::Date(date),
        CanonicalValue::DateTime(datetime),
        CanonicalValue::DateTimeTz(
            CanonicalDateTimeTz::new(datetime, TimeZoneDesignator::Utc).expect("datetime tz"),
        ),
        CanonicalValue::Decimal(DecimalValue::new("12.50").expect("decimal")),
        CanonicalValue::Duration(CanonicalDuration::new(false, 1, 2, 3, 4).expect("duration")),
    ]
}

#[test]
fn every_scalar_literal_and_input_domain_round_trips_through_authoring() {
    let authority = schema_authority();
    let labels = [
        "name", "age", "ratio", "active", "born", "seen", "zoned", "amount", "elapsed",
    ];
    let values = canonical_scalar_values();
    let mut builder = QueryPlanBuilder::new(authority);
    let person = builder.binding("person").expect("person");
    let mut patterns = vec![
        builder
            .isa(&person, type_id(TypeKind::Entity, "person"), false)
            .expect("isa"),
    ];

    for (index, (label, value)) in labels.iter().zip(&values).enumerate() {
        let scalar = builder
            .binding(format!("scalar_{index}"))
            .expect("scalar binding");
        patterns.push(
            builder
                .has(&person, &scalar, attribute(label))
                .expect("has"),
        );
        let scalar_operand = builder.binding_operand(&scalar).expect("binding operand");
        let literal = builder
            .literal_operand(value.clone())
            .expect("literal operand");
        patterns.push(
            builder
                .value(ValueComparator::Equal, &scalar_operand, &literal)
                .expect("literal comparison"),
        );
        if *label == "age" {
            for comparator in [
                ValueComparator::Less,
                ValueComparator::LessOrEqual,
                ValueComparator::Greater,
                ValueComparator::GreaterOrEqual,
            ] {
                let boundary = builder
                    .literal_operand(CanonicalValue::Long(100))
                    .expect("ordered comparison literal");
                patterns.push(
                    builder
                        .value(comparator, &scalar_operand, &boundary)
                        .expect("ordered comparison"),
                );
            }
        }
        let input = builder
            .input(
                format!("input_{index}"),
                value.value_type(),
                index == values.len() - 1,
            )
            .expect("typed input");
        let input_operand = builder.input_operand(&input).expect("input operand");
        patterns.push(
            builder
                .value(ValueComparator::Equal, &scalar_operand, &input_operand)
                .expect("input comparison"),
        );
    }

    builder.r#match(patterns).expect("match");
    let plan = builder
        .finalize_rows(vec![person])
        .expect("scalar plan finalization");
    let present = values.iter().cloned().map(Some).collect::<Vec<_>>();
    let mut explicit_absence = values.into_iter().map(Some).collect::<Vec<_>>();
    *explicit_absence.last_mut().expect("optional input column") = None;
    let invocation = plan
        .rows(vec![present, explicit_absence])
        .expect("typed invocation with explicit optional absence");
    assert_eq!(invocation.plan_fingerprint(), plan.fingerprint());
    assert_eq!(
        invocation.required_transport_capabilities(),
        ["query.input.given-rows"],
        "datetime-tz inputs select the exact invocation-derived capability",
    );
}

#[test]
fn generated_stage_transitions_and_deep_nesting_never_panic() {
    let transition_authority = schema_authority();
    for length in 1..=5 {
        let combinations = 5usize.pow(u32::try_from(length).expect("small length"));
        for encoded in 0..combinations {
            let result = catch_unwind(AssertUnwindSafe(|| {
                let mut builder = QueryPlanBuilder::new(Arc::clone(&transition_authority));
                let person = builder.binding("person").expect("person");
                let pattern = builder
                    .isa(&person, type_id(TypeKind::Entity, "person"), false)
                    .expect("isa");
                builder.r#match(vec![pattern]).expect("match");
                let mut value = encoded;
                for _ in 0..length {
                    let action = value % 5;
                    value /= 5;
                    match action {
                        0 => {
                            let _ = builder.select(vec![person.clone()]);
                        }
                        1 => {
                            let _ = builder.require(vec![person.clone()]);
                        }
                        2 => {
                            let _ = builder.distinct();
                        }
                        3 => {
                            let _ = builder.offset(0);
                        }
                        4 => {
                            let _ = builder.limit(1);
                        }
                        _ => unreachable!("modulo five"),
                    }
                }
            }));
            assert!(
                result.is_ok(),
                "transition sequence {encoded}/{length} panicked"
            );
        }
    }

    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let person = builder.binding("person").expect("person");
    let base = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("base pattern");
    let mut nested = base.clone();
    let mut depth_error = None;
    for _ in 0..=MAX_NESTING_DEPTH {
        match catch_unwind(AssertUnwindSafe(|| builder.not(vec![nested.clone()]))) {
            Ok(Ok(next)) => nested = next,
            Ok(Err(error)) => {
                depth_error = Some(error);
                break;
            }
            Err(_) => panic!("deep pattern composition panicked"),
        }
    }
    assert_eq!(
        depth_error
            .expect("deep tree rejected at composition")
            .code()
            .as_str(),
        "query_plan_pattern_depth_limit"
    );
    builder
        .r#match(vec![base])
        .expect("failed deep attachment did not claim or corrupt state");
    builder
        .finalize_rows(vec![person])
        .expect("builder remained usable");

    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let person = builder.binding("person").expect("person");
    let base = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("base pattern");
    let mut reused = base.clone();
    let node_error = loop {
        match catch_unwind(AssertUnwindSafe(|| {
            builder.or(vec![
                vec![reused.clone()],
                vec![reused.clone(), reused.clone()],
            ])
        })) {
            Ok(Ok(next)) => reused = next,
            Ok(Err(error)) => break error,
            Err(_) => panic!("reused-subtree composition panicked"),
        }
    };
    assert_eq!(node_error.code().as_str(), "query_plan_pattern_node_limit");
    builder
        .r#match(vec![base])
        .expect("prior bounded handle remained usable");
    builder
        .finalize_rows(vec![person])
        .expect("failed reused-subtree composition did not corrupt builder");
}

const MAX_NESTING_DEPTH: usize = 64;

#[test]
fn root_only_patterns_reject_nested_composition_at_the_handle_boundary() {
    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let person = builder.binding("person").expect("person");
    let target = builder.binding("target").expect("target");
    let optional_name = builder.binding("optional_name").expect("optional name");
    let assigned = builder.binding("assigned").expect("assigned");
    let base = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("base");
    let optional_has = builder
        .has(&person, &optional_name, attribute("name"))
        .expect("optional has");
    let optional = builder.r#try(vec![optional_has]).expect("try");
    let reachable = builder
        .reachable(
            &person,
            &target,
            type_id(TypeKind::Relation, "edge"),
            RoleId::new("edge", "origin").expect("origin"),
            RoleId::new("edge", "destination").expect("destination"),
            1,
            2,
        )
        .expect("reachable");
    let person_operand = builder.binding_operand(&person).expect("person operand");
    let function = builder
        .function_call(
            &assigned,
            QueryFunctionTarget::Schema(
                FunctionId::new("schema_name_count").expect("schema function"),
            ),
            vec![person_operand],
        )
        .expect("function call");

    for (pattern, code) in [
        (optional, "query_plan_try_not_root"),
        (reachable, "query_plan_reachable_not_root"),
        (function, "query_plan_function_in_negation"),
    ] {
        let not_result = catch_unwind(AssertUnwindSafe(|| builder.not(vec![pattern.clone()])))
            .expect("not composition must not panic")
            .expect_err("root-only child rejected by not");
        assert_eq!(not_result.code().as_str(), code);
        let or_result = catch_unwind(AssertUnwindSafe(|| {
            builder.or(vec![vec![base.clone()], vec![pattern.clone()]])
        }))
        .expect("or composition must not panic")
        .expect_err("root-only child rejected by or");
        assert_eq!(or_result.code().as_str(), code);
    }

    builder
        .r#match(vec![base])
        .expect("failed nested compositions did not claim symbols");
    builder
        .finalize_rows(vec![person])
        .expect("builder and prior handle remained usable");
}

#[test]
fn declaration_collisions_and_failed_stage_appends_do_not_mutate_state() {
    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = builder.binding("person").expect("person");
    let pattern = builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("isa");
    builder.r#match(vec![pattern]).expect("match");
    assert_eq!(
        builder
            .input("person", ValueTypeTag::String, false)
            .expect_err("input cannot collide with an attached root")
            .code()
            .as_str(),
        "query_plan_duplicate_variable"
    );
    builder.select(vec![person.clone()]).expect("first select");
    assert_eq!(
        builder
            .select(vec![person.clone()])
            .expect_err("duplicate select")
            .code()
            .as_str(),
        "query_plan_stage_order"
    );
    builder
        .require(vec![person.clone()])
        .expect("failed duplicate append left stage ordinal unchanged");
    builder.distinct().expect("later stage remains appendable");
    builder
        .finalize_rows(vec![person])
        .expect("builder remained valid");

    let mut collision = QueryPlanBuilder::new(authority);
    collision
        .input("collision", ValueTypeTag::String, false)
        .expect("input");
    let first = collision.binding("collision").expect("first collision");
    let second = collision.binding("collision").expect("second collision");
    let first_pattern = collision
        .isa(&first, type_id(TypeKind::Entity, "person"), false)
        .expect("first pattern");
    let second_pattern = collision
        .isa(&second, type_id(TypeKind::Entity, "person"), false)
        .expect("second pattern");
    assert_eq!(
        collision
            .r#match(vec![first_pattern, second_pattern])
            .expect_err("root/input and duplicate-root collision")
            .code()
            .as_str(),
        "query_plan_duplicate_variable"
    );
    let valid = collision.binding("valid_root").expect("valid root");
    let valid_pattern = collision
        .isa(&valid, type_id(TypeKind::Entity, "person"), false)
        .expect("valid pattern");
    collision
        .r#match(vec![valid_pattern])
        .expect("failed collision claim was atomic");
    collision
        .finalize_rows(vec![valid])
        .expect("unscoped colliding handles do not enter the plan");
}

#[test]
fn malformed_local_consumption_and_reducer_shapes_fail_before_scope_mutation() {
    use type_bridge_contract::query_plan::Reducer;

    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let local_person = builder.binding("local_person").expect("local person");
    let omitted = builder.binding("omitted").expect("omitted");
    let unused = builder.binding("unused").expect("unused");
    let pattern = builder
        .isa(&local_person, type_id(TypeKind::Entity, "person"), false)
        .expect("pattern");
    let returns = builder
        .local_return(Reducer::Count, &omitted, ValueTypeTag::Long)
        .expect("return");
    assert_eq!(
        builder
            .local_function(
                FunctionId::new("bad_omission").expect("function"),
                vec![local_person.clone(), unused.clone()],
                vec![(
                    local_person.clone(),
                    Label::new("person").expect("parameter"),
                )],
                vec![pattern.clone()],
                &returns,
            )
            .expect_err("return binding omitted")
            .code()
            .as_str(),
        "query_builder_local_binding_omitted"
    );

    let valid_return = builder
        .local_return(Reducer::Count, &local_person, ValueTypeTag::Long)
        .expect("valid return");
    assert_eq!(
        builder
            .local_function(
                FunctionId::new("bad_unused").expect("function"),
                vec![local_person.clone(), unused],
                vec![(
                    local_person.clone(),
                    Label::new("person").expect("parameter"),
                )],
                vec![pattern.clone()],
                &valid_return,
            )
            .expect_err("consumed binding must be used")
            .code()
            .as_str(),
        "query_builder_unused_local_binding"
    );
    builder
        .local_function(
            FunctionId::new("still_usable").expect("function"),
            vec![local_person.clone()],
            vec![(local_person, Label::new("person").expect("parameter"))],
            vec![pattern],
            &valid_return,
        )
        .expect("failed local attachment did not claim symbols");

    let root = builder.binding("root").expect("root");
    assert_eq!(
        builder
            .reduce_assignment(&root, Reducer::Count, Some(&root))
            .expect_err("count input rejected")
            .code()
            .as_str(),
        "query_plan_reduce_missing_input"
    );
    assert_eq!(
        builder
            .reduce_assignment(&root, Reducer::Mean, None)
            .expect_err("mean input required")
            .code()
            .as_str(),
        "query_plan_reduce_missing_input"
    );

    let root_pattern = builder
        .isa(&root, type_id(TypeKind::Entity, "person"), false)
        .expect("root pattern");
    builder.r#match(vec![root_pattern]).expect("root match");
    let partial = builder
        .reduce_assignment(&omitted, Reducer::Mean, Some(&root))
        .expect("partial reducer assignment");
    assert_eq!(
        builder
            .reduce(vec![partial], Vec::new())
            .expect_err("partial reducer without groups")
            .code()
            .as_str(),
        "query_plan_reduce_requires_groups"
    );
    let total = builder
        .reduce_assignment(&omitted, Reducer::Count, None)
        .expect("total reducer assignment");
    builder
        .reduce(vec![total], Vec::new())
        .expect("failed reduce append did not claim or advance state");
    builder
        .finalize_rows(vec![omitted])
        .expect("builder remained valid after rejected reducer");
}

#[test]
fn reserved_optional_require_and_unordered_truncation_reject_before_append() {
    let authority = schema_authority();
    let mut optional_builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = optional_builder.binding("person").expect("person");
    let optional_name = optional_builder
        .binding("optional_name")
        .expect("optional name");
    let person_isa = optional_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("person isa");
    let optional_has = optional_builder
        .has(&person, &optional_name, attribute("name"))
        .expect("optional has");
    let optional = optional_builder
        .r#try(vec![optional_has])
        .expect("try pattern");
    optional_builder
        .r#match(vec![person_isa, optional])
        .expect("match");
    assert_eq!(
        optional_builder
            .require(vec![optional_name])
            .expect_err("requiring an optional binding is reserved")
            .code()
            .as_str(),
        "query_plan_require_optional_reserved"
    );
    optional_builder
        .require(vec![person.clone()])
        .expect("rejected require did not append or advance the builder");
    optional_builder
        .finalize_rows(vec![person])
        .expect("repaired optional plan");

    let mut truncation_builder = QueryPlanBuilder::new(authority);
    let person = truncation_builder.binding("person").expect("person");
    let name = truncation_builder.binding("name").expect("name");
    let person_isa = truncation_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("person isa");
    let name_has = truncation_builder
        .has(&person, &name, attribute("name"))
        .expect("name has");
    truncation_builder
        .r#match(vec![person_isa, name_has])
        .expect("match");
    assert_eq!(
        truncation_builder
            .limit(1)
            .expect_err("truncation requires a total sort before append")
            .code()
            .as_str(),
        "query_plan_unordered_truncation"
    );
    let order = truncation_builder
        .order(&name, OrderDirection::Ascending)
        .expect("unique name order");
    truncation_builder
        .sort(vec![order])
        .expect("rejected limit left the sort stage available");
    truncation_builder.limit(1).expect("ordered limit");
    truncation_builder
        .finalize_rows(vec![person])
        .expect("repaired truncation plan");
}

#[test]
fn invisible_later_stages_reject_atomically_and_remain_repairable() {
    use type_bridge_contract::query_plan::Reducer;

    fn matched_builder() -> (
        QueryPlanBuilder,
        type_bridge_orm::query_v2_builder::QueryBindingHandle,
        type_bridge_orm::query_v2_builder::QueryBindingHandle,
        type_bridge_orm::query_v2_builder::QueryBindingHandle,
    ) {
        let mut builder = QueryPlanBuilder::new(schema_authority());
        let person = builder.binding("person").expect("person");
        let name = builder.binding("name").expect("name");
        let ghost = builder.binding("ghost").expect("unmatched ghost");
        let person_isa = builder
            .isa(&person, type_id(TypeKind::Entity, "person"), false)
            .expect("person isa");
        let name_has = builder
            .has(&person, &name, attribute("name"))
            .expect("name has");
        builder.r#match(vec![person_isa, name_has]).expect("match");
        (builder, person, name, ghost)
    }

    let (mut select_builder, person, _name, ghost) = matched_builder();
    assert_eq!(
        select_builder
            .select(vec![ghost])
            .expect_err("select cannot introduce an invisible binding")
            .code()
            .as_str(),
        "query_plan_stage_unknown_binding",
    );
    select_builder
        .select(vec![person.clone()])
        .expect("rejected select did not append");
    select_builder
        .finalize_rows(vec![person])
        .expect("select builder remains usable");

    let (mut require_builder, person, _name, ghost) = matched_builder();
    assert_eq!(
        require_builder
            .require(vec![ghost])
            .expect_err("require cannot reference an invisible binding")
            .code()
            .as_str(),
        "query_plan_stage_unknown_binding",
    );
    require_builder
        .require(vec![person.clone()])
        .expect("rejected require did not append");
    require_builder
        .finalize_rows(vec![person])
        .expect("require builder remains usable");

    let (mut sort_builder, person, name, ghost) = matched_builder();
    let ghost_order = sort_builder
        .order(&ghost, OrderDirection::Ascending)
        .expect("ghost order handle");
    assert_eq!(
        sort_builder
            .sort(vec![ghost_order])
            .expect_err("sort cannot reference an invisible binding")
            .code()
            .as_str(),
        "query_plan_stage_unknown_binding",
    );
    let person_order = sort_builder
        .order(&name, OrderDirection::Ascending)
        .expect("unique name order handle");
    sort_builder
        .sort(vec![person_order])
        .expect("rejected sort did not append");
    sort_builder
        .finalize_rows(vec![person])
        .expect("sort builder remains usable");

    let (mut reduce_builder, person, _name, ghost) = matched_builder();
    let invalid = reduce_builder
        .reduce_assignment(&person, Reducer::Count, None)
        .expect("pattern-bound assignment handle");
    assert_eq!(
        reduce_builder
            .reduce(vec![invalid], Vec::new())
            .expect_err("reduce cannot overwrite a pattern binding")
            .code()
            .as_str(),
        "query_plan_reduce_assigned_bound",
    );
    let valid = reduce_builder
        .reduce_assignment(&ghost, Reducer::Count, None)
        .expect("fresh assignment");
    reduce_builder
        .reduce(vec![valid], Vec::new())
        .expect("rejected reduce did not append or claim");
    reduce_builder
        .finalize_rows(vec![ghost])
        .expect("reduce builder remains usable");
}

#[test]
fn schema_invalid_match_and_local_function_transitions_are_atomic() {
    use type_bridge_contract::query_plan::Reducer;

    let authority = schema_authority();

    let mut unknown_type_builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = unknown_type_builder.binding("person").expect("person");
    let unknown = unknown_type_builder
        .isa(&person, type_id(TypeKind::Entity, "missing_person"), false)
        .expect("structurally valid unknown type");
    assert_eq!(
        unknown_type_builder
            .r#match(vec![unknown])
            .expect_err("unknown schema type must fail at the mutating transition")
            .code()
            .as_str(),
        "query_plan_unknown_type",
    );
    let valid = unknown_type_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("valid person type");
    unknown_type_builder
        .r#match(vec![valid])
        .expect("rejected match did not claim the root binding");
    unknown_type_builder
        .finalize_rows(vec![person])
        .expect("unknown-type builder remained usable");

    let mut function_builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = function_builder.binding("person").expect("person");
    let assigned = function_builder.binding("assigned").expect("assigned");
    let person_isa = function_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("person isa");
    let invalid_call = function_builder
        .function_call(
            &assigned,
            QueryFunctionTarget::Schema(
                FunctionId::new("schema_name_count").expect("schema function"),
            ),
            Vec::new(),
        )
        .expect("structurally valid zero-argument call");
    assert_eq!(
        function_builder
            .r#match(vec![person_isa.clone(), invalid_call])
            .expect_err("schema function arity must fail before scope mutation")
            .code()
            .as_str(),
        "query_plan_function_arity_mismatch",
    );
    let person_operand = function_builder
        .binding_operand(&person)
        .expect("person operand");
    let valid_call = function_builder
        .function_call(
            &assigned,
            QueryFunctionTarget::Schema(
                FunctionId::new("schema_name_count").expect("schema function"),
            ),
            vec![person_operand],
        )
        .expect("valid schema call");
    function_builder
        .r#match(vec![person_isa, valid_call])
        .expect("rejected function call did not claim either root binding");
    function_builder
        .finalize_rows(vec![person, assigned])
        .expect("function builder remained usable");

    let mut local_builder = QueryPlanBuilder::new(authority);
    let local_person = local_builder.binding("local_person").expect("local person");
    let invalid_body = local_builder
        .isa(
            &local_person,
            type_id(TypeKind::Entity, "missing_person"),
            false,
        )
        .expect("structurally valid invalid local body");
    let local_return = local_builder
        .local_return(Reducer::Count, &local_person, ValueTypeTag::Long)
        .expect("local return");
    let function_name = FunctionId::new("local_person_count").expect("function name");
    assert_eq!(
        local_builder
            .local_function(
                function_name.clone(),
                vec![local_person.clone()],
                vec![(
                    local_person.clone(),
                    Label::new("person").expect("parameter type"),
                )],
                vec![invalid_body],
                &local_return,
            )
            .expect_err("invalid local body must fail before claiming its private scope")
            .code()
            .as_str(),
        "query_plan_unknown_type",
    );
    let valid_body = local_builder
        .isa(&local_person, type_id(TypeKind::Entity, "person"), false)
        .expect("valid local body");
    local_builder
        .local_function(
            function_name,
            vec![local_person.clone()],
            vec![(local_person, Label::new("person").expect("parameter type"))],
            vec![valid_body],
            &local_return,
        )
        .expect("rejected local function did not claim bindings or reserve its name");
}

#[test]
fn schema_invalid_later_stages_reject_before_append_and_remain_repairable() {
    use type_bridge_contract::query_plan::Reducer;

    let authority = schema_authority();

    let mut sort_builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = sort_builder.binding("person").expect("person");
    let name = sort_builder.binding("name").expect("name");
    let person_isa = sort_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("person isa");
    let name_has = sort_builder
        .has(&person, &name, attribute("name"))
        .expect("name has");
    sort_builder
        .r#match(vec![person_isa, name_has])
        .expect("match");
    let invalid_order = sort_builder
        .order(&person, OrderDirection::Ascending)
        .expect("entity order handle");
    assert_eq!(
        sort_builder
            .sort(vec![invalid_order])
            .expect_err("entity bindings are not scalar sort keys")
            .code()
            .as_str(),
        "query_plan_sort_not_scalar",
    );
    let valid_order = sort_builder
        .order(&name, OrderDirection::Ascending)
        .expect("unique scalar order");
    sort_builder
        .sort(vec![valid_order])
        .expect("rejected sort did not append");
    sort_builder
        .finalize_rows(vec![person, name])
        .expect("sort builder remained usable");

    let mut reduce_builder = QueryPlanBuilder::new(Arc::clone(&authority));
    let person = reduce_builder.binding("person").expect("person");
    let result = reduce_builder.binding("result").expect("result");
    let person_isa = reduce_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("person isa");
    reduce_builder.r#match(vec![person_isa]).expect("match");
    let invalid_reduce = reduce_builder
        .reduce_assignment(&result, Reducer::Sum, Some(&person))
        .expect("structurally valid entity reduction");
    assert_eq!(
        reduce_builder
            .reduce(vec![invalid_reduce], vec![person.clone()])
            .expect_err("sum requires a numeric scalar input")
            .code()
            .as_str(),
        "query_plan_reduce_input_domain",
    );
    let valid_reduce = reduce_builder
        .reduce_assignment(&result, Reducer::Count, None)
        .expect("valid count assignment");
    reduce_builder
        .reduce(vec![valid_reduce], Vec::new())
        .expect("rejected reduce did not claim the result or append");
    reduce_builder
        .finalize_rows(vec![result])
        .expect("reduce builder remained usable");

    let mut window_builder = QueryPlanBuilder::new(authority);
    let person = window_builder.binding("person").expect("person");
    let age = window_builder.binding("age").expect("age");
    let person_isa = window_builder
        .isa(&person, type_id(TypeKind::Entity, "person"), false)
        .expect("person isa");
    let age_has = window_builder
        .has(&person, &age, attribute("age"))
        .expect("age has");
    window_builder
        .r#match(vec![person_isa, age_has])
        .expect("match");
    let age_order = window_builder
        .order(&age, OrderDirection::Ascending)
        .expect("age order");
    window_builder
        .sort(vec![age_order])
        .expect("a non-total scalar sort is valid without a window");
    assert_eq!(
        window_builder
            .limit(1)
            .expect_err("a window requires a validator-proven total order")
            .code()
            .as_str(),
        "query_plan_window_order_not_total",
    );
    window_builder
        .finalize_rows(vec![person, age])
        .expect("rejected limit did not append");
}

#[test]
fn authored_binding_ceiling_rejects_before_mutation_and_remains_repairable() {
    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let mut first = None;
    for index in 0..MAX_BINDINGS {
        let binding = builder
            .binding(format!("binding_{index}"))
            .expect("binding through exact ceiling");
        first.get_or_insert(binding);
    }
    assert_eq!(
        builder
            .binding("one_too_many")
            .expect_err("257th authored binding rejected before mutation")
            .code()
            .as_str(),
        "query_builder_authored_binding_limit"
    );
    let first = first.expect("first binding");
    let pattern = builder
        .isa(&first, type_id(TypeKind::Entity, "person"), false)
        .expect("first binding remains usable");
    builder
        .r#match(vec![pattern])
        .expect("failed declaration did not corrupt scope");
    builder
        .finalize_rows(vec![first])
        .expect("unscoped declarations do not enter the finalized plan");
}

#[test]
fn aggregate_local_and_root_pattern_ceiling_is_atomic() {
    use type_bridge_contract::query_plan::Reducer;

    let authority = schema_authority();
    let mut builder = QueryPlanBuilder::new(authority);
    let full_functions = (MAX_PREDICATE_NODES - 1) / MAX_BOOLEAN_TERMS;
    let remainder = (MAX_PREDICATE_NODES - 1) % MAX_BOOLEAN_TERMS;

    for function_index in 0..full_functions {
        let local = builder
            .binding(format!("local_{function_index}"))
            .expect("local binding");
        let pattern = builder
            .isa(&local, type_id(TypeKind::Entity, "person"), false)
            .expect("local pattern");
        let returns = builder
            .local_return(Reducer::Count, &local, ValueTypeTag::Long)
            .expect("local return");
        builder
            .local_function(
                FunctionId::new(format!("local_{function_index}_count")).expect("function id"),
                vec![local.clone()],
                vec![(local, Label::new("person").expect("parameter"))],
                vec![pattern; MAX_BOOLEAN_TERMS],
                &returns,
            )
            .expect("aggregate nodes through full chunk");
    }

    let remainder_binding = builder.binding("remainder").expect("remainder binding");
    let remainder_pattern = builder
        .isa(
            &remainder_binding,
            type_id(TypeKind::Entity, "person"),
            false,
        )
        .expect("remainder pattern");
    let remainder_return = builder
        .local_return(Reducer::Count, &remainder_binding, ValueTypeTag::Long)
        .expect("remainder return");
    builder
        .local_function(
            FunctionId::new("remainder_count").expect("function id"),
            vec![remainder_binding.clone()],
            vec![(remainder_binding, Label::new("person").expect("parameter"))],
            vec![remainder_pattern; remainder],
            &remainder_return,
        )
        .expect("aggregate local nodes stop one below ceiling");

    let rejected = builder.binding("rejected").expect("rejected binding");
    let rejected_pattern = builder
        .isa(&rejected, type_id(TypeKind::Entity, "person"), false)
        .expect("rejected pattern");
    let rejected_return = builder
        .local_return(Reducer::Count, &rejected, ValueTypeTag::Long)
        .expect("rejected return");
    assert_eq!(
        builder
            .local_function(
                FunctionId::new("too_many_nodes").expect("function id"),
                vec![rejected.clone()],
                vec![(rejected, Label::new("person").expect("parameter"))],
                vec![rejected_pattern; 2],
                &rejected_return,
            )
            .expect_err("aggregate node overflow rejected before scope claim")
            .code()
            .as_str(),
        "query_plan_pattern_node_limit"
    );

    let root = builder.binding("root").expect("root binding");
    let root_pattern = builder
        .isa(&root, type_id(TypeKind::Entity, "person"), false)
        .expect("root pattern");
    builder
        .r#match(vec![root_pattern])
        .expect("failed local append left one root node available");
    builder
        .finalize_rows(vec![root])
        .expect("exact aggregate ceiling remains valid");
}

#[test]
fn reducer_spellings_cover_the_complete_canonical_vocabulary() {
    use type_bridge_contract::query_plan::Reducer;
    use type_bridge_orm::query_v2_builder::query_builder_reducer;
    for (spelling, expected) in [
        ("count", Reducer::Count),
        ("max", Reducer::Max),
        ("mean", Reducer::Mean),
        ("median", Reducer::Median),
        ("min", Reducer::Min),
        ("std", Reducer::Std),
        ("sum", Reducer::Sum),
    ] {
        assert_eq!(
            query_builder_reducer(spelling).expect("canonical reducer spelling"),
            expected,
        );
    }
    assert!(query_builder_reducer("variance").is_err());
}
