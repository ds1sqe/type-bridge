use std::{collections::BTreeSet, path::PathBuf};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, DecimalValue, ValueTypeTag};
use type_bridge_core_lib::__internal as tql;

#[test]
fn every_audited_grammar_construct_has_a_corpus_case() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("tql_corpus/manifest.json")).unwrap();
    let rows = manifest.as_array().unwrap();
    let actual: BTreeSet<_> = rows.iter().map(|row| row["id"].as_u64().unwrap()).collect();
    let expected: BTreeSet<_> = (1..=140)
        .filter(|id| ![64, 80, 131, 140].contains(id))
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(rows.len(), 136);
    for row in rows {
        assert!(corpus().join(row["file"].as_str().unwrap()).is_file());
    }
}

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/tql_corpus")
}

#[test]
fn corpus_round_trips_official_asts() {
    let mut failures = Vec::new();
    let mut count = 0;
    for file in std::fs::read_dir(corpus()).unwrap() {
        let path = file.unwrap().path();
        if path.extension().and_then(|v| v.to_str()) != Some("tql") {
            continue;
        }
        count += 1;
        eprintln!("corpus: {}", path.file_name().unwrap().to_string_lossy());
        let source = std::fs::read_to_string(&path).unwrap();
        let source = source.trim();
        if path.file_stem().unwrap() == "struct-literal" {
            assert!(matches!(
                tql::parse(source),
                Err(tql::TqlError::InvalidSyntax { .. })
            ));
            assert!(typeql::parse_query(source).is_err());
            continue;
        }
        let parsed = match tql::parse(source) {
            Ok(query) => query,
            Err(error) => {
                failures.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let unsupported = match path.file_stem().unwrap().to_str().unwrap() {
            "struct-destructuring" => Some(tql::UnsupportedConstruct::StructDestructuring),
            "end" => Some(tql::UnsupportedConstruct::ExplicitEnd),
            "schema-rename" => Some(tql::UnsupportedConstruct::SchemaRename),
            "schema-redefine-sequence" => Some(tql::UnsupportedConstruct::RedefineSequence),
            _ => None,
        };
        if let Some(construct) = unsupported {
            assert_eq!(
                tql::render(&parsed),
                Err(tql::TqlError::Unsupported { construct })
            );
            continue;
        }
        let rendered = match tql::render(&parsed) {
            Ok(text) => text,
            Err(error) => {
                failures.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if let Err(error) = tql::parse(&rendered) {
            failures.push(format!(
                "{}: rendered invalid syntax: {error}: {rendered}",
                path.display()
            ));
            continue;
        }
        if let Some(destination) = std::env::var_os("TYPE_BRIDGE_TQL_CORPUS_CANONICAL_DIR") {
            let destination = PathBuf::from(destination);
            std::fs::create_dir_all(&destination).unwrap();
            std::fs::write(
                destination.join(path.file_name().unwrap()),
                format!("{rendered}\n"),
            )
            .unwrap();
        } else {
            assert_eq!(
                rendered,
                source,
                "{}: canonical fixture changed",
                path.display()
            );
            // Canonical fixture positions match: compare the actual upstream ASTs,
            // including every expression, constraint, value and fetch projection.
            assert_eq!(
                typeql::parse_query(source).unwrap(),
                typeql::parse_query(&rendered).unwrap()
            );
        }
    }
    assert!(count >= 40);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn typed_builders_cover_the_core_pipeline_and_exact_literals() {
    let decimal = CanonicalValue::Decimal(DecimalValue::new("0.1000000000000000001").unwrap());
    let string = CanonicalValue::String(CanonicalString::new("quote\" and slash\\").unwrap());
    let query = tql::pipeline(vec![
        tql::given(&[("input", ValueTypeTag::String)]).unwrap(),
        tql::match_stage(vec![
            tql::thing(
                "x",
                vec![
                    tql::isa("person", true).unwrap(),
                    tql::has_comparison("name", "==", tql::var("input").unwrap()).unwrap(),
                ],
            )
            .unwrap(),
            tql::optional(vec![
                tql::thing(
                    "x",
                    vec![tql::has("age", tql::var("age").unwrap()).unwrap()],
                )
                .unwrap(),
            ]),
            tql::let_value(&["precise"], tql::literal(&decimal).unwrap(), false).unwrap(),
        ]),
        tql::update(vec![
            tql::thing(
                "x",
                vec![tql::has("name", tql::literal(&string).unwrap()).unwrap()],
            )
            .unwrap(),
        ]),
        tql::select(&["x"]).unwrap(),
        tql::sort(&[("x", false)]).unwrap(),
        tql::offset(0),
        tql::limit(1),
        tql::fetch(vec![(
            "quote\"key",
            tql::fetch_expr(tql::var("x").unwrap()),
        )])
        .unwrap(),
    ]);
    let source = tql::render(&query).unwrap();
    assert!(source.contains("try {"));
    assert!(!source.contains("not {"));
    assert!(source.contains("0.1000000000000000001dec"));
    assert!(source.contains("\\\"key"));
    tql::parse(&source).unwrap();
}

#[test]
fn destructuring_is_refused_at_every_nested_render_entry() {
    for source in [
        "match let {field: $v} = $object;",
        "match try {let {field: $v} = $object;};",
        "define fun f($object: object) -> string: match let {field: $v} = $object; return first $v;",
        "with fun f($object: object) -> string: match let {field: $v} = $object; return first $v; match let $x = 1;",
        "match $x isa person; fetch {\"nested\": [match let {field: $v} = $object; fetch {\"v\": $v};]};",
        "match $x isa person; fetch {\"nested\": (match let {field: $v} = $object; return first $v;)};",
    ] {
        let query = tql::parse(source).unwrap();
        assert_eq!(
            tql::render(&query),
            Err(tql::TqlError::Unsupported {
                construct: tql::UnsupportedConstruct::StructDestructuring
            })
        );
    }
}

#[test]
fn syntax_errors_and_builder_errors_do_not_expose_values() {
    let error = tql::parse("invalid private-value").unwrap_err();
    assert!(matches!(
        error,
        tql::TqlError::InvalidSyntax { line: 1, column: 1 }
    ));
    assert!(!error.to_string().contains("private-value"));
    assert!(tql::var("unsafe; private-value").is_err());
    assert!(tql::isa("unsafe; private-value", false).is_err());
    assert!(tql::iid("0x1234; private-value").is_err());
    assert!(
        tql::compare(
            tql::var("x").unwrap(),
            "unsafe private-value",
            tql::var("y").unwrap()
        )
        .is_err()
    );
}

#[test]
fn every_canonical_scalar_uses_its_exact_typeql_literal_domain() {
    use type_bridge_contract::{temporal::CanonicalDateTimeTz, value::CanonicalDouble};
    let named = CanonicalDateTimeTz::new_named_resolved(
        "2026-10-15T12:30:00.123456789".parse().unwrap(),
        "Europe/London",
        3600,
    )
    .unwrap();
    let values = [
        CanonicalValue::String(CanonicalString::new("text").unwrap()),
        CanonicalValue::Long(i64::MIN),
        CanonicalValue::Double(CanonicalDouble::new(1.5e21).unwrap()),
        CanonicalValue::Boolean(true),
        CanonicalValue::Date("2026-10-15".parse().unwrap()),
        CanonicalValue::DateTime("2026-10-15T12:30:00.123456789".parse().unwrap()),
        CanonicalValue::DateTimeTz(named),
        CanonicalValue::Decimal(DecimalValue::new("0.1000000000000000001").unwrap()),
        CanonicalValue::Duration("PT1.123456789S".parse().unwrap()),
    ];
    for value in values {
        let query = tql::pipeline(vec![tql::match_stage(vec![
            tql::let_value(&["value"], tql::literal(&value).unwrap(), false).unwrap(),
        ])]);
        let source = tql::render(&query).unwrap();
        tql::parse(&source).unwrap();
        if matches!(value, CanonicalValue::DateTimeTz(_)) {
            assert!(source.contains("12:30:00.123456789 Europe/London"));
            let parsed = typeql::parse_query(&source)
                .unwrap()
                .into_structure()
                .into_pipeline();
            let typeql::query::stage::Stage::Match(stage) = &parsed.stages[0] else {
                panic!("match")
            };
            let typeql::Pattern::Statement(typeql::Statement::Assignment(assignment)) =
                &stage.patterns[0]
            else {
                panic!("assignment")
            };
            let typeql::Expression::Value(literal) = &assignment.rhs else {
                panic!("literal")
            };
            assert_eq!(
                literal.inner,
                typeql::parse_value("2026-10-15T12:30:00.123456789 Europe/London").unwrap()
            );
        }
    }
}

#[test]
fn write_pattern_and_reduce_builders_produce_official_queries() {
    let thing = || tql::thing("x", vec![tql::isa("person", false).unwrap()]).unwrap();
    let relation = tql::thing(
        "r",
        vec![
            tql::isa("friendship", false).unwrap(),
            tql::links(vec![("friend", "x")]).unwrap(),
        ],
    )
    .unwrap();
    let alternatives = tql::or(vec![vec![thing()], vec![tql::not(vec![thing()])]]);
    let computed = tql::let_value(
        &["id"],
        tql::function("iid", vec![tql::var("x").unwrap()]).unwrap(),
        false,
    )
    .unwrap();
    for stages in [
        vec![tql::insert(vec![thing(), relation])],
        vec![tql::put(vec![thing()])],
        vec![
            tql::match_stage(vec![thing()]),
            tql::delete(vec![tql::delete_thing("x").unwrap()]),
        ],
        vec![
            tql::match_stage(vec![alternatives, computed]),
            tql::count("count", None, &[]).unwrap(),
        ],
    ] {
        tql::parse(&tql::render(&tql::pipeline(stages)).unwrap()).unwrap();
    }
    for reducer in [
        "count", "list", "sum", "min", "max", "mean", "median", "std",
    ] {
        let stage = tql::reduce_stage(
            vec![("result", tql::reducer(reducer, Some("value")).unwrap())],
            &["group"],
        )
        .unwrap();
        tql::parse(
            &tql::render(&tql::pipeline(vec![tql::match_stage(vec![thing()]), stage])).unwrap(),
        )
        .unwrap();
    }
    let qualified = tql::function("math::absolute", vec![tql::var("value").unwrap()]).unwrap();
    tql::parse(
        &tql::render(&tql::pipeline(vec![tql::match_stage(vec![
            tql::let_value(&["result"], qualified, false).unwrap(),
        ])]))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn caller_owned_function_bodies_survive_query_and_document_rendering() {
    let body = "\n# leading comment\nmatch\n  # retained comment\n  let $value = $seed;\nreturn first $value; # trailing comment";
    let build = || tql::schema_function("fun keep-body($seed: integer) -> integer", body).unwrap();
    let next = || tql::schema_function("fun next-body($seed: integer) -> integer", body).unwrap();
    assert!(
        tql::render_schema_definition(build())
            .unwrap()
            .contains(body)
    );
    for output in [
        tql::render(&tql::schema_define(vec![build(), next()])).unwrap(),
        tql::render_schema_document(vec![
            (build(), vec!["function metadata".to_owned()]),
            (next(), vec![]),
        ])
        .unwrap(),
    ] {
        assert_eq!(output.matches(body).count(), 2, "{output}");
        let original = format!(
            "define fun keep-body($seed: integer) -> integer:\n{body}\nfun next-body($seed: integer) -> integer:\n{body}\n"
        );
        assert!(tql::equivalent(&original, &output).unwrap());
    }
}

#[test]
fn caller_owned_function_body_cannot_add_another_definition() {
    assert!(
        tql::schema_function(
            "fun keep-body($seed: integer) -> integer",
            "match let $value = $seed; return first $value; entity injected;",
        )
        .is_err()
    );
}

#[test]
fn schema_undefinitions_and_typed_values_round_trip() {
    let definitions = vec![
        tql::schema_undefine_type("person").unwrap(),
        tql::schema_undefine_function("answer").unwrap(),
        tql::schema_undefine_struct("record").unwrap(),
        tql::schema_undefine_capability("person", tql::schema_owns("name", false, vec![]).unwrap())
            .unwrap(),
        tql::schema_undefine_specialisation("family", "father", "parent").unwrap(),
        tql::schema_undefine_annotation("meta", Some("owner"), "person", None).unwrap(),
        tql::schema_undefine_annotation(
            "card",
            None,
            "person",
            Some(tql::schema_plays("family", "parent", vec![]).unwrap()),
        )
        .unwrap(),
    ];
    let source = tql::render(&tql::schema_undefine(definitions)).unwrap();
    assert!(tql::equivalent(&source, "undefine person; fun answer; struct record; owns name from person; as parent from family relates father; @meta(\"owner\") from person; @card from person plays family:parent;").unwrap());
    for category in [
        "abstract",
        "card",
        "cascade",
        "distinct",
        "doc",
        "independent",
        "key",
        "range",
        "regex",
        "subkey",
        "unique",
        "values",
    ] {
        let source = tql::render(&tql::schema_undefine(vec![
            tql::schema_undefine_annotation(category, None, "person", None).unwrap(),
        ]))
        .unwrap();
        tql::parse(&source).unwrap();
    }
    let values = [CanonicalValue::Long(1), CanonicalValue::Long(2)];
    let definition = tql::schema_type(
        Some("attribute"),
        "number",
        vec![],
        vec![tql::schema_value_domain(
            ValueTypeTag::Long,
            vec![
                tql::schema_canonical_values(&values).unwrap(),
                tql::schema_canonical_range(Some(&values[0]), Some(&values[1])).unwrap(),
            ],
        )],
    )
    .unwrap();
    let source = tql::render(&tql::schema_define(vec![definition])).unwrap();
    assert!(
        tql::equivalent(
            &source,
            "define attribute number, value integer @values(1, 2) @range(1..2);"
        )
        .unwrap()
    );
}

#[test]
fn typed_tuple_function_headers_preserve_body_in_define_and_redefine() {
    use type_bridge_contract::id::{FunctionId, Label};
    use type_bridge_contract::schema::{
        FunctionBody, FunctionFact, FunctionParameter, FunctionReturnElement, FunctionReturnMode,
        FunctionSignature, TypeReference,
    };
    let body =
        "match\n  # retained tuple comment\n  let $value = $seed;\nreturn first $value, $value;\n";
    let element = FunctionReturnElement::new(TypeReference::Value(ValueTypeTag::Long), false);
    let function = FunctionFact::new(
        FunctionId::new("pair").unwrap(),
        FunctionSignature::new(
            vec![FunctionParameter::new(
                Label::new("seed").unwrap(),
                TypeReference::Value(ValueTypeTag::Long),
            )],
            FunctionReturnMode::tuple(vec![element.clone(), element]).unwrap(),
        )
        .unwrap(),
        FunctionBody::new(body).unwrap(),
    );
    for redefine in [false, true] {
        let definition =
            tql::schema_function_fact(&function, vec![tql::schema_doc("pair docs").unwrap()])
                .unwrap();
        let query = if redefine {
            tql::schema_redefine(vec![definition])
        } else {
            tql::schema_define(vec![definition])
        };
        let source = tql::render(&query).unwrap();
        assert!(
            source.contains("-> integer, integer @doc(\"pair docs\")"),
            "{source}"
        );
        assert!(source.contains(body), "{source}");
        tql::parse(&source).unwrap();
        assert_eq!(function.body().text(), body);
    }
}

#[test]
fn legacy_role_specialisations_keep_matching_ordering() {
    for (ordered, parent, expected) in [
        (true, "participant", "reviewer[] as participant[]"),
        (true, "participant[]", "reviewer[] as participant[]"),
        (false, "participant", "reviewer as participant"),
    ] {
        let capability =
            tql::schema_relates_with_parent_source("reviewer", ordered, Some(parent), vec![])
                .unwrap();
        let source = tql::render(&tql::schema_define(vec![
            tql::schema_type(None, "review", vec![], vec![capability]).unwrap(),
        ]))
        .unwrap();
        assert!(source.contains(expected), "{source}");
        tql::parse(&source).unwrap();
    }
    assert!(
        tql::schema_relates_with_parent_source("reviewer", false, Some("participant[]"), vec![],)
            .is_err()
    );
    assert!(tql::parse("define review relates reviewer[] as participant;").is_err());
}

#[test]
fn expression_fragments_and_tagged_queries_share_render_guards() {
    let source = "2024-07-01T12:00:00 Europe/Amsterdam";
    let expression = tql::literal_source(source).unwrap();
    assert_eq!(tql::render_expression(&expression), source);
    assert!(tql::literal_source("1; insert $x isa person;").is_err());
    let query = tql::pipeline(vec![tql::match_stage(vec![
        tql::schema_kind_pattern("$t", "entity").unwrap(),
    ])]);
    let tagged = tql::render_commented(&query, "internal-probe/v1").unwrap();
    assert!(tagged.starts_with("# internal-probe/v1\n"));
    assert!(tql::equivalent(&tagged, "match entity $t;").unwrap());
    assert!(tql::render_commented(&query, "first\ninsert $x isa person;").is_err());
}
