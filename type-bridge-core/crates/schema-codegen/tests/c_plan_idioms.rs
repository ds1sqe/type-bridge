//! Generated nominal plan wrappers compile as C17 and C++17.
use std::fs;
use std::path::Path;
use std::process::Command;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{BindingTarget, CSymbolPrefix, ProjectionConfig};
use type_bridge_contract::schema::DocumentId;
use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
use type_bridge_schema_codegen::{CEmitter, GeneratedPackage};
mod support;
use support::{Stage, runtime_include, write_package};

fn package(source: &str, config: ProjectionConfig) -> Result<GeneratedPackage, Diagnostic> {
    let documents =
        SchemaDocumentSet::parse([(DocumentId::new("plan.yaml").unwrap(), source)]).unwrap();
    let declared = normalize_documents(&documents).unwrap();
    let resolved = resolve(
        &declared,
        &type_bridge_contract::fingerprint::SemanticProfileId::new(support::TEST_PROFILE).unwrap(),
    )
    .unwrap();
    let emitter = CEmitter::new();
    let projection = project(
        &resolved,
        BindingTarget::C,
        &config,
        &emitter.generator_handlers_for(&resolved),
        &emitter.code_resources_for(&resolved).unwrap(),
    )
    .unwrap();
    emitter.emit(&projection, &support::authority(source))
}

#[test]
fn nominal_match_field_and_set_wrappers_compile_in_both_languages() {
    let generated = package(
        include_str!("acceptance/schema-plan-idioms.yaml"),
        ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
    )
    .unwrap();
    let header = std::str::from_utf8(generated.get("include/acme/models.h").unwrap()).unwrap();
    assert!(header.contains("acme_item_insert_t;"));
    assert!(header.contains("acme_purchase_insert_t;"));
    assert!(!header.contains("acme_many_insert_t;"));
    assert!(!header.contains("acme_manyzhitem_insert_t;"));
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let consumer = stage.path().join("consumer.c");
    fs::write(
        &consumer,
        r#"
#include <acme/models.h>
void author(type_bridge_plan_builder_t *p) {
  acme_item_handle_t item;
  acme_account_handle_t buyer;
  acme_purchase_handle_t purchase;
  acme_purchase_link_t players;
  acme_purchase_insert_t created;
  memset(&created, 0, sizeof(created));
  memset(&players, 0, sizeof(players));
  acme_item_match_t named;
  acme_item_set_t updated;
  acme_item_unset_t removed;
  memset(&named, 0, sizeof(named));
  memset(&updated, 0, sizeof(updated));
  memset(&removed, 0, sizeof(removed));
  removed.stock = true;
  (void)acme_item_match(p, "I1", &item);
  (void)acme_account_match(p, "A1", &buyer);
  (void)acme_purchase_subtypes(p, &purchase);
  players.buyer = buyer;
  created.buyer = buyer;
  created.orderzhid = "O1";
  (void)created;
  (void)acme_purchase_link(p, purchase, &players);
  (void)acme_purchase_unlink(p, purchase, &players);
  (void)acme_item_where(p, item, tb_long_gt(acme_item_plan_stock(p, item), tb_long(0)));
  updated.stock = tb_long_sub(acme_item_plan_stock(p, item), tb_long(1));
  (void)acme_item_set(p, item, &updated);
  (void)acme_item_unset(p, item, &removed);
  { acme_item_plan_t held; type_bridge_matched_plan_t deleted;
    (void)acme_item_row(p, item, &held);
    (void)acme_item_insert_many(p, NULL, 0u, &held);
    (void)acme_item_put_many(p, NULL, 0u, &held);
    (void)acme_item_delete(p, item, &deleted);
    (void)acme_item_delete_returning(p, item, &held);
  }
  { acme_purchase_plan_t held;
    (void)acme_purchase_insert(p, &created, &held);
    (void)acme_purchase_put(p, &created, &held);
  }
  named.stock = tb_long(0);
  (void)acme_item_match_by(p, &named, &item);
  (void)acme_item_subtypes(p, &item);
  (void)acme_item_subtypes_by(p, &named, &item);
  (void)acme_item_by_iid(p, "0x1234", &item);
}
"#,
    )
    .unwrap();
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let result = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(&consumer)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn single_player_role_inputs_reject_a_different_nominal_model() {
    let generated = package(
        include_str!("acceptance/schema-plan-idioms.yaml"),
        ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
    )
    .unwrap();
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let consumer = stage.path().join("wrong-player.c");
    fs::write(&consumer, "#include <acme/models.h>\nvoid wrong(acme_item_handle_t item) { acme_purchase_link_t links; memset(&links, 0, sizeof(links)); links.buyer = item; (void)links; }\n").unwrap();
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(&consumer)
            .output()
            .unwrap();
        assert!(
            !output.status.success(),
            "{compiler} admitted an item in the account-only buyer role"
        );
    }
}

