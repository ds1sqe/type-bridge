use std::collections::BTreeSet;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{
    AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind,
};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocText, DocumentId, FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement,
    FunctionReturnMode, FunctionSignature, ManagedFactSelection, ManagedSchemaState,
    ManagedScopeId, OwnsFact, OwnsFactId, PatchFormatVersion, PlaysFact, PlaysFactId, RegexPattern,
    RelatesFact, RelatesFactId, SchemaAnnotationValue, SchemaDelta, SchemaDiagnostics, SchemaFact,
    SchemaFactId, SchemaOperation, SchemaOperationKind, SourceSpan, SourcedSchemaFact, StructFact,
    StructField, SubFact, SubFactId, TypeFact, TypeReference, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{Cardinality, ValueTypeTag};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, DeltaError, DeltaSafety, FactDependencyGraph,
    ManagedDeltaContext, apply_delta, classify_delta_safety, classify_schema_operation_safety,
    diff_managed, inverse_delta, managed_schema_state, plan_schema_operations,
};

fn capabilities(ids: &[&str]) -> CapabilitySet {
    ids.iter()
        .map(|id| CapabilityId::new(*id).expect("test capability"))
        .collect()
}

fn builtin_capabilities() -> CapabilitySet {
    capabilities(BUILTIN_SCHEMA_CAPABILITY_IDS)
}

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("delta-test").expect("test scope"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("test profile"),
        builtin_capabilities(),
    )
}

fn declared(facts: Vec<SchemaFact>) -> DeclaredSchema {
    declared_with_capabilities(facts, builtin_capabilities())
}

fn declared_with_capabilities(
    facts: Vec<SchemaFact>,
    required_capabilities: CapabilitySet,
) -> DeclaredSchema {
    try_declared_with_capabilities(facts, required_capabilities).expect("test declaration")
}

fn try_declared(facts: Vec<SchemaFact>) -> Result<DeclaredSchema, SchemaDiagnostics> {
    try_declared_with_capabilities(facts, builtin_capabilities())
}

fn try_declared_with_capabilities(
    facts: Vec<SchemaFact>,
    required_capabilities: CapabilitySet,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let line = u32::try_from(index + 1).expect("small fixture");
        let start = u64::try_from(index).expect("small fixture");
        let span = SourceSpan::new(
            DocumentId::new("delta-test.yaml").expect("test document"),
            start,
            start + 1,
            line,
            1,
            line,
            2,
        )
        .expect("test span");
        SourcedSchemaFact::new(fact, span)
    });
    DeclaredSchema::from_facts(FormatVersion::V1, required_capabilities, sourced)
}

fn type_id(kind: TypeKind, label: &str) -> TypeId {
    TypeId::new(kind, label).expect("test type")
}

fn type_fact(kind: TypeKind, label: &str) -> SchemaFact {
    SchemaFact::Type(TypeFact::new(type_id(kind, label)).expect("test type fact"))
}

fn value_fact(label: &str, value_type: ValueTypeTag) -> SchemaFact {
    SchemaFact::Value(ValueFact::new(
        ValueFactId::new(AttributeId::new(label).expect("test attribute")),
        value_type,
    ))
}

fn owns_id(owner: &str, attribute: &str) -> OwnsFactId {
    OwnsFactId::new(
        type_id(TypeKind::Entity, owner),
        AttributeId::new(attribute).expect("test attribute"),
    )
    .expect("test owns identity")
}

fn role_id(relation: &str, role: &str) -> RoleId {
    RoleId::new(relation, role).expect("test role")
}

fn relates_id(relation: &str, role: &str) -> RelatesFactId {
    RelatesFactId::new(
        type_id(TypeKind::Relation, relation),
        role_id(relation, role),
    )
    .expect("test relates identity")
}

fn plays_id(player: &str, relation: &str, role: &str) -> PlaysFactId {
    PlaysFactId::new(type_id(TypeKind::Entity, player), role_id(relation, role))
        .expect("test plays identity")
}

fn card_annotation(subject: OwnsFactId) -> SchemaFact {
    card_annotation_with_bounds(subject, 0, Some(1))
}

