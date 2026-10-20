//! Compatibility lowering remains until 11.30 / 3.0.
#![allow(deprecated)]
use type_bridge_core_lib::{__internal as tql, ast::*};

fn literal(value: &str, domain: &str) -> Value {
    Value::Literal(LiteralValue {
        value: serde_json::Value::String(value.into()),
        value_type: domain.into(),
    })
}
fn same_ast(actual: &str, expected: &str) {
    let canonical = |source: &str| typeql::parse_query(source).unwrap().to_string();
    assert_eq!(
        typeql::parse_query(&canonical(actual)).unwrap(),
        typeql::parse_query(&canonical(expected)).unwrap()
    );
}
#[test]
fn attribute_value_is_one_isa_constraint() {
    let source = tql::compile_legacy(&[Clause::Match(vec![Pattern::Attribute {
        variable: "$a".into(),
        type_name: "name".into(),
        value: Some(literal("Ada", "string")),
    }])])
    .unwrap();
    same_ast(&source, "match $a isa name \"Ada\";");
}
#[test]
fn keyed_wildcard_is_an_object() {
    let source = tql::compile_legacy(&[
        Clause::Match(vec![Pattern::Iid {
            variable: "$x".into(),
            iid: "0x1234".into(),
        }]),
        Clause::Fetch(vec![FetchItem::Wildcard {
            key: "all".into(),
            var: "$x".into(),
        }]),
    ])
    .unwrap();
    same_ast(&source, "match $x iid 0x1234; fetch {\"all\": {$x.*}};");
}
#[test]
fn zero_argument_count_is_a_reducer_not_a_function() {
    let source = tql::compile_legacy(&[Clause::Reduce {
        assignments: vec![ReduceAssignment {
            variable: "$n".into(),
            expression: Value::FunctionCall(FunctionCallValue {
                function: "count".into(),
                args: vec![],
            }),
        }],
        group_by: None,
    }])
    .unwrap();
    same_ast(&source, "reduce $n = count;");
    assert!(!source.contains("count()"));
}
#[test]
fn try_keeps_optional_meaning() {
    let source = tql::compile_legacy(&[Clause::Match(vec![Pattern::Try(vec![Pattern::Entity {
        variable: "$x".into(),
        type_name: "person".into(),
        constraints: vec![],
        is_strict: false,
    }])])])
    .unwrap();
    let typeql::query::QueryStructure::Pipeline(query) =
        typeql::parse_query(&source).unwrap().structure
    else {
        panic!("pipeline")
    };
    let typeql::query::stage::Stage::Match(stage) = &query.stages[0] else {
        panic!("match")
    };
    assert!(matches!(stage.patterns[0], typeql::Pattern::Optional(_)));
}
#[test]
fn decimal_never_passes_through_f64() {
    let exact = "0.1000000000000000001";
    let source = tql::compile_legacy(&[Clause::MatchLet(vec![LetAssignment {
        variables: vec!["$value".into()],
        expression: literal(exact, "decimal"),
        is_stream: false,
    }])])
    .unwrap();
    assert!(source.contains(&format!("{exact}dec")));
    same_ast(&source, "match let $value = 0.1000000000000000001dec;");
}
#[test]
fn fetch_keys_are_encoded_as_string_literals() {
    let key = "quote\" and slash\\ and newline\n";
    let source = tql::compile_legacy(&[
        Clause::Match(vec![Pattern::Iid {
            variable: "$x".into(),
            iid: "0x1234".into(),
        }]),
        Clause::Fetch(vec![FetchItem::Variable {
            key: key.into(),
            var: "$x".into(),
        }]),
    ])
    .unwrap();
    let typeql::query::QueryStructure::Pipeline(query) =
        typeql::parse_query(&source).unwrap().structure
    else {
        panic!("pipeline")
    };
    let typeql::query::stage::Stage::Fetch(stage) = &query.stages[1] else {
        panic!("fetch")
    };
    let typeql::query::stage::fetch::FetchObjectBody::Entries(entries) = &stage.object.body else {
        panic!("entries")
    };
    assert_eq!(entries[0].key.unescape().unwrap(), key);
}
#[test]
fn relation_attributes_are_never_discarded() {
    let clause = Clause::Insert(vec![Statement::Relation {
        variable: "$r".into(),
        type_name: "friendship".into(),
        role_players: vec![RolePlayer {
            role: "friend".into(),
            player_var: "$x".into(),
        }],
        include_variable: true,
        attributes: vec![Statement::Isa {
            variable: "$other".into(),
            type_name: "person".into(),
        }],
    }]);
    assert_eq!(
        tql::compile_legacy(&[clause]),
        Err(tql::TqlError::InvalidShape)
    );
}
#[test]
fn raw_escape_hatches_preserve_caller_owned_text() {
    let raw = "$x isa person; $x has name \"Ada\";";
    let source = tql::compile_legacy(&[Clause::Match(vec![Pattern::Raw(raw.into())])]).unwrap();
    assert!(source.contains(raw));
    same_ast(&source, "match $x isa person; $x has name \"Ada\";");
    assert_eq!(
        tql::compile_legacy_node(&QueryNode::Pattern(Pattern::Raw(
            "caller-owned text".into()
        )))
        .unwrap(),
        "caller-owned text"
    );
}