#[test]
fn encoded_fields_and_helper_members_remain_distinct() {
    let source = "format: typebridge.schema/v2\nattributes:\n  stock: {value: integer}\n  plan-stock: {value: integer}\n  plan_stock: {value: integer}\n  has-stock: {value: integer}\nentities:\n  item:\n    owns:\n      stock: {card: {min: 0, max: 1}}\n      plan-stock: {card: 1}\n      plan_stock: {card: 1}\n      has-stock: {card: 1}\n";
    let generated = package(
        source,
        ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
    )
    .unwrap();
    let header = std::str::from_utf8(generated.get("include/acme/models.h").unwrap()).unwrap();
    for symbol in [
        "acme_item_plan_stock",
        "acme_item_planzhstock",
        "acme_item_planzustock",
        "has_stock",
        "haszhstock",
    ] {
        assert!(header.contains(symbol), "missing {symbol}");
    }
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let source = stage.path().join("names.c");
    fs::write(&source, "#include <acme/models.h>\nvoid named(void) { acme_item_values_t row; memset(&row, 0, sizeof(row)); row.stock = 0; row.has_stock = true; row.haszhstock = 1; row.planzhstock = 2; row.planzustock = 3; (void)row; }\n").unwrap();
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn plan_helpers_refuse_real_type_override_collisions() {
    use type_bridge_contract::id::{TypeId, TypeKind};
    let source = "format: typebridge.schema/v2\nattributes:\n  stock: {value: integer}\nentities:\n  item: {}\n  other:\n    owns:\n      stock: {card: 1}\n";
    let config = ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap())
        .with_type_name_override(
            TypeId::new(TypeKind::Entity, "item").unwrap(),
            "acme_other_plan_stock",
        )
        .unwrap();
    let error = match package(source, config) {
        Err(error) => error,
        Ok(_) => panic!("actual C symbol collision must refuse"),
    };
    assert_eq!(
        error.code().as_str(),
        "c_emitter_generated_symbol_collision"
    );
}