fn card_annotation_with_bounds(subject: OwnsFactId, min: u64, max: Option<u64>) -> SchemaFact {
    SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(AnnotationSubjectId::Owns(subject), AnnotationKindId::Card),
            SchemaAnnotationValue::Cardinality(
                Cardinality::new(min, max).expect("test cardinality"),
            ),
        )
        .expect("test annotation"),
    )
}

fn person_function() -> SchemaFact {
    person_function_with_body("return $input;")
}

fn person_function_with_body(body: &str) -> SchemaFact {
    SchemaFact::Function(FunctionFact::new(
        FunctionId::new("identity_person").expect("test function"),
        FunctionSignature::new(
            vec![FunctionParameter::new(
                Label::new("input").expect("test parameter"),
                TypeReference::Schema(Label::new("person").expect("test reference")),
            )],
            FunctionReturnMode::scalar(FunctionReturnElement::new(
                TypeReference::Schema(Label::new("person").expect("test reference")),
                false,
            )),
        )
        .expect("test signature"),
        FunctionBody::new(body).expect("test function body"),
    ))
}

fn struct_fact() -> SchemaFact {
    SchemaFact::Struct(
        StructFact::new(
            StructId::new("record").expect("test struct"),
            vec![StructField::new(
                Label::new("value").expect("test field"),
                ValueTypeTag::String,
                false,
            )],
        )
        .expect("test struct fact"),
    )
}

fn owns_fixture(include_annotation: bool) -> Vec<SchemaFact> {
    let owns = owns_id("person", "name");
    let mut facts = vec![
        type_fact(TypeKind::Entity, "person"),
        type_fact(TypeKind::Attribute, "name"),
        value_fact("name", ValueTypeTag::String),
        SchemaFact::Owns(OwnsFact::new(owns.clone())),
    ];
    if include_annotation {
        facts.push(card_annotation(owns));
    }
    facts
}

struct VariantCase {
    name: &'static str,
    source: DeclaredSchema,
    target: DeclaredSchema,
    touched: SchemaFactId,
    dependencies: BTreeSet<SchemaFactId>,
    opaque: bool,
}

