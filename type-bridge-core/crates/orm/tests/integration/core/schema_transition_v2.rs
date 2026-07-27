//! Live TypeDB 3.12.1 schema-transition evidence for Schema V2 lowering.

use crate::common::dynamic_crud::unique_schema_suffix;
use crate::common::rust_binding::setup_db;
use type_bridge_orm::TxType;
use type_bridge_orm::session::backend::QueryResult;

fn declaration<'a>(schema: &'a str, head: &str) -> &'a str {
    let start = schema
        .find(head)
        .unwrap_or_else(|| panic!("schema export omitted declaration {head:?}:\n{schema}"));
    let tail = &schema[start..];
    let end = tail
        .find(';')
        .unwrap_or_else(|| panic!("schema declaration {head:?} was not terminated:\n{tail}"));
    &tail[..=end]
}

fn assert_single_row(result: QueryResult, context: &str) {
    let QueryResult::Rows(rows) = result else {
        panic!("{context} returned a non-row result: {result:?}")
    };
    assert_eq!(rows.len(), 1, "{context} did not preserve exactly one row");
}

#[tokio::test]
async fn low1_redefine_is_singleton_and_rejection_preserves_every_prior_change() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-low1");
    let root_a = format!("{suffix}-root-a");
    let root_b = format!("{suffix}-root-b");
    let root_c = format!("{suffix}-root-c");
    let child = format!("{suffix}-child");
    let scalar = format!("{suffix}-scalar");
    let owned = format!("{suffix}-owned");
    let owner = format!("{suffix}-owner");
    let parent_relation = format!("{suffix}-parent-link");
    let parent_role_a = format!("{suffix}-parent-role-a");
    let parent_role_b = format!("{suffix}-parent-role-b");
    let child_relation = format!("{suffix}-child-link");
    let child_role = format!("{suffix}-child-role");
    let function = format!("{suffix}-constant");

    db.execute_raw(
        &format!(
            "define\n\
             entity {root_a};\n\
             entity {root_b};\n\
             entity {root_c};\n\
             entity {child} sub {root_a};\n\
             attribute {scalar}, value string;\n\
             attribute {owned}, value string;\n\
             entity {owner}, owns {owned} @card(0..1);\n\
             relation {parent_relation} @abstract, \
               relates {parent_role_a}, relates {parent_role_b};\n\
             relation {child_relation} sub {parent_relation}, \
               relates {child_role} as {parent_role_a};\n\
             fun {function}() -> integer:\n\
               match\n\
                 let $value = 11;\n\
               return first $value;"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW1 fixture schema must be accepted by TypeDB 3.12.1");

    for (case, query) in [
        ("sub", format!("redefine\n{child} sub {root_b};")),
        ("value", format!("redefine\n{scalar} value integer;")),
        (
            "relates specialization",
            format!("redefine\n{child_relation} relates {child_role} as {parent_role_b};"),
        ),
        (
            "ordinary parameterized annotation",
            format!("redefine\n{owner} owns {owned} @card(0..2);"),
        ),
        (
            "function",
            format!(
                "redefine\n\
                 fun {function}() -> integer:\n\
                   match\n\
                     let $value = 22;\n\
                   return first $value;"
            ),
        ),
    ] {
        db.execute_raw(&query, TxType::Schema)
            .await
            .unwrap_or_else(|error| panic!("LOW1 singleton {case} redefine failed: {error}"));
    }

    let before_rejection = db
        .schema_text()
        .await
        .expect("LOW1 schema must export after singleton redefines");
    let child_declaration = declaration(&before_rejection, &format!("entity {child}"));
    assert!(child_declaration.contains(&format!("sub {root_b}")));
    let scalar_declaration = declaration(&before_rejection, &format!("attribute {scalar}"));
    assert!(scalar_declaration.contains("value integer"));
    let relation_declaration =
        declaration(&before_rejection, &format!("relation {child_relation}"));
    assert!(relation_declaration.contains(&format!("relates {child_role} as {parent_role_b}")));
    let owner_declaration = declaration(&before_rejection, &format!("entity {owner}"));
    assert!(owner_declaration.contains(&format!("owns {owned} @card(0..2)")));
    assert!(
        before_rejection.contains("let $value = 22;"),
        "function singleton redefine was not exported:\n{before_rejection}"
    );

    let error = db
        .execute_raw(
            &format!(
                "redefine\n\
                 {child} sub {root_c};\n\
                 {owner} owns {owned} @card(0..3);"
            ),
            TxType::Schema,
        )
        .await
        .expect_err("TypeDB 3.12.1 must reject a redefine query with two actual changes");
    assert!(
        !error.to_string().is_empty(),
        "LOW1 multi-change rejection returned an empty diagnostic"
    );

    let after_rejection = db
        .schema_text()
        .await
        .expect("LOW1 schema must export after rejected multi-change redefine");
    assert_eq!(
        after_rejection, before_rejection,
        "a rejected multi-change redefine modified committed schema"
    );
}

#[tokio::test]
async fn low2_relates_specialization_transitions_preserve_existing_data() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-low2");
    let parent_relation = format!("{suffix}-parent-link");
    let parent_role_a = format!("{suffix}-parent-role-a");
    let parent_role_b = format!("{suffix}-parent-role-b");
    let child_relation = format!("{suffix}-child-link");
    let child_role = format!("{suffix}-child-role");
    let player = format!("{suffix}-player");

    db.execute_raw(
        &format!(
            "define\n\
             relation {parent_relation} @abstract, \
               relates {parent_role_a}, relates {parent_role_b};\n\
             relation {child_relation} sub {parent_relation}, relates {child_role};\n\
             entity {player}, plays {child_relation}:{child_role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW2 fixture schema must be accepted by TypeDB 3.12.1");
    db.execute_raw(
        &format!(
            "insert\n\
             $player isa {player};\n\
             ({child_role}: $player) isa {child_relation};"
        ),
        TxType::Write,
    )
    .await
    .expect("LOW2 fixture data must commit before specialization transitions");

    db.execute_raw(
        &format!("define\nrelation {child_relation}, relates {child_role} as {parent_role_a};"),
        TxType::Schema,
    )
    .await
    .expect("LOW2 None-to-Some specialization must preserve existing data");
    let exported = db
        .schema_text()
        .await
        .expect("LOW2 None-to-Some schema must export");
    assert!(
        declaration(&exported, &format!("relation {child_relation}"))
            .contains(&format!("relates {child_role} as {parent_role_a}"))
    );
    assert_single_row(
        db.execute_raw(
            &format!("match\n$relation isa {child_relation};\nselect $relation;"),
            TxType::Read,
        )
        .await
        .expect("LOW2 data query must succeed after None-to-Some"),
        "LOW2 None-to-Some data probe",
    );

    db.execute_raw(
        &format!("redefine\n{child_relation} relates {child_role} as {parent_role_b};"),
        TxType::Schema,
    )
    .await
    .expect("LOW2 Some-to-Some specialization must preserve existing data");
    let exported = db
        .schema_text()
        .await
        .expect("LOW2 Some-to-Some schema must export");
    assert!(
        declaration(&exported, &format!("relation {child_relation}"))
            .contains(&format!("relates {child_role} as {parent_role_b}"))
    );
    assert_single_row(
        db.execute_raw(
            &format!("match\n$relation isa {child_relation};\nselect $relation;"),
            TxType::Read,
        )
        .await
        .expect("LOW2 data query must succeed after Some-to-Some"),
        "LOW2 Some-to-Some data probe",
    );

    db.execute_raw(
        &format!("undefine\nas {parent_role_b} from {child_relation} relates {child_role};"),
        TxType::Schema,
    )
    .await
    .expect("LOW2 Some-to-None specialization must preserve existing data");
    let exported = db
        .schema_text()
        .await
        .expect("LOW2 Some-to-None schema must export");
    let child_declaration = declaration(&exported, &format!("relation {child_relation}"));
    assert!(child_declaration.contains(&format!("relates {child_role}")));
    assert!(
        !child_declaration.contains(&format!("relates {child_role} as")),
        "LOW2 Some-to-None retained specialization:\n{child_declaration}"
    );
    assert_single_row(
        db.execute_raw(
            &format!("match\n$relation isa {child_relation};\nselect $relation;"),
            TxType::Read,
        )
        .await
        .expect("LOW2 data query must succeed after Some-to-None"),
        "LOW2 Some-to-None data probe",
    );
}

#[tokio::test]
async fn low3_annotations_add_change_and_remove_on_every_ordinary_subject() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    if !crate::common::rust_binding::server_supports_v2_conformance(&db) {
        eprintln!("skipping: V2 annotation transitions require a proven TypeDB 3.12+ server");
        return;
    }
    let suffix = unique_schema_suffix("rust", "schema-v2-low3");
    let documented = format!("{suffix}-documented");
    let value = format!("{suffix}-value");
    let owner = format!("{suffix}-owner");
    let relation = format!("{suffix}-link");
    let role = format!("{suffix}-role");
    let player = format!("{suffix}-player");
    let doc_added = format!("{suffix}-doc-added");
    let doc_changed = format!("{suffix}-doc-changed");

    db.execute_raw(
        &format!(
            "define\n\
             entity {documented};\n\
             attribute {value}, value string;\n\
             entity {owner}, owns {value};\n\
             relation {relation}, relates {role};\n\
             entity {player}, plays {relation}:{role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW3 fixture schema must be accepted by TypeDB 3.12.1");

    db.execute_raw(
        &format!(
            "define\n\
             entity {documented} @doc(\"{doc_added}\");\n\
             attribute {value}, value string @regex(\"^a+$\");\n\
             entity {owner}, owns {value} @card(0..2);\n\
             relation {relation}, relates {role} @card(0..2);\n\
             entity {player}, plays {relation}:{role} @card(0..2);"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW3 annotation additions must be accepted");
    let added = db
        .schema_text()
        .await
        .expect("LOW3 added annotations must export");
    assert!(
        declaration(&added, &format!("entity {documented}"))
            .contains(&format!("@doc(\"{doc_added}\")"))
    );
    assert!(declaration(&added, &format!("attribute {value}")).contains("@regex(\"^a+$\")"));
    assert!(
        declaration(&added, &format!("entity {owner}"))
            .contains(&format!("owns {value} @card(0..2)"))
    );
    assert!(
        declaration(&added, &format!("relation {relation}"))
            .contains(&format!("relates {role} @card(0..2)"))
    );
    assert!(
        declaration(&added, &format!("entity {player}"))
            .contains(&format!("plays {relation}:{role} @card(0..2)"))
    );

    for (subject, query) in [
        (
            "Type",
            format!("redefine\n{documented} @doc(\"{doc_changed}\");"),
        ),
        (
            "Value",
            format!("redefine\n{value} value string @regex(\"^b+$\");"),
        ),
        (
            "Owns",
            format!("redefine\n{owner} owns {value} @card(0..3);"),
        ),
        (
            "Relates",
            format!("redefine\n{relation} relates {role} @card(0..3);"),
        ),
        (
            "Plays",
            format!("redefine\n{player} plays {relation}:{role} @card(0..3);"),
        ),
    ] {
        db.execute_raw(&query, TxType::Schema)
            .await
            .unwrap_or_else(|error| panic!("LOW3 {subject} annotation change failed: {error}"));
    }
    let changed = db
        .schema_text()
        .await
        .expect("LOW3 changed annotations must export");
    assert!(
        declaration(&changed, &format!("entity {documented}"))
            .contains(&format!("@doc(\"{doc_changed}\")"))
    );
    assert!(declaration(&changed, &format!("attribute {value}")).contains("@regex(\"^b+$\")"));
    assert!(
        declaration(&changed, &format!("entity {owner}"))
            .contains(&format!("owns {value} @card(0..3)"))
    );
    assert!(
        declaration(&changed, &format!("relation {relation}"))
            .contains(&format!("relates {role} @card(0..3)"))
    );
    assert!(
        declaration(&changed, &format!("entity {player}"))
            .contains(&format!("plays {relation}:{role} @card(0..3)"))
    );

    db.execute_raw(
        &format!(
            "undefine\n\
             @doc from {documented};\n\
             @regex from {value} value string;\n\
             @card from {owner} owns {value};\n\
             @card from {relation} relates {role};\n\
             @card from {player} plays {relation}:{role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW3 annotation removals must be accepted");
    let removed = db
        .schema_text()
        .await
        .expect("LOW3 annotation removals must export");
    assert!(!declaration(&removed, &format!("entity {documented}")).contains("@doc"));
    assert!(!declaration(&removed, &format!("attribute {value}")).contains("@regex"));
    assert!(!declaration(&removed, &format!("entity {owner}")).contains("@card"));
    assert!(!declaration(&removed, &format!("relation {relation}")).contains("@card"));
    assert!(!declaration(&removed, &format!("entity {player}")).contains("@card"));
}

#[tokio::test]
async fn low4_sub_annotations_require_atomic_replace_and_rollback_restores_them() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    if !crate::common::rust_binding::server_supports_v2_conformance(&db) {
        eprintln!("skipping: sub @doc/@meta annotations require a proven TypeDB 3.12+ server");
        return;
    }
    let suffix = unique_schema_suffix("rust", "schema-v2-low4");
    let base = format!("{suffix}-base");
    let child = format!("{suffix}-child");
    let original_doc = format!("{suffix}-doc-original");
    let original_meta = format!("{suffix}-meta-original");
    let committed_doc = format!("{suffix}-doc-committed");
    let committed_meta = format!("{suffix}-meta-committed");
    let rejected_doc = format!("{suffix}-doc-rejected");
    let rejected_meta = format!("{suffix}-meta-rejected");
    let rolled_back_doc = format!("{suffix}-doc-rolled-back");
    let rolled_back_meta = format!("{suffix}-meta-rolled-back");

    db.execute_raw(
        &format!(
            "define\n\
             entity {base};\n\
             entity {child} sub {base} \
               @doc(\"{original_doc}\") @meta(\"owner\", \"{original_meta}\");"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW4 fixture schema must be accepted by TypeDB 3.12.1");

    for (kind, query) in [
        (
            "doc",
            format!("redefine\n{child} sub {base} @doc(\"{rejected_doc}\");"),
        ),
        (
            "meta",
            format!("redefine\n{child} sub {base} @meta(\"owner\", \"{rejected_meta}\");"),
        ),
    ] {
        let error = db
            .execute_raw(&query, TxType::Schema)
            .await
            .expect_err(&format!(
                "TypeDB 3.12.1 must reject direct {kind} redefine on sub"
            ));
        assert!(
            !error.to_string().is_empty(),
            "LOW4 direct {kind} rejection returned an empty diagnostic"
        );
    }
    let after_rejections = db
        .schema_text()
        .await
        .expect("LOW4 schema must export after direct-redefine rejections");
    let child_declaration = declaration(&after_rejections, &format!("entity {child}"));
    assert!(child_declaration.contains(&format!("@doc(\"{original_doc}\")")));
    assert!(child_declaration.contains(&format!("@meta(\"owner\", \"{original_meta}\")")));

    let committed = db
        .transaction_context(TxType::Schema)
        .await
        .expect("LOW4 committed fallback schema transaction must open");
    committed
        .query(&format!(
            "undefine\n\
             @doc from {child} sub {base};\n\
             @meta(\"owner\") from {child} sub {base};"
        ))
        .await
        .expect("LOW4 fallback must undefine old sub annotations");
    committed
        .query(&format!(
            "define\n\
             {child} sub {base} \
               @doc(\"{committed_doc}\") @meta(\"owner\", \"{committed_meta}\");"
        ))
        .await
        .expect("LOW4 fallback must define replacement sub annotations");
    committed
        .commit()
        .await
        .expect("LOW4 fallback schema transaction must commit atomically");

    let after_commit = db
        .schema_text()
        .await
        .expect("LOW4 committed fallback must export");
    let child_declaration = declaration(&after_commit, &format!("entity {child}"));
    assert!(child_declaration.contains(&format!("@doc(\"{committed_doc}\")")));
    assert!(child_declaration.contains(&format!("@meta(\"owner\", \"{committed_meta}\")")));
    assert!(!child_declaration.contains(&original_doc));
    assert!(!child_declaration.contains(&original_meta));

    let rolled_back = db
        .transaction_context(TxType::Schema)
        .await
        .expect("LOW4 rollback schema transaction must open");
    rolled_back
        .query(&format!(
            "undefine\n\
             @doc from {child} sub {base};\n\
             @meta(\"owner\") from {child} sub {base};"
        ))
        .await
        .expect("LOW4 rollback probe must undefine committed sub annotations");
    rolled_back
        .query(&format!(
            "define\n\
             {child} sub {base} \
               @doc(\"{rolled_back_doc}\") @meta(\"owner\", \"{rolled_back_meta}\");"
        ))
        .await
        .expect("LOW4 rollback probe must define temporary sub annotations");
    rolled_back
        .rollback()
        .await
        .expect("LOW4 schema transaction must accept a forced rollback");

    let after_rollback = db
        .schema_text()
        .await
        .expect("LOW4 schema must export after forced rollback");
    assert_eq!(
        after_rollback, after_commit,
        "LOW4 forced rollback did not restore the committed sub annotations"
    );
}

#[tokio::test]
async fn low5_meta_redefine_and_undefine_are_isolated_by_key() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    if !crate::common::rust_binding::server_supports_v2_conformance(&db) {
        eprintln!("skipping: @meta key isolation requires a proven TypeDB 3.12+ server");
        return;
    }
    let suffix = unique_schema_suffix("rust", "schema-v2-low5");
    let subject = format!("{suffix}-subject");
    let owner_original = format!("{suffix}-owner-original");
    let owner_changed = format!("{suffix}-owner-changed");
    let source = format!("{suffix}-source");

    db.execute_raw(
        &format!(
            "define\n\
             entity {subject} \
               @meta(\"owner\", \"{owner_original}\") \
               @meta(\"source\", \"{source}\");"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW5 fixture with two meta keys must be accepted by TypeDB 3.12.1");

    db.execute_raw(
        &format!("redefine\n{subject} @meta(\"owner\", \"{owner_changed}\");"),
        TxType::Schema,
    )
    .await
    .expect("LOW5 redefining one meta key must succeed");
    let changed = db
        .schema_text()
        .await
        .expect("LOW5 redefined meta keys must export");
    let subject_declaration = declaration(&changed, &format!("entity {subject}"));
    assert!(subject_declaration.contains(&format!("@meta(\"owner\", \"{owner_changed}\")")));
    assert!(subject_declaration.contains(&format!("@meta(\"source\", \"{source}\")")));
    assert!(!subject_declaration.contains(&owner_original));

    db.execute_raw(
        &format!("undefine\n@meta(\"owner\") from {subject};"),
        TxType::Schema,
    )
    .await
    .expect("LOW5 keyed meta undefine must remove only the selected key");
    let removed = db
        .schema_text()
        .await
        .expect("LOW5 keyed meta removal must export");
    let subject_declaration = declaration(&removed, &format!("entity {subject}"));
    assert!(!subject_declaration.contains("@meta(\"owner\""));
    assert!(subject_declaration.contains(&format!("@meta(\"source\", \"{source}\")")));
}

fn single_row_json(result: QueryResult, context: &str) -> String {
    let QueryResult::Rows(rows) = result else {
        panic!("{context} returned a non-row result: {result:?}")
    };
    assert_eq!(rows.len(), 1, "{context} did not return exactly one row");
    serde_json::to_string(&rows[0]).expect("raw TypeDB row must serialize as JSON")
}

#[tokio::test]
async fn low6_function_metadata_requires_transactional_replacement() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    if !crate::common::rust_binding::server_supports_v2_conformance(&db) {
        eprintln!("skipping: function @doc/@meta annotations require a proven TypeDB 3.12+ server");
        return;
    }
    let suffix = unique_schema_suffix("rust", "schema-v2-low6");
    let target = format!("{suffix}-target");
    let caller = format!("{suffix}-caller");
    let original_doc = format!("{suffix}-doc-original");
    let original_meta = format!("{suffix}-meta-original");
    let raw_doc = format!("{suffix}-doc-raw-redefine");
    let raw_meta = format!("{suffix}-meta-raw-redefine");
    let committed_doc = format!("{suffix}-doc-committed");
    let committed_meta = format!("{suffix}-meta-committed");
    let rolled_back_doc = format!("{suffix}-doc-rolled-back");
    let rolled_back_meta = format!("{suffix}-meta-rolled-back");

    db.execute_raw(
        &format!(
            "define\n\
             fun {target}() -> integer \
               @doc(\"{original_doc}\") @meta(\"owner\", \"{original_meta}\"):\n\
               match\n\
                 let $value = 6101;\n\
               return first $value;\n\
             fun {caller}() -> integer:\n\
               match\n\
                 let $value = {target}();\n\
               return first $value;"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW6 annotated target and caller functions must define");

    let original_export = db
        .schema_text()
        .await
        .expect("LOW6 original functions must export");
    for marker in [&original_doc, &original_meta, "let $value = 6101;"] {
        assert!(
            original_export.contains(marker),
            "LOW6 original export omitted {marker:?}:\n{original_export}"
        );
    }
    let original_properties = single_row_json(
        db.execute_raw(
            &format!(
                "match\n\
                 let $doc = get_fun_doc(\"{target}\");\n\
                 let $meta = get_fun_meta(\"owner\", \"{target}\");\n\
                 select $doc, $meta;"
            ),
            TxType::Read,
        )
        .await
        .expect("LOW6 original get_fun_doc/get_fun_meta query must succeed"),
        "LOW6 original function metadata",
    );
    assert!(original_properties.contains(&original_doc));
    assert!(original_properties.contains(&original_meta));

    db.execute_raw(
        &format!(
            "redefine\n\
             fun {target}() -> integer \
               @doc(\"{raw_doc}\") @meta(\"owner\", \"{raw_meta}\"):\n\
               match\n\
                 let $value = 6102;\n\
               return first $value;"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW6 whole-function redefine must accept changed raw syntax");

    let raw_redefined_export = db
        .schema_text()
        .await
        .expect("LOW6 raw-redefined function must export");
    for marker in [&raw_doc, &raw_meta, "let $value = 6102;"] {
        assert!(
            raw_redefined_export.contains(marker),
            "LOW6 raw redefine export omitted {marker:?}:\n{raw_redefined_export}"
        );
    }
    let stale_properties = single_row_json(
        db.execute_raw(
            &format!(
                "match\n\
                 let $doc = get_fun_doc(\"{target}\");\n\
                 let $meta = get_fun_meta(\"owner\", \"{target}\");\n\
                 select $doc, $meta;"
            ),
            TxType::Read,
        )
        .await
        .expect("LOW6 metadata builtins must remain readable after full redefine"),
        "LOW6 stale function metadata",
    );
    assert!(stale_properties.contains(&original_doc));
    assert!(stale_properties.contains(&original_meta));
    assert!(!stale_properties.contains(&raw_doc));
    assert!(!stale_properties.contains(&raw_meta));
    let caller_after_redefine = single_row_json(
        db.execute_raw(
            &format!("match\nlet $value = {caller}();\nselect $value;"),
            TxType::Read,
        )
        .await
        .expect("LOW6 caller must remain valid after whole-function redefine"),
        "LOW6 caller after redefine",
    );
    assert!(caller_after_redefine.contains("6102"));

    let committed = db
        .transaction_context(TxType::Schema)
        .await
        .expect("LOW6 replacement schema transaction must open");
    committed
        .query(&format!("undefine\nfun {target};"))
        .await
        .expect("LOW6 replacement must undefine the target with a live caller");
    committed
        .query(&format!(
            "define\n\
             fun {target}() -> integer \
               @doc(\"{committed_doc}\") @meta(\"owner\", \"{committed_meta}\"):\n\
               match\n\
                 let $value = 6103;\n\
               return first $value;"
        ))
        .await
        .expect("LOW6 replacement must redefine the target before commit");
    committed
        .commit()
        .await
        .expect("LOW6 target replacement with a caller must commit atomically");

    let committed_export = db
        .schema_text()
        .await
        .expect("LOW6 committed function replacement must export");
    for marker in [&committed_doc, &committed_meta, "let $value = 6103;"] {
        assert!(
            committed_export.contains(marker),
            "LOW6 committed export omitted {marker:?}:\n{committed_export}"
        );
    }
    let committed_properties = single_row_json(
        db.execute_raw(
            &format!(
                "match\n\
                 let $doc = get_fun_doc(\"{target}\");\n\
                 let $meta = get_fun_meta(\"owner\", \"{target}\");\n\
                 select $doc, $meta;"
            ),
            TxType::Read,
        )
        .await
        .expect("LOW6 committed metadata builtins must succeed"),
        "LOW6 committed function metadata",
    );
    assert!(committed_properties.contains(&committed_doc));
    assert!(committed_properties.contains(&committed_meta));
    let caller_after_commit = single_row_json(
        db.execute_raw(
            &format!("match\nlet $value = {caller}();\nselect $value;"),
            TxType::Read,
        )
        .await
        .expect("LOW6 caller must resolve the transactionally replaced target"),
        "LOW6 caller after committed replacement",
    );
    assert!(caller_after_commit.contains("6103"));

    let rolled_back = db
        .transaction_context(TxType::Schema)
        .await
        .expect("LOW6 rollback schema transaction must open");
    rolled_back
        .query(&format!("undefine\nfun {target};"))
        .await
        .expect("LOW6 rollback probe must undefine the target");
    rolled_back
        .query(&format!(
            "define\n\
             fun {target}() -> integer \
               @doc(\"{rolled_back_doc}\") @meta(\"owner\", \"{rolled_back_meta}\"):\n\
               match\n\
                 let $value = 6104;\n\
               return first $value;"
        ))
        .await
        .expect("LOW6 rollback probe must define a temporary replacement");
    rolled_back
        .rollback()
        .await
        .expect("LOW6 caller-owned schema transaction must roll back");

    let after_rollback = db
        .schema_text()
        .await
        .expect("LOW6 schema must export after forced rollback");
    assert_eq!(
        after_rollback, committed_export,
        "LOW6 forced rollback did not restore function syntax and annotations"
    );
    let restored_properties = single_row_json(
        db.execute_raw(
            &format!(
                "match\n\
                 let $doc = get_fun_doc(\"{target}\");\n\
                 let $meta = get_fun_meta(\"owner\", \"{target}\");\n\
                 select $doc, $meta;"
            ),
            TxType::Read,
        )
        .await
        .expect("LOW6 restored metadata builtins must succeed"),
        "LOW6 restored function metadata",
    );
    assert!(restored_properties.contains(&committed_doc));
    assert!(restored_properties.contains(&committed_meta));
    assert!(!restored_properties.contains(&rolled_back_doc));
    assert!(!restored_properties.contains(&rolled_back_meta));
    let caller_after_rollback = single_row_json(
        db.execute_raw(
            &format!("match\nlet $value = {caller}();\nselect $value;"),
            TxType::Read,
        )
        .await
        .expect("LOW6 caller must resolve the restored target after rollback"),
        "LOW6 caller after rollback",
    );
    assert!(caller_after_rollback.contains("6103"));
}

#[tokio::test]
async fn low7_user_defined_struct_transitions_are_unavailable() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let record = unique_schema_suffix("rust", "schema-v2-low7").replace('-', "_");
    let before = db
        .schema_text()
        .await
        .expect("LOW7 schema must export before unavailable struct transitions");

    for (case, query) in [
        (
            "whole struct define",
            format!("define\nstruct {record}, value alpha integer;"),
        ),
        (
            "partial struct append",
            format!("define\nstruct {record}, value middle boolean;"),
        ),
        (
            "struct redefine",
            format!("redefine\nstruct {record}, value alpha string;"),
        ),
        (
            "struct field undefine",
            format!("undefine\nvalue alpha from struct {record};"),
        ),
    ] {
        let error = db
            .execute_raw(&query, TxType::Schema)
            .await
            .expect_err(&format!(
                "LOW7 TypeDB 3.12.1 must reject unavailable {case}: {query}"
            ));
        let message = error.to_string();
        assert!(
            !message.is_empty(),
            "LOW7 {case} returned an empty diagnostic"
        );
        assert!(
            message.contains(&record),
            "LOW7 {case} diagnostic omitted stable marker {record:?}: {message}"
        );
    }

    let after = db
        .schema_text()
        .await
        .expect("LOW7 schema must export after unavailable struct transitions");
    assert_eq!(
        after, before,
        "LOW7 rejected struct transitions changed committed schema"
    );
}

#[tokio::test]
async fn low8_populated_data_guards_reject_unsafe_schema_transitions() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-low8");
    let abstract_type = format!("{suffix}-abstract-guard");
    let independent_attribute = format!("{suffix}-independent-guard");
    let key_attribute = format!("{suffix}-key-value");
    let key_owner = format!("{suffix}-key-owner");
    let unique_attribute = format!("{suffix}-unique-value");
    let unique_owner = format!("{suffix}-unique-owner");
    let card_attribute = format!("{suffix}-card-value");
    let card_owner = format!("{suffix}-card-owner");
    let regex_attribute = format!("{suffix}-regex-value");
    let range_attribute = format!("{suffix}-range-value");
    let values_attribute = format!("{suffix}-values-value");
    let constraint_owner = format!("{suffix}-constraint-owner");
    let sub_attribute = format!("{suffix}-sub-value");
    let parent_a = format!("{suffix}-parent-a");
    let parent_b = format!("{suffix}-parent-b");
    let child = format!("{suffix}-child");
    let parent_relation = format!("{suffix}-parent-link");
    let wide_role = format!("{suffix}-wide-role");
    let narrow_role = format!("{suffix}-narrow-role");
    let child_relation = format!("{suffix}-child-link");
    let child_role = format!("{suffix}-child-role");
    let role_player = format!("{suffix}-role-player");
    let typed_attribute = format!("{suffix}-typed-value");
    let typed_owner = format!("{suffix}-typed-owner");

    db.execute_raw(
        &format!(
            "define\n\
             entity {abstract_type};\n\
             attribute {independent_attribute} @independent, value string;\n\
             attribute {key_attribute}, value string;\n\
             entity {key_owner}, owns {key_attribute};\n\
             attribute {unique_attribute}, value string;\n\
             entity {unique_owner}, owns {unique_attribute};\n\
             attribute {card_attribute}, value string;\n\
             entity {card_owner}, owns {card_attribute} @card(0..2);\n\
             attribute {regex_attribute}, value string;\n\
             attribute {range_attribute}, value integer;\n\
             attribute {values_attribute}, value integer;\n\
             entity {constraint_owner}, \
               owns {regex_attribute}, owns {range_attribute}, owns {values_attribute};\n\
             attribute {sub_attribute}, value string;\n\
             entity {parent_a}, owns {sub_attribute};\n\
             entity {parent_b};\n\
             entity {child} sub {parent_a};\n\
             relation {parent_relation} @abstract, \
               relates {wide_role} @card(0..2), \
               relates {narrow_role} @card(0..1);\n\
             relation {child_relation} sub {parent_relation}, \
               relates {child_role} as {wide_role} @card(0..2);\n\
             entity {role_player}, plays {child_relation}:{child_role};\n\
             attribute {typed_attribute}, value string;\n\
             entity {typed_owner}, owns {typed_attribute};"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW8 guard fixture schema must define");

    db.execute_raw(
        &format!(
            "insert\n\
             $abstract isa {abstract_type};\n\
             $orphan isa {independent_attribute} \"orphan\";\n\
             $key_owner isa {key_owner};\n\
             $unique_left isa {unique_owner}, has {unique_attribute} \"duplicate\";\n\
             $unique_right isa {unique_owner}, has {unique_attribute} \"duplicate\";\n\
             $card_owner isa {card_owner}, \
               has {card_attribute} \"one\", has {card_attribute} \"two\";\n\
             $constraint_owner isa {constraint_owner}, \
               has {regex_attribute} \"bad\", \
               has {range_attribute} 5, \
               has {values_attribute} 5;\n\
             $child isa {child}, has {sub_attribute} \"inherited\";\n\
             $player_a isa {role_player};\n\
             $player_b isa {role_player};\n\
             ({child_role}: $player_a, {child_role}: $player_b) isa {child_relation};\n\
             $typed_owner isa {typed_owner}, has {typed_attribute} \"text\";"
        ),
        TxType::Write,
    )
    .await
    .expect("LOW8 guard fixture data must commit before schema transitions");

    db.execute_raw(
        &format!("undefine\n@independent from {independent_attribute};"),
        TxType::Schema,
    )
    .await
    .expect("LOW8 independent removal must commit on TypeDB 3.12.1");
    let orphan_probe = db
        .execute_raw(
            &format!("match\n$orphan isa {independent_attribute} \"orphan\";\nselect $orphan;"),
            TxType::Read,
        )
        .await
        .expect("LOW8 exact orphan probe must execute after independent removal");
    let QueryResult::Rows(orphan_rows) = orphan_probe else {
        panic!("LOW8 exact orphan probe returned a non-row result: {orphan_probe:?}")
    };
    assert!(
        orphan_rows.is_empty(),
        "LOW8 TypeDB 3.12.1 must delete the exact ownerless attribute when @independent is removed: {orphan_rows:?}"
    );

    let before = db
        .schema_text()
        .await
        .expect("LOW8 fixture schema must export before guarded transitions");
    let transitions = [
        (
            "abstract add",
            abstract_type.as_str(),
            format!("define\nentity {abstract_type} @abstract;"),
        ),
        (
            "key add",
            key_owner.as_str(),
            format!("define\nentity {key_owner}, owns {key_attribute} @key;"),
        ),
        (
            "unique add",
            unique_owner.as_str(),
            format!("define\nentity {unique_owner}, owns {unique_attribute} @unique;"),
        ),
        (
            "cardinality narrowing",
            card_owner.as_str(),
            format!("redefine\n{card_owner} owns {card_attribute} @card(0..1);"),
        ),
        (
            "regex narrowing",
            regex_attribute.as_str(),
            format!("define\n{regex_attribute} value string @regex(\"^good$\");"),
        ),
        (
            "range narrowing",
            range_attribute.as_str(),
            format!("define\n{range_attribute} value integer @range(10..20);"),
        ),
        (
            "values narrowing",
            values_attribute.as_str(),
            format!("define\n{values_attribute} value integer @values(1, 2);"),
        ),
        (
            "sub change",
            child.as_str(),
            format!("redefine\n{child} sub {parent_b};"),
        ),
        (
            "relates specialization change",
            child_relation.as_str(),
            format!("redefine\n{child_relation} relates {child_role} as {narrow_role};"),
        ),
        (
            "value-type change",
            typed_attribute.as_str(),
            format!("redefine\n{typed_attribute} value integer;"),
        ),
    ];

    for (case, marker, query) in transitions {
        let error = db
            .execute_raw(&query, TxType::Schema)
            .await
            .expect_err(&format!(
                "LOW8 {case} must be rejected against populated data: {query}"
            ));
        let message = error.to_string();
        assert!(
            !message.is_empty(),
            "LOW8 {case} returned an empty diagnostic"
        );
        assert!(
            message.contains(marker),
            "LOW8 {case} diagnostic omitted stable marker {marker:?}: {message}"
        );
    }

    let after = db
        .schema_text()
        .await
        .expect("LOW8 schema must export after guarded transition failures");
    assert_eq!(
        after, before,
        "LOW8 rejected data-guard transitions changed committed schema"
    );
}

#[tokio::test]
async fn low9_cardinality_removal_rejects_implicit_default_narrowing() {
    let _guard = crate::common::integration_test_guard().await;
    let db = setup_db().await;
    let suffix = unique_schema_suffix("rust", "schema-v2-low9");
    let attribute = format!("{suffix}-value");
    let owner = format!("{suffix}-owner");
    let relation = format!("{suffix}-link");
    let role = format!("{suffix}-role");
    let player = format!("{suffix}-player");

    db.execute_raw(
        &format!(
            "define\n\
             attribute {attribute}, value string;\n\
             entity {owner}, owns {attribute} @card(0..2);\n\
             relation {relation}, relates {role} @card(0..2);\n\
             entity {player}, plays {relation}:{role};"
        ),
        TxType::Schema,
    )
    .await
    .expect("LOW9 explicit-cardinality fixture schema must define");
    db.execute_raw(
        &format!(
            "insert\n\
             $owner isa {owner}, \
               has {attribute} \"one\", has {attribute} \"two\";\n\
             $player_a isa {player};\n\
             $player_b isa {player};\n\
             ({role}: $player_a, {role}: $player_b) isa {relation};"
        ),
        TxType::Write,
    )
    .await
    .expect("LOW9 two-value/two-player fixture data must commit under @card(0..2)");

    let before = db
        .schema_text()
        .await
        .expect("LOW9 fixture schema must export before removals");
    for (case, marker, query) in [
        (
            "owns",
            owner.as_str(),
            format!("undefine\n@card from {owner} owns {attribute};"),
        ),
        (
            "relates",
            relation.as_str(),
            format!("undefine\n@card from {relation} relates {role};"),
        ),
    ] {
        let error = db
            .execute_raw(&query, TxType::Schema)
            .await
            .expect_err(&format!(
                "LOW9 removing explicit {case} cardinality must reject the implicit 0..1 default"
            ));
        let message = error.to_string();
        assert!(
            !message.is_empty(),
            "LOW9 {case} returned an empty diagnostic"
        );
        assert!(
            message.contains(marker) && message.contains("@card(0..1)"),
            "LOW9 {case} rejection omitted marker/default evidence: {message}"
        );
    }

    let after = db
        .schema_text()
        .await
        .expect("LOW9 schema must export after rejected cardinality removals");
    assert_eq!(
        after, before,
        "LOW9 rejected default-narrowing removals changed committed schema"
    );
}