#[test]
fn supported_queries_project_and_compile_without_losing_data() {
    for source in [
        "match $x isa person, has name \"Ada\", iid 0x1234; try { $x has age $a; }; fetch {\"name\": $x.name, \"all\": {$x.*}};",
        "match let $n = 0.1000000000000000001dec;",
        "insert $x isa person, has name \"Ada\";",
        "match $x isa person; update $x has name == \"Grace\";",
        "match $x isa person; delete $x;",
        "match $x isa person; reduce $n = count groupby $x;",
        "match $x isa person; reduce $items = list($x);",
    ] {
        let clauses = tql::project_legacy(&tql::parse(source).unwrap()).unwrap();
        let rendered = tql::compile_legacy(&clauses).unwrap();
        // Write projections can split grouped constraints into equivalent statements.
        let reparsed = tql::project_legacy(&tql::parse(&rendered).unwrap()).unwrap();
        assert_eq!(clauses, reparsed, "{source}");
    }
}

#[test]
fn unsupported_legacy_projections_fail_without_a_raw_fallback() {
    for source in [
        "match $x isa person; select $x;",
        "match $x is $y;",
        "match let $x = 1; $y isa person;",
        "match $x isa person; fetch {\"constant\": 1};",
        "define entity person;",
    ] {
        assert_eq!(
            tql::project_legacy(&tql::parse(source).unwrap()),
            Err(tql::TqlError::Unsupported {
                construct: tql::UnsupportedConstruct::LegacyProjection
            }),
            "{source}"
        );
    }
}

#[test]
fn raw_fetch_entries_are_parsed_and_cannot_inject_another_stage() {
    let safe = FetchItem::Raw("\"quoted\\\"key\": $x".into());
    assert!(tql::lower_legacy_fetch_item(&safe).is_ok());
    for source in ["\"a\": $x, \"b\": $y", "\"a\": $x}; end; fetch {\"b\": $y"] {
        assert!(tql::lower_legacy_fetch_item(&FetchItem::Raw(source.into())).is_err());
    }
}

#[test]
fn released_string_wrappers_keep_compatibility_without_panicking() {
    let compiler = type_bridge_core_lib::compiler::QueryCompiler::new();
    let relation = Statement::Relation {
        variable: "$r".into(),
        type_name: "friendship".into(),
        role_players: vec![RolePlayer {
            role: "friend".into(),
            player_var: "$x".into(),
        }],
        include_variable: true,
        attributes: vec![Statement::Isa {
            variable: "$other".into(),
            type_name: "person".into(),
        }],
    };
    assert_eq!(
        compiler.try_compile(&[Clause::Insert(vec![relation.clone()])]),
        Err(tql::TqlError::InvalidShape)
    );
    let released = compiler.compile(&[Clause::Insert(vec![relation])]);
    assert!(!released.contains("other"));
    assert!(tql::parse(&released).is_ok());
    let invalid = QueryNode::Value(Value::FunctionCall(FunctionCallValue {
        function: "fetch".into(),
        args: vec![],
    }));
    assert!(compiler.try_compile_node(&invalid).is_err());
    assert_eq!(compiler.compile_node(&invalid), "");
    assert_eq!(compiler.compile(&[]), "");
}

#[test]
fn legacy_ownership_deletion_cannot_discard_its_attribute_type() {
    let clauses = [Clause::Delete(vec![Statement::Has {
        subject_var: "$owner".into(),
        attr_name: "name".into(),
        value: Value::Variable("$attribute".into()),
    }])];
    assert_eq!(
        tql::compile_legacy(&clauses),
        Err(tql::TqlError::InvalidShape)
    );
    assert_eq!(
        type_bridge_core_lib::compiler::QueryCompiler::new().compile(&clauses),
        ""
    );
}

#[test]
fn released_expression_strings_use_one_official_expression() {
    for source in ["func($p)", "\"hello world\"", "$x + 1", "person"] {
        let rendered =
            tql::compile_legacy_node(&QueryNode::Value(Value::Variable(source.into()))).unwrap();
        same_ast(
            &format!("match let $result = {rendered};"),
            &format!("match let $result = {source};"),
        );
    }
    for source in ["1; let $other = 2", "1; select $fragment", "1; end"] {
        assert!(
            tql::compile_legacy_node(&QueryNode::Value(Value::Variable(source.into()))).is_err()
        );
    }
}