fn variant_cases() -> Vec<VariantCase> {
    let anchor = type_fact(TypeKind::Entity, "anchor");
    let person = type_fact(TypeKind::Entity, "person");
    let employee = type_fact(TypeKind::Entity, "employee");
    let person_id = type_id(TypeKind::Entity, "person");
    let employee_id = type_id(TypeKind::Entity, "employee");

    let new_type = type_fact(TypeKind::Entity, "company");
    let sub = SchemaFact::Sub(SubFact::new(
        SubFactId::new(employee_id.clone(), person_id.clone()).expect("test sub identity"),
    ));
    let attribute_type = type_fact(TypeKind::Attribute, "name");
    let value = value_fact("name", ValueTypeTag::String);
    let owns = SchemaFact::Owns(OwnsFact::new(owns_id("person", "name")));
    let relation_type = type_fact(TypeKind::Relation, "friendship");
    let relates = SchemaFact::Relates(
        RelatesFact::new(relates_id("friendship", "friend"), None).expect("test relates"),
    );
    let plays = SchemaFact::Plays(PlaysFact::new(plays_id("person", "friendship", "friend")));
    let annotation = card_annotation(owns_id("person", "name"));
    let function = person_function();
    let structure = struct_fact();

    vec![
        VariantCase {
            name: "type",
            source: declared(vec![anchor.clone()]),
            target: declared(vec![anchor.clone(), new_type.clone()]),
            touched: new_type.id(),
            dependencies: BTreeSet::new(),
            opaque: false,
        },
        VariantCase {
            name: "sub",
            source: declared(vec![person.clone(), employee.clone()]),
            target: declared(vec![person.clone(), employee.clone(), sub.clone()]),
            touched: sub.id(),
            dependencies: BTreeSet::from([
                SchemaFactId::Type(person_id.clone()),
                SchemaFactId::Type(employee_id.clone()),
            ]),
            opaque: false,
        },
        VariantCase {
            name: "value",
            source: declared(vec![attribute_type.clone()]),
            target: declared(vec![attribute_type.clone(), value.clone()]),
            touched: value.id(),
            dependencies: BTreeSet::from([attribute_type.id()]),
            opaque: false,
        },
        VariantCase {
            name: "owns",
            source: declared(vec![person.clone(), attribute_type.clone(), value.clone()]),
            target: declared(vec![
                person.clone(),
                attribute_type.clone(),
                value.clone(),
                owns.clone(),
            ]),
            touched: owns.id(),
            dependencies: BTreeSet::from([person.id(), attribute_type.id(), value.id()]),
            opaque: false,
        },
        VariantCase {
            name: "relates",
            source: declared(vec![relation_type.clone()]),
            target: declared(vec![relation_type.clone(), relates.clone()]),
            touched: relates.id(),
            dependencies: BTreeSet::from([relation_type.id()]),
            opaque: false,
        },
        VariantCase {
            name: "plays",
            source: declared(vec![person.clone(), relation_type.clone(), relates.clone()]),
            target: declared(vec![
                person.clone(),
                relation_type.clone(),
                relates.clone(),
                plays.clone(),
            ]),
            touched: plays.id(),
            dependencies: BTreeSet::from([person.id(), relates.id()]),
            opaque: false,
        },
        VariantCase {
            name: "annotation",
            source: declared(vec![
                person.clone(),
                attribute_type.clone(),
                value.clone(),
                owns.clone(),
            ]),
            target: declared(vec![
                person.clone(),
                attribute_type.clone(),
                value.clone(),
                owns.clone(),
                annotation.clone(),
            ]),
            touched: annotation.id(),
            dependencies: BTreeSet::from([owns.id()]),
            opaque: false,
        },
        VariantCase {
            name: "function",
            source: declared(vec![person.clone()]),
            target: declared(vec![person.clone(), function.clone()]),
            touched: function.id(),
            dependencies: BTreeSet::from([person.id()]),
            opaque: true,
        },
        VariantCase {
            name: "struct",
            source: declared(vec![anchor.clone()]),
            target: declared(vec![anchor, structure.clone()]),
            touched: structure.id(),
            dependencies: BTreeSet::new(),
            opaque: false,
        },
    ]
}

fn operation_index(delta: &SchemaDelta, id: &SchemaFactId) -> usize {
    delta
        .operations()
        .iter()
        .position(|operation| operation.affected_ids().contains(id))
        .expect("operation affects fixture fact")
}

#[test]
fn all_fact_variants_have_exact_dependencies_and_closed_migration_behavior() {
    let context = context();
    for case in variant_cases() {
        let graph = FactDependencyGraph::from_declared(&case.target).expect(case.name);
        graph.validate_complete().expect(case.name);
        assert_eq!(
            graph.dependencies(&case.touched),
            Some(&case.dependencies),
            "{} dependencies",
            case.name
        );

        if case.opaque {
            assert!(
                diff_managed(&case.source, &case.target, &context).is_err(),
                "{} diff must fail closed",
                case.name
            );
            let operation = SchemaOperation::define(vec![
                case.target
                    .fact(&case.touched)
                    .expect("target fact")
                    .clone(),
            ])
            .expect("manual function operation");
            let delta = SchemaDelta::new(
                PatchFormatVersion::V1,
                managed_schema_state(&case.source, &context).expect("source state"),
                managed_schema_state(&case.target, &context).expect("target state"),
                vec![operation],
            )
            .expect("manual opaque delta");
            assert_eq!(
                classify_delta_safety(&delta).classification(),
                DeltaSafety::Additive
            );
            assert!(matches!(
                apply_delta(&case.source, &delta, &context),
                Err(DeltaError::Contract(_))
            ));
        } else {
            let delta = diff_managed(&case.source, &case.target, &context).expect(case.name);
            assert!(
                delta
                    .operations()
                    .iter()
                    .flat_map(SchemaOperation::affected_ids)
                    .any(|id| id == case.touched),
                "{} formal diff",
                case.name
            );
            let applied = apply_delta(&case.source, &delta, &context).expect(case.name);
            assert_eq!(
                applied
                    .canonical_identity_bytes()
                    .expect("applied identity"),
                case.target
                    .canonical_identity_bytes()
                    .expect("target identity"),
                "{} replay",
                case.name
            );
        }
    }
}