#[test]
fn authoring_keyword_members_are_lossless_in_c_and_cpp() {
    let source = "format: typebridge.schema/v2\nattributes:\n  class: {value: integer}\n  int: {value: integer}\n  operator: {value: integer}\n  classz: {value: integer}\nentities:\n  item:\n    owns:\n      class: {card: 1}\n      int: {card: 1}\n      operator: {card: 1}\n      classz: {card: 1}\n";
    let generated = package(
        source,
        ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
    )
    .unwrap();
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let consumer = stage.path().join("keywords.c");
    fs::write(&consumer, "#include <acme/models.h>\nvoid keywords(type_bridge_plan_builder_t *p, acme_item_handle_t h) { acme_item_values_t v; acme_item_set_t s; memset(&v, 0, sizeof(v)); memset(&s, 0, sizeof(s)); v.classzk = 1; v.intzk = 2; v.operatorzk = 3; v.classzz = 4; s.classzk = tb_long(1); (void)acme_item_set(p, h, &s); (void)acme_item_plan_class(p, h); (void)v; }\n").unwrap();
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(&consumer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn runtime_short_aliases_refuse_only_actual_generated_collisions() {
    use type_bridge_contract::id::{TypeId, TypeKind};
    let source = "format: typebridge.schema/v2\nentities:\n  item: {}\n";
    let prefix = || ProjectionConfig::c(CSymbolPrefix::new("tb").unwrap());
    assert!(package(source, prefix()).is_ok());
    for name in [
        "tb_long",
        "tb_long_t",
        "tb_cond",
        "tb_string_contains",
        "tb_string_starts_with",
        "tb_string_ends_with",
        "tb_string_like",
    ] {
        let config = prefix()
            .with_type_name_override(TypeId::new(TypeKind::Entity, "item").unwrap(), name)
            .unwrap();
        let error = match package(source, config) {
            Err(error) => error,
            Ok(_) => panic!("runtime identifier {name} must not be shadowed"),
        };
        assert_eq!(
            error.code().as_str(),
            "c_emitter_generated_symbol_collision"
        );
    }
}

#[test]
fn wide_model_handles_have_the_same_pinned_layout() {
    let mut source = String::from("format: typebridge.schema/v2\nattributes:\n");
    for index in 0..140 {
        source.push_str(&format!("  a{index}: {{value: integer}}\n"));
    }
    source.push_str("entities:\n  item:\n    owns:\n");
    for index in 0..140 {
        source.push_str(&format!("      a{index}: {{card: 1}}\n"));
    }
    let generated = package(
        &source,
        ProjectionConfig::c(CSymbolPrefix::new("wide").unwrap()),
    )
    .unwrap();
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let consumer = stage.path().join("layout.c");
    fs::write(&consumer, "#include <wide/models.h>\n").unwrap();
    let header = std::str::from_utf8(generated.get("include/wide/models.h").unwrap()).unwrap();
    assert!(header.contains("uint32_t model_ordinal; } wide_item_handle_t;"));
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let output = Command::new(compiler)
            .args([
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .args(["-x", language])
            .arg(&consumer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn ordered_ownerships_and_roles_omit_unsupported_authoring_constructors() {
    for (source, forbidden) in [
        (
            include_str!("acceptance/schema-plan-ordered-roles.yaml"),
            &[
                "acme_team_insert_t",
                "acme_team_link_t",
                "acme_team_unlink_t",
            ][..],
        ),
        (
            include_str!("acceptance/schema-ordered.yaml"),
            &[
                "acme_person_values_t",
                "acme_person_insert_t",
                "acme_person_plan_bind_rows",
                "acme_membership_insert_t",
                "acme_membership_link_t",
            ][..],
        ),
    ] {
        let generated = package(
            source,
            ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
        )
        .unwrap();
        let header = std::str::from_utf8(generated.get("include/acme/models.h").unwrap()).unwrap();
        for symbol in forbidden {
            assert!(
                !header.contains(symbol),
                "unsupported ordered authoring helper: {symbol}"
            );
        }
    }
}

#[test]
fn root_and_player_view_names_participate_in_the_clash_check() {
    use type_bridge_contract::id::{TypeId, TypeKind};
    let source = "format: typebridge.schema/v2\nentities:\n  item: {}\n  other: {}\n";
    for suffix in [
        "plan_view_t",
        "plan_player_view_t",
        "plan_exact_view_t",
        "plan_player_exact_view_t",
    ] {
        let config = ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap())
            .with_type_name_override(
                TypeId::new(TypeKind::Entity, "other").unwrap(),
                format!("acme_item_{suffix}"),
            )
            .unwrap();
        let error = match package(source, config) {
            Err(error) => error,
            Ok(_) => panic!("view type collision must refuse"),
        };
        assert_eq!(
            error.code().as_str(),
            "c_emitter_generated_symbol_collision"
        );
    }
}

#[test]
fn cyclic_relation_player_views_are_nonrecursive_and_compile_in_c_and_cpp() {
    let source = "format: typebridge.schema/v2\nattributes:\n  id: { value: string }\nrelations:\n  node:\n    owns:\n      id: { key: true }\n    relates:\n      next: { card: { min: 0, max: 3 } }\nplays:\n  node:\n    node: [next]\n";
    let generated = package(
        source,
        ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
    )
    .unwrap();
    let header = std::str::from_utf8(generated.get("include/acme/models.h").unwrap()).unwrap();
    assert!(header.contains("acme_node_plan_player_view_t"));
    assert!(header.contains("const acme_node_plan_next_player_view_t * next;"));
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let consumer = stage.path().join("cyclic.c");
    fs::write(&consumer, "#include <acme/models.h>\nvoid inspect(const acme_node_plan_view_t *node) { size_t i; for (i = 0u; i < node->next_count; ++i) { const acme_node_plan_player_view_t *player = &node->next[i]; const char *id = player->id; bool present = player->has_id; (void)id; (void)present; (void)player->iid; (void)player->deleted; } }\n").unwrap();
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let output = Command::new(compiler)
            .args([
                "-x",
                language,
                &format!("-std={standard}"),
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fsyntax-only",
                "-I",
            ])
            .arg(runtime_include())
            .arg("-I")
            .arg(stage.path().join("include"))
            .arg(&consumer)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn cross_binding_comparisons_check_the_scalar_type() {
    let generated = package(
        include_str!("acceptance/schema-plan-idioms.yaml"),
        ProjectionConfig::c(CSymbolPrefix::new("acme").unwrap()),
    )
    .unwrap();
    let stage = Stage::new();
    write_package(&generated, stage.path());
    let header = "#include <acme/models.h>\n";
    let positive = stage.path().join("price-left.c");
    fs::write(&positive, format!("{header}tb_cond_t ok(type_bridge_plan_builder_t *p, acme_item_handle_t item, acme_account_handle_t buyer) {{ (void)acme_item_plan_itemzhid(p, item); return tb_long_le(acme_item_plan_price(p, item), acme_account_plan_balance(p, buyer)); }}\n")).unwrap();
    let negative = stage.path().join("wrong-scalar.c");
    fs::write(&negative, format!("{header}tb_cond_t wrong(type_bridge_plan_builder_t *p, acme_item_handle_t item, acme_account_handle_t buyer) {{ return tb_long_le(acme_item_plan_itemzhid(p, item), acme_account_plan_balance(p, buyer)); }}\n")).unwrap();
    for (compiler, language, standard) in [("cc", "c", "c17"), ("c++", "c++", "c++17")] {
        let check = |source: &Path| {
            Command::new(compiler)
                .args([
                    "-x",
                    language,
                    &format!("-std={standard}"),
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-fsyntax-only",
                    "-I",
                ])
                .arg(runtime_include())
                .arg("-I")
                .arg(stage.path().join("include"))
                .arg(source)
                .output()
                .unwrap()
        };
        let accepted = check(&positive);
        assert!(
            accepted.status.success(),
            "{compiler}: {}",
            String::from_utf8_lossy(&accepted.stderr)
        );
        let refused = check(&negative);
        assert!(
            !refused.status.success(),
            "{compiler} compared a string field with an integer field"
        );
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("tb_long_le"),
            "{compiler}: {}",
            String::from_utf8_lossy(&refused.stderr)
        );
    }
}
