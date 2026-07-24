use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, Label, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration_assertion::{AssertionBinding, BindingId, QueryVariable};
use type_bridge_contract::query_plan::{
    QueryInvocation, QueryOperation, QueryOutput, QueryPattern, QueryPlan, ReadStage,
};
use type_bridge_contract::schema::DocumentId;
use type_bridge_orm::query_v2::lower_validated_query;
use type_bridge_query::{MigrationAssertionValidationContext, validate_query_plan};
use type_bridge_schema::{
    ManagedDeltaContext, SchemaDocumentSet, managed_schema_state, normalize_documents, resolve,
};

fn binding(id: u16, variable: &str) -> AssertionBinding {
    AssertionBinding::new(
        BindingId::new(id).expect("binding ID"),
        QueryVariable::new(variable).expect("query variable"),
    )
}

#[test]
fn contract_label_decisions_match_typeql_3_12() {
    for label in ["_", "type-with-hyphens", "a·b", "a\u{301}", "℘x"] {
        assert!(Label::new(label).is_ok(), "contract rejected {label:?}");
        assert!(
            typeql::parse_label(label).is_ok(),
            "TypeQL rejected {label:?}"
        );
    }
    for label in ["", "9person", "person name", "a²"] {
        assert!(Label::new(label).is_err(), "contract accepted {label:?}");
        assert!(
            typeql::parse_label(label).is_err(),
            "TypeQL accepted {label:?}"
        );
    }
    for label in [
        "with",
        "given",
        "match",
        "fetch",
        "update",
        "define",
        "undefine",
        "redefine",
        "insert",
        "put",
        "delete",
        "end",
        "entity",
        "relation",
        "attribute",
        "role",
        "asc",
        "desc",
        "struct",
        "fun",
        "return",
        "alias",
        "sub",
        "owns",
        "as",
        "plays",
        "relates",
        "iid",
        "isa",
        "links",
        "has",
        "is",
        "or",
        "not",
        "try",
        "in",
        "true",
        "false",
        "of",
        "from",
        "first",
        "last",
    ] {
        assert!(
            typeql::is_reserved_keyword(label),
            "TypeQL does not reserve the contract keyword {label:?}"
        );
        assert!(
            Label::new(label).is_err(),
            "contract accepted reserved TypeQL keyword {label:?}"
        );
    }
}

#[test]
fn contract_builtin_function_vocabulary_matches_typeql_3_12() {
    use type_bridge_contract::id::is_typeql_3_12_builtin_function_name;
    use typeql::expression::FunctionName;
    use typeql::query::{QueryStructure, stage::Stage};

    let parser_classifies_as_builtin = |name: &str| {
        let source = format!("match let $value = {name}(1);");
        let query = typeql::parse_query(&source).expect("function-call query parses");
        let QueryStructure::Pipeline(pipeline) = query.into_structure() else {
            panic!("function-call query must parse as a pipeline");
        };
        let [Stage::Match(match_stage)] = pipeline.stages.as_slice() else {
            panic!("function-call query must contain one match stage");
        };
        let [typeql::Pattern::Statement(typeql::Statement::Assignment(assignment))] =
            match_stage.patterns.as_slice()
        else {
            panic!("function-call query must contain one assignment");
        };
        let typeql::Expression::Function(call) = &assignment.rhs else {
            panic!("assignment must contain a function call");
        };
        matches!(call.name, FunctionName::Builtin(_))
    };

    for name in [
        "abs", "ceil", "floor", "iid", "label", "len", "max", "min", "round",
    ] {
        assert!(
            parser_classifies_as_builtin(name),
            "TypeQL did not parse {name:?} as a built-in call",
        );
        assert!(
            is_typeql_3_12_builtin_function_name(name),
            "contract omitted TypeQL built-in {name:?}",
        );
    }

    for name in ["absolute", "length", "person_name_length"] {
        assert!(
            !parser_classifies_as_builtin(name),
            "TypeQL unexpectedly parsed {name:?} as a built-in call",
        );
        assert!(!is_typeql_3_12_builtin_function_name(name));
    }
}

#[test]
fn yaml_unicode_labels_survive_validation_and_typeql_lowering() {
    let combining_label = "a\u{301}";
    let source = format!(
        r#"format: typebridge.schema/v2
attributes:
  a·b:
    value: string
  "{combining_label}":
    value: string
entities:
  ℘x:
    owns:
      - a·b
      - "{combining_label}"
"#,
    );
    let documents = SchemaDocumentSet::parse([(
        DocumentId::new("unicode-labels.yaml").expect("document ID"),
        source.as_str(),
    )])
    .expect("YAML parses");
    let declared = normalize_documents(&documents).expect("YAML labels normalize");
    let profile = SemanticProfileId::new("typedb-3.12.1/v1").expect("semantic profile");
    let delta_context = ManagedDeltaContext::new(
        ManagedScopeId::new("unicode-label-conformance").expect("managed scope"),
        profile.clone(),
        CapabilitySet::new(),
    );
    let managed = managed_schema_state(&declared, &delta_context).expect("managed state");
    let resolved = resolve(&declared, &profile).expect("resolved schema");

    let thing = BindingId::new(0).expect("binding ID");
    let middle_dot_value = BindingId::new(1).expect("binding ID");
    let combining_value = BindingId::new(2).expect("binding ID");
    let plan = QueryPlan::new(
        vec![
            binding(0, "thing"),
            binding(1, "middle_dot_value"),
            binding(2, "combining_value"),
        ],
        Vec::new(),
        vec![ReadStage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: thing,
                    include_subtypes: false,
                    type_id: TypeId::new(TypeKind::Entity, "℘x").expect("entity label"),
                },
                QueryPattern::Has {
                    attribute: middle_dot_value,
                    attribute_id: AttributeId::new("a·b").expect("middle-dot label"),
                    owner: thing,
                },
                QueryPattern::Has {
                    attribute: combining_value,
                    attribute_id: AttributeId::new(combining_label).expect("combining label"),
                    owner: thing,
                },
            ],
        }],
        QueryOutput::Rows {
            columns: vec![thing, middle_dot_value, combining_value],
        },
        managed.managed_semantic_schema().clone(),
    )
    .expect("query plan");
    let validation_context = MigrationAssertionValidationContext::new(&resolved, &managed);
    let validated = validate_query_plan(&plan, &validation_context, StructuralLimits::CANONICAL)
        .expect("Unicode-labelled query validates");
    let invocation =
        QueryInvocation::new(&plan, QueryOperation::Rows, Vec::new()).expect("query invocation");
    let lowered = lower_validated_query(&validated, &invocation).expect("query lowers");

    assert!(lowered.typeql().contains("$thing isa! ℘x;"));
    assert!(
        lowered
            .typeql()
            .contains("$thing has a·b $middle_dot_value;")
    );
    assert!(
        lowered
            .typeql()
            .contains(&format!("$thing has {combining_label} $combining_value;"))
    );
    typeql::parse_query(lowered.typeql()).expect("TypeQL 3.12 accepts lowered query");
}