#[test]
fn explicit_default_is_a_formal_change_but_semantically_equal() {
    let context = context();
    let omitted = declared(owns_fixture(false));
    let explicit = declared(owns_fixture(true));
    let omitted_state = managed_schema_state(&omitted, &context).expect("omitted state");
    let explicit_state = managed_schema_state(&explicit, &context).expect("explicit state");

    assert_ne!(
        omitted_state.managed_declared_identity(),
        explicit_state.managed_declared_identity()
    );
    assert_eq!(
        omitted_state.managed_semantic_schema(),
        explicit_state.managed_semantic_schema()
    );
    let delta = diff_managed(&omitted, &explicit, &context).expect("formal default diff");
    assert_eq!(delta.operations().len(), 1);
    assert_eq!(delta.operations()[0].kind(), SchemaOperationKind::Define);
    assert_eq!(
        classify_delta_safety(&delta).classification(),
        DeltaSafety::FormalOnly
    );
    let applied = apply_delta(&omitted, &delta, &context).expect("explicit default replay");
    assert_eq!(
        applied
            .canonical_identity_bytes()
            .expect("applied identity"),
        explicit
            .canonical_identity_bytes()
            .expect("explicit identity")
    );
}

#[test]
fn capability_only_transition_applies_and_inverts() {
    let facts = vec![type_fact(TypeKind::Entity, "person")];
    let source = declared_with_capabilities(facts.clone(), CapabilitySet::new());
    let target = declared_with_capabilities(facts, capabilities(&["schema.annotations"]));
    let context = context();

    let delta = diff_managed(&source, &target, &context).expect("capability transition");
    assert!(delta.operations().is_empty());
    assert_eq!(
        classify_delta_safety(&delta).classification(),
        DeltaSafety::FormalOnly
    );
    let applied = apply_delta(&source, &delta, &context).expect("capability replay");
    assert_eq!(
        applied.required_capabilities(),
        target.required_capabilities()
    );

    let inverse = inverse_delta(&delta).expect("capability inverse");
    assert!(inverse.operations().is_empty());
    let restored = apply_delta(&applied, &inverse, &context).expect("inverse replay");
    assert_eq!(
        restored
            .canonical_identity_bytes()
            .expect("restored identity"),
        source.canonical_identity_bytes().expect("source identity")
    );
    assert_eq!(
        restored.required_capabilities(),
        source.required_capabilities()
    );
}

#[test]
fn source_target_and_expected_fact_tampering_fail_closed() {
    let context = context();
    let attribute = type_fact(TypeKind::Attribute, "name");
    let source_value = value_fact("name", ValueTypeTag::String);
    let target_value = value_fact("name", ValueTypeTag::Long);
    let wrong_value = value_fact("name", ValueTypeTag::Double);
    let source = declared(vec![attribute.clone(), source_value.clone()]);
    let target = declared(vec![attribute.clone(), target_value.clone()]);
    let wrong = declared(vec![attribute, wrong_value.clone()]);
    let exact = diff_managed(&source, &target, &context).expect("exact redefine");

    let source_fingerprint_tamper = SchemaDelta::new(
        PatchFormatVersion::V1,
        managed_schema_state(&wrong, &context).expect("wrong source state"),
        managed_schema_state(&target, &context).expect("target state"),
        exact.operations().to_vec(),
    )
    .expect("contract permits opaque fingerprint payload");
    assert!(matches!(
        apply_delta(&source, &source_fingerprint_tamper, &context),
        Err(DeltaError::Contract(_))
    ));

    let target_fingerprint_tamper = SchemaDelta::new(
        PatchFormatVersion::V1,
        managed_schema_state(&source, &context).expect("source state"),
        managed_schema_state(&wrong, &context).expect("wrong target state"),
        exact.operations().to_vec(),
    )
    .expect("contract permits opaque fingerprint payload");
    assert!(matches!(
        apply_delta(&source, &target_fingerprint_tamper, &context),
        Err(DeltaError::Contract(_))
    ));

    let expected_fact_tamper = SchemaDelta::new(
        PatchFormatVersion::V1,
        managed_schema_state(&source, &context).expect("source state"),
        managed_schema_state(&target, &context).expect("target state"),
        vec![
            SchemaOperation::redefine(wrong_value, target_value)
                .expect("same identity tampered redefine"),
        ],
    )
    .expect("contract permits opaque expected payload");
    assert!(matches!(
        apply_delta(&source, &expected_fact_tamper, &context),
        Err(DeltaError::Contract(_))
    ));
}

#[test]
fn undefines_reverse_owns_value_and_plays_relates_dependencies() {
    let context = context();
    let owns_source = declared(owns_fixture(false));
    let owns_target = declared(vec![type_fact(TypeKind::Entity, "person")]);
    let owns_delta = diff_managed(&owns_source, &owns_target, &context).expect("owns removal");
    let owns = SchemaFactId::Owns(owns_id("person", "name"));
    let value = SchemaFactId::Value(ValueFactId::new(
        AttributeId::new("name").expect("test attribute"),
    ));
    assert!(operation_index(&owns_delta, &owns) < operation_index(&owns_delta, &value));
    apply_delta(&owns_source, &owns_delta, &context).expect("owns removal replay");

    let relates = SchemaFact::Relates(
        RelatesFact::new(relates_id("friendship", "friend"), None).expect("test relates"),
    );
    let plays = SchemaFact::Plays(PlaysFact::new(plays_id("person", "friendship", "friend")));
    let plays_source = declared(vec![
        type_fact(TypeKind::Entity, "person"),
        type_fact(TypeKind::Relation, "friendship"),
        relates.clone(),
        plays.clone(),
    ]);
    let plays_target = declared(vec![
        type_fact(TypeKind::Entity, "person"),
        type_fact(TypeKind::Relation, "friendship"),
    ]);
    let plays_delta = diff_managed(&plays_source, &plays_target, &context).expect("plays removal");
    assert!(
        operation_index(&plays_delta, &plays.id()) < operation_index(&plays_delta, &relates.id())
    );
    apply_delta(&plays_source, &plays_delta, &context).expect("plays removal replay");
}

#[test]
fn specialized_role_orders_before_parent_and_requires_sub_path() {
    let context = context();
    let parent_relation = type_id(TypeKind::Relation, "parenthood");
    let child_relation = type_id(TypeKind::Relation, "fatherhood");
    let sub = SchemaFact::Sub(SubFact::new(
        SubFactId::new(child_relation.clone(), parent_relation.clone()).expect("relation sub"),
    ));
    let parent_role = role_id("parenthood", "parent");
    let parent = SchemaFact::Relates(
        RelatesFact::new(
            RelatesFactId::new(parent_relation, parent_role.clone()).expect("parent relates id"),
            None,
        )
        .expect("parent relates"),
    );
    let child = SchemaFact::Relates(
        RelatesFact::new(
            RelatesFactId::new(child_relation, role_id("fatherhood", "father"))
                .expect("child relates id"),
            Some(parent_role),
        )
        .expect("specialized relates"),
    );
    let structural = vec![
        type_fact(TypeKind::Relation, "parenthood"),
        type_fact(TypeKind::Relation, "fatherhood"),
        sub.clone(),
    ];
    let mut source_facts = structural.clone();
    source_facts.extend([parent.clone(), child.clone()]);
    let source = declared(source_facts);
    let target = declared(structural);

    let graph = FactDependencyGraph::from_declared(&source).expect("specialization graph");
    let dependencies = graph.dependencies(&child.id()).expect("child dependencies");
    assert!(dependencies.contains(&parent.id()));
    assert!(dependencies.contains(&sub.id()));
    let delta = diff_managed(&source, &target, &context).expect("role removal");
    assert!(operation_index(&delta, &child.id()) < operation_index(&delta, &parent.id()));
    apply_delta(&source, &delta, &context).expect("role removal replay");
}

#[test]
fn survivor_dependencies_and_function_signature_removal_are_rejected() {
    let owns_source = declared(owns_fixture(false));
    let owns_without_value = declared(vec![
        type_fact(TypeKind::Entity, "person"),
        type_fact(TypeKind::Attribute, "name"),
        SchemaFact::Owns(OwnsFact::new(owns_id("person", "name"))),
    ]);
    assert!(plan_schema_operations(&owns_source, &owns_without_value).is_err());

    let context = context();
    let source_state = managed_schema_state(&owns_source, &context).expect("source state");
    let value_id = SchemaFactId::Value(ValueFactId::new(
        AttributeId::new("name").expect("test attribute"),
    ));
    let target_selection = ManagedFactSelection::new(
        source_state
            .selection()
            .iter()
            .filter(|id| *id != &value_id)
            .cloned(),
    )
    .expect("tampered target selection");
    let target_state = ManagedSchemaState::new(
        source_state.format(),
        source_state.required_capabilities().clone(),
        source_state.scope().clone(),
        target_selection,
        source_state.declared_identity().clone(),
        source_state.managed_declared_identity().clone(),
        source_state.managed_semantic_schema().clone(),
    )
    .expect("contract-valid opaque target state");
    let survivor_delta = SchemaDelta::new(
        PatchFormatVersion::V1,
        source_state,
        target_state,
        vec![SchemaOperation::undefine(
            owns_source
                .fact(&value_id)
                .expect("source value fact")
                .clone(),
        )],
    )
    .expect("contract-valid survivor delta");
    assert!(matches!(
        apply_delta(&owns_source, &survivor_delta, &context),
        Err(DeltaError::Contract(_))
    ));

    let function = person_function();
    let function_source = declared(vec![
        type_fact(TypeKind::Entity, "person"),
        function.clone(),
    ]);
    let function_without_type =
        try_declared(vec![function]).expect_err("invalid function authoring target");
    assert_eq!(
        function_without_type
            .iter()
            .next()
            .expect("diagnostic")
            .diagnostic()
            .code()
            .as_str(),
        "unknown_schema_fact_reference"
    );
    assert!(plan_schema_operations(&function_source, &function_source).is_ok());
}

#[test]
fn inverse_roundtrip_and_safety_classes_are_deterministic() {
    let context = context();
    let source = declared(vec![type_fact(TypeKind::Entity, "person")]);
    let target = declared(vec![
        type_fact(TypeKind::Entity, "company"),
        type_fact(TypeKind::Entity, "person"),
    ]);
    let delta = diff_managed(&source, &target, &context).expect("additive diff");
    assert_eq!(
        classify_delta_safety(&delta).classification(),
        DeltaSafety::Additive
    );
    let applied = apply_delta(&source, &delta, &context).expect("apply");
    let inverse = inverse_delta(&delta).expect("inverse");
    assert_eq!(
        classify_delta_safety(&inverse).classification(),
        DeltaSafety::Destructive
    );
    let restored = apply_delta(&applied, &inverse, &context).expect("inverse apply");
    assert_eq!(
        restored
            .canonical_identity_bytes()
            .expect("restored identity"),
        source.canonical_identity_bytes().expect("source identity")
    );
}

#[test]
fn safety_lattice_matches_the_live_lowering_registry() {
    let owns = owns_id("person", "name");
    let explicit_default = card_annotation(owns.clone());
    let widened_card = card_annotation_with_bounds(owns.clone(), 0, Some(2));
    let required_card = card_annotation_with_bounds(owns.clone(), 1, Some(1));
    let doc = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(type_id(TypeKind::Entity, "person")),
                AnnotationKindId::Doc,
            ),
            SchemaAnnotationValue::Doc(DocText::new("docs").expect("test docs")),
        )
        .expect("test doc annotation"),
    );
    let key = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(owns.clone()),
                AnnotationKindId::Key,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("test key annotation"),
    );
    let unique = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Owns(owns.clone()),
                AnnotationKindId::Unique,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("test unique annotation"),
    );
    let regex = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Value(ValueFactId::new(
                    AttributeId::new("name").expect("test attribute"),
                )),
                AnnotationKindId::Regex,
            ),
            SchemaAnnotationValue::Regex(RegexPattern::new("^a+$").expect("test regex")),
        )
        .expect("test regex annotation"),
    );
    let independent = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(type_id(TypeKind::Attribute, "name")),
                AnnotationKindId::Independent,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("test independent annotation"),
    );
    let sub = SchemaFact::Sub(SubFact::new(
        SubFactId::new(
            type_id(TypeKind::Entity, "employee"),
            type_id(TypeKind::Entity, "person"),
        )
        .expect("test sub"),
    ));
    let relates = relates_id("friendship", "child");
    let unspecialized =
        SchemaFact::Relates(RelatesFact::new(relates.clone(), None).expect("test relates"));
    let specialized = SchemaFact::Relates(
        RelatesFact::new(relates, Some(role_id("friendship", "parent")))
            .expect("test specialization"),
    );
    let string_value = value_fact("name", ValueTypeTag::String);
    let long_value = value_fact("name", ValueTypeTag::Long);
    let function = person_function();
    let changed_function = person_function_with_body("return first $input;");
    let persistent_function_doc = SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Function(
                    FunctionId::new("identity_person").expect("test function"),
                ),
                AnnotationKindId::Doc,
            ),
            SchemaAnnotationValue::Doc(DocText::new("function docs").expect("test docs")),
        )
        .expect("test function doc"),
    );

    let cases = vec![
        (
            "explicit-default-add",
            SchemaOperation::define(vec![explicit_default.clone()]).expect("operation"),
            DeltaSafety::FormalOnly,
        ),
        (
            "explicit-default-remove",
            SchemaOperation::undefine(explicit_default.clone()),
            DeltaSafety::FormalOnly,
        ),
        (
            "doc",
            SchemaOperation::define(vec![doc]).expect("operation"),
            DeltaSafety::SchemaMetadata,
        ),
        (
            "optional-interface",
            SchemaOperation::define(vec![SchemaFact::Owns(OwnsFact::new(owns.clone()))])
                .expect("operation"),
            DeltaSafety::Additive,
        ),
        (
            "cardinality-widen",
            SchemaOperation::redefine(explicit_default.clone(), widened_card).expect("operation"),
            DeltaSafety::Additive,
        ),
        (
            "constraint-remove",
            SchemaOperation::undefine(regex.clone()),
            DeltaSafety::Additive,
        ),
        (
            "sub",
            SchemaOperation::define(vec![sub]).expect("operation"),
            DeltaSafety::Conditional,
        ),
        (
            "specialization",
            SchemaOperation::redefine(unspecialized, specialized).expect("operation"),
            DeltaSafety::Conditional,
        ),
        (
            "constraint-add",
            SchemaOperation::define(vec![regex]).expect("operation"),
            DeltaSafety::Conditional,
        ),
        (
            "key",
            SchemaOperation::define(vec![key]).expect("operation"),
            DeltaSafety::BackfillRequired,
        ),
        (
            "unique",
            SchemaOperation::define(vec![unique]).expect("operation"),
            DeltaSafety::BackfillRequired,
        ),
        (
            "cardinality-narrow",
            SchemaOperation::redefine(explicit_default, required_card).expect("operation"),
            DeltaSafety::BackfillRequired,
        ),
        (
            "type-remove",
            SchemaOperation::undefine(type_fact(TypeKind::Entity, "person")),
            DeltaSafety::Destructive,
        ),
        (
            "independent-remove",
            SchemaOperation::undefine(independent),
            DeltaSafety::Destructive,
        ),
        (
            "value-type-change",
            SchemaOperation::redefine(string_value, long_value).expect("operation"),
            DeltaSafety::Destructive,
        ),
        (
            "function-remove",
            SchemaOperation::undefine(function.clone()),
            DeltaSafety::Destructive,
        ),
        (
            "function-redefine",
            SchemaOperation::redefine(function, changed_function).expect("operation"),
            DeltaSafety::Opaque,
        ),
        (
            "struct",
            SchemaOperation::define(vec![struct_fact()]).expect("operation"),
            DeltaSafety::Unsupported,
        ),
        (
            "persistent-function-doc",
            SchemaOperation::define(vec![persistent_function_doc]).expect("operation"),
            DeltaSafety::Unsupported,
        ),
    ];

    for (name, operation, expected) in cases {
        assert_eq!(
            classify_schema_operation_safety(&operation),
            expected,
            "{name}"
        );
    }
}
