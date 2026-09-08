use std::collections::BTreeSet;
use std::fmt::Write as _;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{ModelProjection, RuntimeProjection};
use type_bridge_contract::value::ValueTypeTag;

use super::{
    bound_long_generated_identifiers, field_token_symbol, generated_symbol, model_token_ordinal,
    model_token_symbol, projected_attribute_model, projected_member_suffix, role_token_symbol,
};

mod function;
mod manager;
mod reduction;

const QUERY_MODES: [(&str, &str); 2] = [
    ("exact", "TYPE_BRIDGE_QUERY_MATCH_EXACT"),
    ("subtypes", "TYPE_BRIDGE_QUERY_MATCH_SUBTYPES"),
];

const SELECTION_KINDS: [(&str, &str); 2] = [
    ("one", "TYPE_BRIDGE_QUERY_SELECTION_ONE"),
    ("collect", "TYPE_BRIDGE_QUERY_SELECTION_COLLECT"),
];

const REMOTE_FAMILIES: [(&str, &str, &str); 5] = [
    (
        "rows",
        "TYPE_BRIDGE_QUERY_TERMINAL_ROWS",
        "query_rows_result",
    ),
    (
        "first",
        "TYPE_BRIDGE_QUERY_TERMINAL_FIRST",
        "query_rows_result",
    ),
    (
        "page",
        "TYPE_BRIDGE_QUERY_TERMINAL_PAGE",
        "query_page_result",
    ),
    (
        "count",
        "TYPE_BRIDGE_QUERY_TERMINAL_COUNT",
        "query_count_result",
    ),
    (
        "exists",
        "TYPE_BRIDGE_QUERY_TERMINAL_EXISTS",
        "query_exists_result",
    ),
];

fn query_models(projection: &RuntimeProjection) -> impl Iterator<Item = &ModelProjection> {
    projection.models().values().filter(|model| {
        matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation)
            && model.query_tokens().target_name().is_some()
    })
}

fn package_name(prefix: &str, suffix: &str) -> Result<String, Diagnostic> {
    generated_symbol(prefix, suffix)
}

fn model_name(model: &ModelProjection, suffix: &str) -> Result<String, Diagnostic> {
    generated_symbol(model.target_name().as_str(), suffix)
}

fn field_base(
    model: &ModelProjection,
    prefix: &str,
    field: &type_bridge_contract::projection::FieldTokenProjection,
) -> Result<String, Diagnostic> {
    generated_symbol(
        model.target_name().as_str(),
        &format!(
            "{}_query_field",
            projected_member_suffix(prefix, field.target_name())?
        ),
    )
}

fn scalar_suffix(domain: ValueTypeTag) -> &'static str {
    match domain {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "integer",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime_tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    }
}

fn field_domain(
    projection: &RuntimeProjection,
    field: &type_bridge_contract::projection::FieldTokenProjection,
) -> Result<ValueTypeTag, Diagnostic> {
    let attribute = projection
        .models()
        .get(&super::attribute_type_id(field.id())?)
        .ok_or_else(|| {
            super::invalid(
                "c_emitter_query_field_attribute_missing",
                "projected query field attribute is absent",
            )
        })?;
    attribute.declaration().value_type().ok_or_else(|| {
        super::invalid(
            "c_emitter_query_field_domain_missing",
            "projected query field has no scalar domain",
        )
    })
}

fn field_domains(projection: &RuntimeProjection) -> Result<BTreeSet<ValueTypeTag>, Diagnostic> {
    query_models(projection)
        .flat_map(|model| model.query_tokens().fields().values())
        .map(|field| field_domain(projection, field))
        .collect()
}

fn domain_field_ref_name(prefix: &str, domain: ValueTypeTag) -> Result<String, Diagnostic> {
    package_name(
        prefix,
        &format!("query_{}_field_ref_v1_t", scalar_suffix(domain)),
    )
}

fn role_base(
    model: &ModelProjection,
    prefix: &str,
    role: &type_bridge_contract::projection::RoleTokenProjection,
) -> Result<String, Diagnostic> {
    generated_symbol(
        model.target_name().as_str(),
        &format!(
            "{}_query_role",
            projected_member_suffix(prefix, role.target_name())?
        ),
    )
}

fn slot_view_name(model: &ModelProjection, mode: &str, kind: &str) -> Result<String, Diagnostic> {
    model_name(model, &format!("query_{mode}_{kind}_result_slot_v1_t"))
}

fn role_player_binding_name(base: &str) -> Result<String, Diagnostic> {
    generated_symbol(base, "player_binding_v1_t")
}

fn role_player_binding_constructor(
    base: &str,
    prefix: &str,
    player: &ModelProjection,
    mode: &str,
) -> Result<String, Diagnostic> {
    generated_symbol(
        base,
        &format!(
            "player_{}_{}",
            projected_member_suffix(prefix, player.target_name())?,
            mode
        ),
    )
}

fn reachable_endpoint_name(relation: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(relation, "query_reachable_endpoint_v1_t")
}

fn reachable_endpoint_constructor(
    relation: &ModelProjection,
    prefix: &str,
    role: &type_bridge_contract::projection::RoleTokenProjection,
) -> Result<String, Diagnostic> {
    generated_symbol(
        &reachable_endpoint_name(relation)?,
        &format!(
            "from_{}",
            projected_member_suffix(prefix, role.target_name())?
        ),
    )
}

fn reachable_name(relation: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(relation, "query_reachable")
}

fn has_reachable_endpoint(relation: &ModelProjection) -> bool {
    relation
        .query_tokens()
        .roles()
        .values()
        .any(|role| !role.accepted_players().is_empty())
}

fn package_opaque_suffixes() -> [&'static str; 13] {
    [
        "query_session",
        "query_predicate",
        "query_order",
        "query",
        "query_rows_terminal",
        "query_first_terminal",
        "query_page_terminal",
        "query_count_terminal",
        "query_exists_terminal",
        "query_rows_result",
        "query_page_result",
        "query_count_result",
        "query_exists_result",
    ]
}

fn remote_nominal_suffixes() -> impl Iterator<Item = String> {
    std::iter::once("query_remote_context".to_owned()).chain(
        REMOTE_FAMILIES
            .into_iter()
            .map(|(family, _, _)| family)
            .chain(reduction::remote_families().map(|(family, _, _)| family))
            .flat_map(|family| {
                [
                    format!("query_{family}_remote_pending"),
                    format!("query_{family}_remote_claim"),
                ]
            }),
    )
}

pub(super) fn nominal_names(
    projection: &RuntimeProjection,
    prefix: &str,
    ordered: bool,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for suffix in package_opaque_suffixes() {
        names.insert(package_name(prefix, suffix)?);
    }
    for suffix in remote_nominal_suffixes() {
        names.insert(package_name(prefix, &suffix)?);
    }
    for model in query_models(projection) {
        for suffix in [
            "query_exact_binding",
            "query_subtypes_binding",
            "query_exact_one_selection",
            "query_exact_collect_selection",
            "query_subtypes_one_selection",
            "query_subtypes_collect_selection",
            "query_subtypes_result",
        ] {
            names.insert(model_name(model, suffix)?);
        }
        for (mode, _) in QUERY_MODES {
            for (kind, _) in SELECTION_KINDS {
                names.insert(slot_view_name(model, mode, kind)?);
            }
        }
        for field in model.query_tokens().fields().values() {
            names.insert(field_base(model, prefix, field)?);
        }
        for role in model.query_tokens().roles().values() {
            let base = role_base(model, prefix, role)?;
            names.insert(role_player_binding_name(&base)?);
            names.insert(base);
        }
        if model.id().kind() == TypeKind::Relation && has_reachable_endpoint(model) {
            names.insert(reachable_endpoint_name(model)?);
        }
    }
    names.extend(reduction::nominal_names(projection, prefix)?);
    names.extend(function::nominal_names(projection, prefix)?);
    if ordered {
        names.extend(manager::nominal_names(projection, prefix)?);
    }
    Ok(names)
}

fn bridge_type_names(prefix: &str) -> Result<[String; 6], Diagnostic> {
    Ok([
        package_name(prefix, "query_binding_ref_v1_t")?,
        package_name(prefix, "query_field_ref_v1_t")?,
        package_name(prefix, "query_role_ref_v1_t")?,
        package_name(prefix, "query_selection_ref_v1_t")?,
        package_name(prefix, "query_root_v1_t")?,
        package_name(prefix, "query_row_result_ref_v1_t")?,
    ])
}

pub(super) fn auxiliary_type_names(
    projection: &RuntimeProjection,
    prefix: &str,
    ordered: bool,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::from(bridge_type_names(prefix)?);
    for model in query_models(projection) {
        let union = model_name(model, "query_subtypes_result")?;
        names.insert(generated_symbol(&union, "kind_t")?);
    }
    for domain in field_domains(projection)? {
        names.insert(domain_field_ref_name(prefix, domain)?);
    }
    names.extend(reduction::auxiliary_type_names(prefix)?);
    names.extend(function::auxiliary_names(projection, prefix)?);
    if ordered {
        names.extend(manager::auxiliary_type_names(projection)?);
    }
    Ok(names)
}

fn concrete_descendants<'a>(
    projection: &'a RuntimeProjection,
    base: &TypeId,
) -> Vec<&'a ModelProjection> {
    query_models(projection)
        .filter(|candidate| {
            if candidate.id().kind() != base.kind() || candidate.declaration().is_abstract() {
                return false;
            }
            let mut current = Some(candidate.id());
            while let Some(id) = current {
                if id == base {
                    return true;
                }
                current = projection
                    .models()
                    .get(id)
                    .and_then(|model| model.declaration().parent());
            }
            false
        })
        .collect()
}

pub(super) fn macro_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for model in query_models(projection) {
        let union = model_name(model, "query_subtypes_result")?;
        names.insert(generated_symbol(&union, "kind_unknown")?);
        for descendant in concrete_descendants(projection, model.id()) {
            let member = projected_member_suffix(prefix, descendant.target_name())?;
            names.insert(generated_symbol(&union, &format!("kind_{member}"))?);
        }
    }
    Ok(names)
}

pub(super) fn function_names(
    projection: &RuntimeProjection,
    prefix: &str,
    ordered: bool,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for suffix in [
        "query_session_open",
        "query_session_close",
        "query_predicate_and",
        "query_predicate_or",
        "query_predicate_not",
        "query_predicate_close",
        "query_order_close",
        "query_where",
        "query_add_hidden",
        "query_allow_cross_join",
        "query_close",
        "query_one",
        "query_first",
        "query_rows",
        "query_page",
        "query_count",
        "query_exists",
        "query_rows_terminal_close",
        "query_first_terminal_close",
        "query_page_terminal_close",
        "query_count_terminal_close",
        "query_exists_terminal_close",
        "database_query_execute_rows",
        "database_query_execute_first",
        "database_query_execute_page",
        "database_query_execute_count",
        "database_query_execute_exists",
        "read_transaction_query_execute_rows",
        "read_transaction_query_execute_first",
        "read_transaction_query_execute_page",
        "read_transaction_query_execute_count",
        "read_transaction_query_execute_exists",
        "query_rows_result_ref",
        "query_page_result_ref",
        "query_result_row_count",
        "query_page_metadata",
        "query_count_result_value",
        "query_exists_result_value",
        "query_rows_result_close",
        "query_page_result_close",
        "query_count_result_close",
        "query_exists_result_close",
        "query_remote_context_open",
        "query_remote_context_close",
    ] {
        names.insert(package_name(prefix, suffix)?);
    }
    for domain in field_domains(projection)? {
        names.insert(package_name(
            prefix,
            &format!("query_{}_field_compare_field", scalar_suffix(domain)),
        )?);
    }
    for (family, _, _) in REMOTE_FAMILIES
        .into_iter()
        .chain(reduction::remote_families())
    {
        for suffix in [
            format!("query_remote_prepare_{family}"),
            format!("query_{family}_remote_pending_request_bytes"),
            format!("query_{family}_remote_pending_response_snapshot_limit"),
            format!("query_{family}_remote_pending_claim"),
            format!("query_{family}_remote_pending_close"),
            format!("query_{family}_remote_claim_decode"),
            format!("query_{family}_remote_claim_close"),
        ] {
            names.insert(package_name(prefix, &suffix)?);
        }
    }
    for arity in 1..=16 {
        names.insert(package_name(prefix, &format!("query_positional_{arity}"))?);
        names.insert(package_name(prefix, &format!("query_named_{arity}"))?);
    }
    for model in query_models(projection) {
        for (mode, _) in QUERY_MODES {
            let binding = model_name(model, &format!("query_{mode}_binding"))?;
            for suffix in ["open", "ref", "root", "iid", "iid_in", "close"] {
                names.insert(generated_symbol(&binding, suffix)?);
            }
            for (kind, _) in SELECTION_KINDS {
                let selection = model_name(model, &format!("query_{mode}_{kind}_selection"))?;
                for suffix in ["open", "ref", "close"] {
                    names.insert(generated_symbol(&selection, suffix)?);
                }
            }
            let result_base = model_name(model, &format!("query_{mode}"))?;
            for kind in ["one", "collect"] {
                let view = slot_view_name(model, mode, kind)?;
                for source in ["rows", "page"] {
                    names.insert(generated_symbol(&view, source)?);
                }
            }
            for suffix in ["one_at", "collect_count", "collect_at"] {
                names.insert(generated_symbol(&result_base, suffix)?);
            }
        }
        for field in model.query_tokens().fields().values() {
            let base = field_base(model, prefix, field)?;
            for suffix in [
                "from_exact",
                "from_subtypes",
                "ref",
                "compare_value",
                "presence",
                "order",
                "close",
            ] {
                names.insert(generated_symbol(&base, suffix)?);
            }
            names.insert(generated_symbol(
                &base,
                &format!(
                    "{}_field_ref",
                    scalar_suffix(field_domain(projection, field)?)
                ),
            )?);
        }
        for role in model.query_tokens().roles().values() {
            let base = role_base(model, prefix, role)?;
            for suffix in ["from_exact", "from_subtypes", "ref", "close"] {
                names.insert(generated_symbol(&base, suffix)?);
            }
            if !role.accepted_players().is_empty() {
                names.insert(generated_symbol(&base, "connects")?);
            }
            for player in role.accepted_players() {
                let player_model = projection
                    .models()
                    .get(player)
                    .expect("projected query role player exists");
                for (mode, _) in QUERY_MODES {
                    names.insert(role_player_binding_constructor(
                        &base,
                        prefix,
                        player_model,
                        mode,
                    )?);
                }
            }
        }
        if model.id().kind() == TypeKind::Relation && has_reachable_endpoint(model) {
            names.insert(reachable_name(model)?);
            for role in model.query_tokens().roles().values() {
                if !role.accepted_players().is_empty() {
                    names.insert(reachable_endpoint_constructor(model, prefix, role)?);
                }
            }
        }
        let union = model_name(model, "query_subtypes_result")?;
        for suffix in ["kind", "close"] {
            names.insert(generated_symbol(&union, suffix)?);
        }
        for descendant in concrete_descendants(projection, model.id()) {
            let member = projected_member_suffix(prefix, descendant.target_name())?;
            names.insert(generated_symbol(&union, &format!("as_{member}"))?);
        }
    }
    names.extend(reduction::function_names(projection, prefix)?);
    names.extend(function::function_names(projection, prefix)?);
    if ordered {
        names.extend(manager::function_names(projection, prefix)?);
    }
    Ok(names)
}

fn render_declarations(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    ordered: bool,
) -> Result<(), Diagnostic> {
    let [
        binding_ref,
        field_ref,
        role_ref,
        selection_ref,
        root_ref,
        row_result_ref,
    ] = bridge_type_names(prefix)?;
    let session = package_name(prefix, "query_session")?;
    let predicate = package_name(prefix, "query_predicate")?;
    let order = package_name(prefix, "query_order")?;
    let query = package_name(prefix, "query")?;

    let _ = write!(
        output,
        "\n/* Header-local generated nominal immutable query facade. Function addresses\n\
         * are translation-unit local and have no cross-TU identity contract.\n\
         * Application output names are copied metadata; result access remains\n\
         * typed and ordinal. */\n\
         typedef struct {binding_ref} {{\n\
           const type_bridge_query_binding_t *handle;\n\
           const type_bridge_projected_token_v1_t *expected_model;\n\
           type_bridge_query_match_mode_t expected_mode;\n\
           uint32_t reserved[4];\n\
         }} {binding_ref};\n\
         typedef struct {field_ref} {{\n\
           const type_bridge_query_field_t *handle;\n\
           const type_bridge_projected_token_v1_t *expected_field;\n\
           uint32_t reserved[4];\n\
         }} {field_ref};\n\
         typedef struct {role_ref} {{\n\
           const type_bridge_query_role_t *handle;\n\
           const type_bridge_projected_token_v1_t *expected_role;\n\
           uint32_t reserved[4];\n\
         }} {role_ref};\n\
         typedef struct {selection_ref} {{\n\
           const type_bridge_query_selection_t *handle;\n\
           const type_bridge_projected_token_v1_t *expected_model;\n\
           type_bridge_query_match_mode_t expected_mode;\n\
           type_bridge_query_selection_kind_t expected_kind;\n\
           uint32_t reserved[4];\n\
         }} {selection_ref};\n\
         typedef struct {root_ref} {{\n\
           const type_bridge_query_t *query;\n\
           const type_bridge_query_binding_t *root;\n\
           const type_bridge_projected_token_v1_t *expected_model;\n\
           type_bridge_query_match_mode_t expected_mode;\n\
           uint32_t reserved[4];\n\
         }} {root_ref};\n\
         typedef struct {row_result_ref} {{\n\
           const type_bridge_query_result_t *handle;\n\
           type_bridge_query_result_kind_t expected_kind;\n\
           uint32_t reserved[4];\n\
         }} {row_result_ref};\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_session_open(\n\
           const type_bridge_schema_package_t *package, {session} **out_session,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_session_close(\n\
           {session} **session);\n",
    );

    for domain in field_domains(projection)? {
        let domain_field = domain_field_ref_name(prefix, domain)?;
        let _ = write!(
            output,
            "typedef struct {domain_field} {{\n\
               const type_bridge_query_field_t *handle;\n\
               const type_bridge_projected_token_v1_t *expected_field;\n\
               uint32_t reserved[4];\n\
             }} {domain_field};\n"
        );
    }

    for model in query_models(projection) {
        render_model_declarations(
            output,
            projection,
            model,
            prefix,
            &session,
            &predicate,
            &order,
            &binding_ref,
            &field_ref,
            &role_ref,
            &selection_ref,
            &root_ref,
            &row_result_ref,
        )?;
    }
    render_package_query_declarations(
        output,
        projection,
        prefix,
        &session,
        &predicate,
        &order,
        &query,
        &binding_ref,
        &field_ref,
        &role_ref,
        &selection_ref,
        &root_ref,
        &row_result_ref,
    )?;
    reduction::render_declarations(output, projection, prefix)?;
    render_remote_declarations(output, prefix)?;
    function::render_declarations(output, projection, prefix)?;
    if ordered {
        manager::render_declarations(output, projection, prefix)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_model_declarations(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
    session: &str,
    predicate: &str,
    order: &str,
    binding_ref: &str,
    field_ref: &str,
    role_ref: &str,
    selection_ref: &str,
    root_ref: &str,
    _row_result_ref: &str,
) -> Result<(), Diagnostic> {
    let target = model.target_name().as_str();
    for (mode, _) in QUERY_MODES {
        let binding = model_name(model, &format!("query_{mode}_binding"))?;
        let open = generated_symbol(&binding, "open")?;
        let to_ref = generated_symbol(&binding, "ref")?;
        let to_root = generated_symbol(&binding, "root")?;
        let iid = generated_symbol(&binding, "iid")?;
        let iid_in = generated_symbol(&binding, "iid_in")?;
        let close = generated_symbol(&binding, "close")?;
        let _ = write!(
            output,
            "\n/* Every call creates a fresh `{target}` {mode} binding occurrence. */\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {open}(\n\
               const {session} *session, {binding} **out_binding,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             {binding_ref} TYPE_BRIDGE_CALL {to_ref}(const {binding} *binding);\n\
             {root_ref} TYPE_BRIDGE_CALL {to_root}(\n\
               const {prefix}_query *query, const {binding} *binding);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {iid}(\n\
               const {binding} *binding, type_bridge_byte_view_t iid,\n\
               {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {iid_in}(\n\
               const {binding} *binding, const type_bridge_byte_view_t *iids,\n\
               size_t iid_count, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {close}({binding} **binding);\n",
        );

        for (kind, _) in SELECTION_KINDS {
            let selection = model_name(model, &format!("query_{mode}_{kind}_selection"))?;
            let selection_open = generated_symbol(&selection, "open")?;
            let selection_to_ref = generated_symbol(&selection, "ref")?;
            let selection_close = generated_symbol(&selection, "close")?;
            if kind == "one" {
                let _ = write!(
                    output,
                    "type_bridge_status_t TYPE_BRIDGE_CALL {selection_open}(\n\
                       const {binding} *binding, {selection} **out_selection,\n\
                       type_bridge_execution_diagnostics_t **out_diagnostics);\n",
                );
            } else {
                let _ = write!(
                    output,
                    "type_bridge_status_t TYPE_BRIDGE_CALL {selection_open}(\n\
                       const {binding} *binding, uint8_t distinct,\n\
                       const {order} *const *orders, size_t order_count,\n\
                       {selection} **out_selection,\n\
                       type_bridge_execution_diagnostics_t **out_diagnostics);\n",
                );
            }
            let _ = write!(
                output,
                "{selection_ref} TYPE_BRIDGE_CALL {selection_to_ref}(\n\
                   const {selection} *selection);\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {selection_close}(\n\
                   {selection} **selection);\n",
            );
        }

        let result_base = model_name(model, &format!("query_{mode}"))?;
        for (kind, kind_constant) in SELECTION_KINDS {
            let view = slot_view_name(model, mode, kind)?;
            let _ = write!(
                output,
                "\n/* Borrowed slot witness; valid only while its source result is open. */\n\
                 struct {view} {{\n\
                   const type_bridge_query_result_t *handle;\n\
                   type_bridge_query_result_kind_t expected_result_kind;\n\
                   uint32_t slot_index;\n\
                   const type_bridge_projected_token_v1_t *expected_model;\n\
                   type_bridge_query_match_mode_t expected_mode;\n\
                   type_bridge_query_selection_kind_t expected_kind;\n\
                   uint64_t reserved[4];\n\
                 }};\n\
                 {view} TYPE_BRIDGE_CALL {view}_rows(\n\
                   const {prefix}_query_rows_result *result, uint32_t slot_index);\n\
                 {view} TYPE_BRIDGE_CALL {view}_page(\n\
                   const {prefix}_query_page_result *result, uint32_t slot_index);\n",
            );
            let _ = kind_constant;
        }
        let one_at = generated_symbol(&result_base, "one_at")?;
        let collect_count = generated_symbol(&result_base, "collect_count")?;
        let collect_at = generated_symbol(&result_base, "collect_at")?;
        let result_target = if mode == "exact" {
            target.to_owned()
        } else {
            model_name(model, "query_subtypes_result")?
        };
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {one_at}(\n\
               {one_view} slot, size_t row_index,\n\
               {result_target} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {collect_count}(\n\
               {collect_view} slot, size_t row_index,\n\
               size_t *out_count,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {collect_at}(\n\
               {collect_view} slot, size_t row_index,\n\
               size_t value_index, {result_target} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n",
            one_view = slot_view_name(model, mode, "one")?,
            collect_view = slot_view_name(model, mode, "collect")?,
        );
    }

    for field in model.query_tokens().fields().values() {
        let base = field_base(model, prefix, field)?;
        let attribute = projected_attribute_model(
            projection,
            &type_bridge_contract::projection::ProjectedTypeRef::Model(
                type_bridge_contract::projection::ProjectedModelUse::new(
                    super::attribute_type_id(field.id())?,
                    type_bridge_contract::projection::ProjectedModelForm::Complete,
                ),
            ),
        )?;
        let domain = field_domain(projection, field)?;
        let domain_field = domain_field_ref_name(prefix, domain)?;
        let domain_ref = generated_symbol(&base, &format!("{}_field_ref", scalar_suffix(domain)))?;
        let _ = write!(
            output,
            "\n/* Owner-branded query field `{base}`. */\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_from_exact(\n\
               const {target}_query_exact_binding *binding, {base} **out_field,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_from_subtypes(\n\
               const {target}_query_subtypes_binding *binding, {base} **out_field,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             {field_ref} TYPE_BRIDGE_CALL {base}_ref(const {base} *field);\n\
             {domain_field} TYPE_BRIDGE_CALL {domain_ref}(const {base} *field);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_compare_value(\n\
               const {base} *field, type_bridge_query_comparison_t comparison,\n\
               const {attribute} *value, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_presence(\n\
               const {base} *field, uint8_t present,\n\
               {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_order(\n\
               const {base} *field, type_bridge_query_sort_direction_t direction,\n\
               type_bridge_query_missing_order_t missing, {order} **out_order,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_close({base} **field);\n",
            attribute = attribute.target_name().as_str(),
        );
    }

    for role in model.query_tokens().roles().values() {
        let base = role_base(model, prefix, role)?;
        let player_witness = role_player_binding_name(&base)?;
        let _ = write!(
            output,
            "\n/* Closed accepted-player binding witness for `{base}`. */\n\
             struct {player_witness} {{\n\
               const type_bridge_query_binding_t *handle;\n\
               const type_bridge_projected_token_v1_t *expected_model;\n\
               type_bridge_query_match_mode_t expected_mode;\n\
               uint32_t reserved[4];\n\
             }};\n\n\
             /* Owner-branded query role `{base}`. */\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_from_exact(\n\
               const {target}_query_exact_binding *binding, {base} **out_role,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_from_subtypes(\n\
               const {target}_query_subtypes_binding *binding, {base} **out_role,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             {role_ref} TYPE_BRIDGE_CALL {base}_ref(const {base} *role);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_close({base} **role);\n",
        );
        for player in role.accepted_players() {
            let player_model = projection
                .models()
                .get(player)
                .expect("projected query role player exists");
            for (mode, _) in QUERY_MODES {
                let player_binding = model_name(player_model, &format!("query_{mode}_binding"))?;
                let constructor =
                    role_player_binding_constructor(&base, prefix, player_model, mode)?;
                let _ = write!(
                    output,
                    "{player_witness} TYPE_BRIDGE_CALL {constructor}(\n\
                       const {player_binding} *player);\n",
                );
            }
        }
        if !role.accepted_players().is_empty() {
            let connects = generated_symbol(&base, "connects")?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {connects}(\n\
                   const {base} *role, {player_witness} player,\n\
                   {predicate} **out_predicate,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n",
            );
        }
    }

    if model.id().kind() == TypeKind::Relation && has_reachable_endpoint(model) {
        let endpoint = reachable_endpoint_name(model)?;
        let function = reachable_name(model)?;
        let _ = write!(
            output,
            "\n/* Closed relation endpoint: exact role plus an accepted-player binding. */\n\
             struct {endpoint} {{\n\
               const type_bridge_projected_token_v1_t *role;\n\
               const type_bridge_query_binding_t *player;\n\
               const type_bridge_projected_token_v1_t *expected_player_model;\n\
               type_bridge_query_match_mode_t expected_player_mode;\n\
               uint32_t reserved0;\n\
               uint64_t reserved[4];\n\
             }};\n",
        );
        for role in model.query_tokens().roles().values() {
            if role.accepted_players().is_empty() {
                continue;
            }
            let constructor = reachable_endpoint_constructor(model, prefix, role)?;
            let player = role_player_binding_name(&role_base(model, prefix, role)?)?;
            let _ = writeln!(
                output,
                "{endpoint} TYPE_BRIDGE_CALL {constructor}({player} player);"
            );
        }
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
               const {session} *session, {endpoint} source, {endpoint} target,\n\
               uint8_t min_depth, uint8_t max_depth,\n\
               {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n",
        );
    }

    render_subtype_union_declarations(output, projection, model, prefix)?;
    Ok(())
}

fn render_subtype_union_declarations(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let union = model_name(model, "query_subtypes_result")?;
    let kind_type = generated_symbol(&union, "kind_t")?;
    let unknown = generated_symbol(&union, "kind_unknown")?;
    let kind = generated_symbol(&union, "kind")?;
    let close = generated_symbol(&union, "close")?;
    let descendants = concrete_descendants(projection, model.id());
    let _ = write!(
        output,
        "\n/* Closed owned concrete result union for subtype-inclusive `{}`. */\n\
         typedef uint32_t {kind_type};\n\
         #define {unknown} UINT32_C(0)\n",
        model.target_name().as_str(),
    );
    for (index, descendant) in descendants.iter().enumerate() {
        let member = projected_member_suffix(prefix, descendant.target_name())?;
        let constant = generated_symbol(&union, &format!("kind_{member}"))?;
        let _ = writeln!(output, "#define {constant} UINT32_C({})", index + 1);
    }
    for descendant in descendants {
        let member = projected_member_suffix(prefix, descendant.target_name())?;
        let accessor = generated_symbol(&union, &format!("as_{member}"))?;
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {accessor}(\n\
               const {union} *value, {} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n",
            descendant.target_name().as_str(),
        );
    }
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {kind}(\n\
           const {union} *value, {kind_type} *out_kind,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {close}({union} **value);\n",
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_package_query_declarations(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    session: &str,
    predicate: &str,
    order: &str,
    query: &str,
    binding_ref: &str,
    _field_ref: &str,
    _role_ref: &str,
    selection_ref: &str,
    root_ref: &str,
    row_result_ref: &str,
) -> Result<(), Diagnostic> {
    let _ = write!(
        output,
        "\n/* Binding-neutral composition retains generated token expectations. */\n"
    );
    for domain in field_domains(projection)? {
        let domain_field = domain_field_ref_name(prefix, domain)?;
        let comparison = package_name(
            prefix,
            &format!("query_{}_field_compare_field", scalar_suffix(domain)),
        )?;
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {comparison}(\n\
               {domain_field} left, type_bridge_query_comparison_t comparison,\n\
               {domain_field} right, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n"
        );
    }
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_and(\n\
           const {predicate} *left, const {predicate} *right,\n\
           {predicate} **out_predicate,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_or(\n\
           const {predicate} *left, const {predicate} *right,\n\
           {predicate} **out_predicate,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_not(\n\
           const {predicate} *predicate, {predicate} **out_predicate,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_close(\n\
           {predicate} **predicate);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_order_close(\n\
           {order} **order);\n",
    );

    for arity in 1..=16 {
        let positional = package_name(prefix, &format!("query_positional_{arity}"))?;
        let named = package_name(prefix, &format!("query_named_{arity}"))?;
        let mut positional_parameters = String::new();
        let mut named_parameters = String::new();
        for index in 0..arity {
            let _ = writeln!(positional_parameters, "  {selection_ref} slot_{index},");
            let _ = writeln!(
                named_parameters,
                "  type_bridge_byte_view_t name_{index}, {selection_ref} slot_{index},"
            );
        }
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {positional}(\n\
               const {session} *session,\n{positional_parameters}\
               {query} **out_query,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {named}(\n\
               const {session} *session,\n{named_parameters}\
               {query} **out_query,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n",
        );
    }

    let rows_terminal = package_name(prefix, "query_rows_terminal")?;
    let first_terminal = package_name(prefix, "query_first_terminal")?;
    let page_terminal = package_name(prefix, "query_page_terminal")?;
    let count_terminal = package_name(prefix, "query_count_terminal")?;
    let exists_terminal = package_name(prefix, "query_exists_terminal")?;
    let rows_result = package_name(prefix, "query_rows_result")?;
    let page_result = package_name(prefix, "query_page_result")?;
    let count_result = package_name(prefix, "query_count_result")?;
    let exists_result = package_name(prefix, "query_exists_result")?;
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_where(\n\
           const {query} *query, const {predicate} *predicate,\n\
           {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_add_hidden(\n\
           const {query} *query, {binding_ref} binding, {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_allow_cross_join(\n\
           const {query} *query, {binding_ref} left, {binding_ref} right,\n\
           {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_close({query} **query);\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_one(\n\
           const {query} *query,\n\
           const {order} *const *orders, size_t order_count,\n\
           {rows_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_first(\n\
           const {query} *query,\n\
           const {order} *const *orders, size_t order_count,\n\
           {first_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_rows(\n\
           const {query} *query,\n\
           const {order} *const *orders, size_t order_count,\n\
           uint64_t offset, uint64_t limit, {rows_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_page(\n\
           {root_ref} root,\n\
           const {order} *const *orders, size_t order_count,\n\
           uint64_t offset, uint64_t limit, uint8_t include_total,\n\
           {page_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_count(\n\
           {root_ref} root,\n\
           {count_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_exists(\n\
           {root_ref} root,\n\
           {exists_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n",
    );
    for (terminal, suffix) in [
        (&rows_terminal, "rows"),
        (&first_terminal, "first"),
        (&page_terminal, "page"),
        (&count_terminal, "count"),
        (&exists_terminal, "exists"),
    ] {
        let _ = writeln!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{suffix}_terminal_close({terminal} **terminal);"
        );
    }

    for (scope, owner) in [
        ("database", "type_bridge_database_t"),
        ("read_transaction", "type_bridge_read_transaction_t"),
    ] {
        for (kind, terminal, result, _) in [
            (
                "rows",
                &rows_terminal,
                &rows_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_ROWS",
            ),
            (
                "first",
                &first_terminal,
                &rows_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_FIRST",
            ),
            (
                "page",
                &page_terminal,
                &page_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_PAGE",
            ),
            (
                "count",
                &count_terminal,
                &count_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_COUNT",
            ),
            (
                "exists",
                &exists_terminal,
                &exists_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_EXISTS",
            ),
        ] {
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_{scope}_query_execute_{kind}(\n\
                   const {owner} *owner, const {terminal} *terminal,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation,\n\
                   {result} **out_result,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n",
            );
        }
    }
    let _ = write!(
        output,
        "{row_result_ref} TYPE_BRIDGE_CALL {prefix}_query_rows_result_ref(\n\
           const {rows_result} *result);\n\
         {row_result_ref} TYPE_BRIDGE_CALL {prefix}_query_page_result_ref(\n\
           const {page_result} *result);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_result_row_count(\n\
           {row_result_ref} result, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_page_metadata(\n\
           const {page_result} *result,\n\
           type_bridge_query_page_metadata_v1_t *out_metadata,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_count_result_value(\n\
           const {count_result} *result, uint64_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_exists_result_value(\n\
           const {exists_result} *result, uint8_t *out_exists,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n",
    );
    for (result, suffix) in [
        (&rows_result, "rows"),
        (&page_result, "page"),
        (&count_result, "count"),
        (&exists_result, "exists"),
    ] {
        let _ = writeln!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{suffix}_result_close({result} **result);"
        );
    }
    Ok(())
}

fn render_remote_declarations(output: &mut String, prefix: &str) -> Result<(), Diagnostic> {
    let context = package_name(prefix, "query_remote_context")?;
    let _ = write!(
        output,
        r#"
/* Caller-transport remote query witnesses. Pending and claim handles are
 * family-specific owners; request/response byte views borrow them until the
 * corresponding handle is closed. */
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_remote_context_open(
  const type_bridge_schema_package_t *package,
  type_bridge_byte_view_t advertisement,
  const type_bridge_query_execution_limits_v1_t *limits,
  {context} **out_context,
  type_bridge_execution_diagnostics_t **out_diagnostics);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_remote_context_close(
  {context} **context);
"#,
    );

    for (family, _, result_suffix) in REMOTE_FAMILIES
        .into_iter()
        .chain(reduction::remote_families())
    {
        let terminal = package_name(prefix, &format!("query_{family}_terminal"))?;
        let result = package_name(prefix, result_suffix)?;
        let pending = package_name(prefix, &format!("query_{family}_remote_pending"))?;
        let claim = package_name(prefix, &format!("query_{family}_remote_claim"))?;
        let _ = write!(
            output,
            r#"type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_remote_prepare_{family}(
  const {context} *context, const {terminal} *terminal,
  const type_bridge_cancellation_t *cancellation,
  {pending} **out_pending,
  type_bridge_execution_diagnostics_t **out_diagnostics);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_request_bytes(
  const {pending} *pending, type_bridge_byte_view_t *out_request);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_response_snapshot_limit(
  const {pending} *pending, size_t *out_limit);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_claim(
  const {pending} *pending,
  const type_bridge_cancellation_t *cancellation,
  {claim} **out_claim,
  type_bridge_execution_diagnostics_t **out_diagnostics);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_close(
  {pending} **pending);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_claim_decode(
  const {claim} *claim,
  const type_bridge_cancellation_t *cancellation,
  type_bridge_byte_view_t response, {result} **out_result,
  type_bridge_execution_diagnostics_t **out_diagnostics);
type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_claim_close(
  {claim} **claim);
"#,
        );
    }
    Ok(())
}

// Definitions are emitted below in a second section so declaration and symbol
// inventories remain mechanically comparable.
pub(super) fn render_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    ordered: bool,
) -> Result<(), Diagnostic> {
    render_package_definitions(output, projection, prefix)?;
    for model in query_models(projection) {
        render_model_definitions(output, projection, model, prefix)?;
    }
    reduction::render_definitions(output, projection, prefix)?;
    render_remote_definitions(output, prefix)?;
    function::render_definitions(output, projection, prefix)?;
    if ordered {
        manager::render_definitions(output, projection, prefix)?;
    }
    Ok(())
}

pub(super) fn render_inline(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    ordered: bool,
) -> Result<(), Diagnostic> {
    let mut declarations = String::new();
    render_declarations(&mut declarations, projection, prefix, ordered)?;

    let mut definitions = String::new();
    render_definitions(&mut definitions, projection, prefix, ordered)?;

    let mut inline = String::with_capacity(declarations.len() + definitions.len());
    let mut skipping_function = false;
    for line in declarations.split_inclusive('\n') {
        if !skipping_function && line.contains("TYPE_BRIDGE_CALL") {
            skipping_function = true;
        }
        if skipping_function {
            if line.contains(';') {
                skipping_function = false;
            }
            continue;
        }
        inline.push_str(line);
    }
    for line in definitions.split_inclusive('\n') {
        if line.contains("TYPE_BRIDGE_CALL") {
            let indentation = line.len() - line.trim_start().len();
            inline.push_str(&line[..indentation]);
            inline.push_str("static inline ");
            inline.push_str(&line[indentation..]);
        } else {
            inline.push_str(line);
        }
    }
    let inline = bound_long_generated_identifiers(&inline)?;
    let rendered_function_names = inline
        .lines()
        .filter_map(|line| {
            let (_, tail) = line.split_once("TYPE_BRIDGE_CALL ")?;
            Some(tail.split_once('(')?.0.trim().to_owned())
        })
        .collect::<Vec<_>>();
    let unique_rendered_function_names = rendered_function_names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if rendered_function_names.len() != unique_rendered_function_names.len()
        || unique_rendered_function_names != function_names(projection, prefix, ordered)?
    {
        return Err(super::invalid(
            "c_emitter_query_function_inventory_drift",
            "generated C query function inventory differs from the emitted static-inline facade",
        ));
    }
    output.push_str(&inline);
    Ok(())
}

fn render_package_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let [
        binding_ref,
        _field_ref,
        _role_ref,
        selection_ref,
        root_ref,
        row_result_ref,
    ] = bridge_type_names(prefix)?;
    let session = package_name(prefix, "query_session")?;
    let predicate = package_name(prefix, "query_predicate")?;
    let order = package_name(prefix, "query_order")?;
    let query = package_name(prefix, "query")?;
    let rows_terminal = package_name(prefix, "query_rows_terminal")?;
    let first_terminal = package_name(prefix, "query_first_terminal")?;
    let page_terminal = package_name(prefix, "query_page_terminal")?;
    let count_terminal = package_name(prefix, "query_count_terminal")?;
    let exists_terminal = package_name(prefix, "query_exists_terminal")?;
    let rows_result = package_name(prefix, "query_rows_result")?;
    let page_result = package_name(prefix, "query_page_result")?;
    let count_result = package_name(prefix, "query_count_result")?;
    let exists_result = package_name(prefix, "query_exists_result")?;

    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_session_open(\n\
           const type_bridge_schema_package_t *package, {session} **out_session,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_session_open(\n\
               package, (type_bridge_query_session_t **)out_session, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_session_close(\n\
           {session} **session) {{\n\
           return type_bridge_query_session_close(\n\
               (type_bridge_query_session_t **)session);\n\
         }}\n\n",
    );
    for domain in field_domains(projection)? {
        let domain_field = domain_field_ref_name(prefix, domain)?;
        let comparison = package_name(
            prefix,
            &format!("query_{}_field_compare_field", scalar_suffix(domain)),
        )?;
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {comparison}(\n\
               {domain_field} left, type_bridge_query_comparison_t operation,\n\
               {domain_field} right, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_field_compare_field(\n\
                   left.handle, left.expected_field, operation,\n\
                   right.handle, right.expected_field,\n\
                   (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
             }}\n\n"
        );
    }
    for (suffix, operation, unary) in [
        ("and", "TYPE_BRIDGE_QUERY_PREDICATE_AND", false),
        ("or", "TYPE_BRIDGE_QUERY_PREDICATE_OR", false),
        ("not", "TYPE_BRIDGE_QUERY_PREDICATE_NOT", true),
    ] {
        if unary {
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_{suffix}(\n\
                   const {predicate} *predicate, {predicate} **out_predicate,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_predicate_combine(\n\
                       {operation}, (const type_bridge_query_predicate_t *)predicate, NULL,\n\
                       (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
                 }}\n\n",
            );
        } else {
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_{suffix}(\n\
                   const {predicate} *left, const {predicate} *right,\n\
                   {predicate} **out_predicate,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_predicate_combine(\n\
                       {operation}, (const type_bridge_query_predicate_t *)left,\n\
                       (const type_bridge_query_predicate_t *)right,\n\
                       (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
                 }}\n\n",
            );
        }
    }
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_predicate_close(\n\
           {predicate} **predicate) {{\n\
           return type_bridge_query_predicate_close(\n\
               (type_bridge_query_predicate_t **)predicate);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_order_close(\n\
           {order} **order) {{\n\
           return type_bridge_query_order_close((type_bridge_query_order_t **)order);\n\
         }}\n\n",
    );

    for arity in 1..=16 {
        render_shape_definition(
            output,
            prefix,
            arity,
            false,
            &session,
            &selection_ref,
            &query,
        )?;
        render_shape_definition(
            output,
            prefix,
            arity,
            true,
            &session,
            &selection_ref,
            &query,
        )?;
    }

    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_where(\n\
           const {query} *query, const {predicate} *predicate,\n\
           {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_where(\n\
               (const type_bridge_query_t *)query,\n\
               (const type_bridge_query_predicate_t *)predicate,\n\
               (type_bridge_query_t **)out_query, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_add_hidden(\n\
           const {query} *query, {binding_ref} binding, {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_add_hidden(\n\
               (const type_bridge_query_t *)query, binding.handle,\n\
               binding.expected_model, binding.expected_mode,\n\
               (type_bridge_query_t **)out_query, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_allow_cross_join(\n\
           const {query} *query, {binding_ref} left, {binding_ref} right,\n\
           {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_allow_cross_join(\n\
               (const type_bridge_query_t *)query, left.handle, left.expected_model,\n\
               left.expected_mode, right.handle, right.expected_model,\n\
               right.expected_mode, (type_bridge_query_t **)out_query,\n\
               out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_close({query} **query) {{\n\
           return type_bridge_query_close((type_bridge_query_t **)query);\n\
         }}\n\n",
    );

    render_terminal_definition(
        output,
        prefix,
        "one",
        "TYPE_BRIDGE_QUERY_TERMINAL_ROWS",
        "TYPE_BRIDGE_QUERY_ROWS_EXACTLY_ONE",
        &query,
        &root_ref,
        &order,
        &rows_terminal,
        false,
        false,
    );
    render_terminal_definition(
        output,
        prefix,
        "first",
        "TYPE_BRIDGE_QUERY_TERMINAL_FIRST",
        "TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY",
        &query,
        &root_ref,
        &order,
        &first_terminal,
        false,
        false,
    );
    render_terminal_definition(
        output,
        prefix,
        "rows",
        "TYPE_BRIDGE_QUERY_TERMINAL_ROWS",
        "TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY",
        &query,
        &root_ref,
        &order,
        &rows_terminal,
        true,
        false,
    );
    render_terminal_definition(
        output,
        prefix,
        "page",
        "TYPE_BRIDGE_QUERY_TERMINAL_PAGE",
        "TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY",
        &query,
        &root_ref,
        &order,
        &page_terminal,
        true,
        true,
    );
    for (suffix, kind, terminal) in [
        ("count", "TYPE_BRIDGE_QUERY_TERMINAL_COUNT", &count_terminal),
        (
            "exists",
            "TYPE_BRIDGE_QUERY_TERMINAL_EXISTS",
            &exists_terminal,
        ),
    ] {
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{suffix}(\n\
               {root_ref} root, {terminal} **out_terminal,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
                 sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
                 TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, {kind},\n\
                 TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY, root.root,\n\
                 root.expected_model, root.expected_mode, 0u, NULL, 0u,\n\
                 0u, 0u, 0u, {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
                 NULL, NULL, 0u, 0u, NULL, 0u, NULL, 0u,\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return type_bridge_query_terminal_open_v1(\n\
                   root.query, &descriptor,\n\
                   (type_bridge_query_terminal_t **)out_terminal, out_diagnostics);\n\
             }}\n\n",
        );
    }
    for (terminal, suffix) in [
        (&rows_terminal, "rows"),
        (&first_terminal, "first"),
        (&page_terminal, "page"),
        (&count_terminal, "count"),
        (&exists_terminal, "exists"),
    ] {
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{suffix}_terminal_close(\n\
               {terminal} **terminal) {{\n\
               return type_bridge_query_terminal_close(\n\
                   (type_bridge_query_terminal_t **)terminal);\n\
             }}\n\n",
        );
    }

    for (scope, owner, runtime) in [
        (
            "database",
            "type_bridge_database_t",
            "type_bridge_database_query_execute_v1",
        ),
        (
            "read_transaction",
            "type_bridge_read_transaction_t",
            "type_bridge_read_transaction_query_execute_v1",
        ),
    ] {
        for (kind, terminal, result, terminal_kind) in [
            (
                "rows",
                &rows_terminal,
                &rows_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_ROWS",
            ),
            (
                "first",
                &first_terminal,
                &rows_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_FIRST",
            ),
            (
                "page",
                &page_terminal,
                &page_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_PAGE",
            ),
            (
                "count",
                &count_terminal,
                &count_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_COUNT",
            ),
            (
                "exists",
                &exists_terminal,
                &exists_result,
                "TYPE_BRIDGE_QUERY_TERMINAL_EXISTS",
            ),
        ] {
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_{scope}_query_execute_{kind}(\n\
                   const {owner} *owner, const {terminal} *terminal,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation,\n\
                   {result} **out_result,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return {runtime}(owner,\n\
                       (const type_bridge_query_terminal_t *)terminal, {terminal_kind},\n\
                       limits, cancellation,\n\
                       (type_bridge_query_result_t **)out_result, out_diagnostics);\n\
                 }}\n\n",
            );
        }
    }

    let _ = write!(
        output,
        "{row_result_ref} TYPE_BRIDGE_CALL {prefix}_query_rows_result_ref(\n\
           const {rows_result} *result) {{\n\
           {row_result_ref} value = {{\n\
             (const type_bridge_query_result_t *)result, TYPE_BRIDGE_QUERY_RESULT_ROWS,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return value;\n\
         }}\n\n\
         {row_result_ref} TYPE_BRIDGE_CALL {prefix}_query_page_result_ref(\n\
           const {page_result} *result) {{\n\
           {row_result_ref} value = {{\n\
             (const type_bridge_query_result_t *)result, TYPE_BRIDGE_QUERY_RESULT_PAGE,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return value;\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_result_row_count(\n\
           {row_result_ref} result, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_row_count(\n\
               result.handle, result.expected_kind, out_count, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_page_metadata(\n\
           const {page_result} *result,\n\
           type_bridge_query_page_metadata_v1_t *out_metadata,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_page_metadata_v1(\n\
               (const type_bridge_query_result_t *)result, out_metadata,\n\
               out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_count_result_value(\n\
           const {count_result} *result, uint64_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_count(\n\
               (const type_bridge_query_result_t *)result, out_count, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_exists_result_value(\n\
           const {exists_result} *result, uint8_t *out_exists,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_exists(\n\
               (const type_bridge_query_result_t *)result, out_exists, out_diagnostics);\n\
         }}\n\n",
    );
    for (result, suffix) in [
        (&rows_result, "rows"),
        (&page_result, "page"),
        (&count_result, "count"),
        (&exists_result, "exists"),
    ] {
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{suffix}_result_close(\n\
               {result} **result) {{\n\
               return type_bridge_query_result_close(\n\
                   (type_bridge_query_result_t **)result);\n\
             }}\n\n",
        );
    }
    Ok(())
}

fn render_remote_definitions(output: &mut String, prefix: &str) -> Result<(), Diagnostic> {
    let context = package_name(prefix, "query_remote_context")?;
    let _ = write!(
        output,
        r#"type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_remote_context_open(
  const type_bridge_schema_package_t *package,
  type_bridge_byte_view_t advertisement,
  const type_bridge_query_execution_limits_v1_t *limits,
  {context} **out_context,
  type_bridge_execution_diagnostics_t **out_diagnostics) {{
  return type_bridge_query_remote_context_open_v1(
      package, advertisement, limits,
      (type_bridge_query_remote_context_t **)out_context, out_diagnostics);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_remote_context_close(
  {context} **context) {{
  return type_bridge_query_remote_context_close(
      (type_bridge_query_remote_context_t **)context);
}}

"#,
    );

    for (family, terminal_kind, result_suffix) in REMOTE_FAMILIES
        .into_iter()
        .chain(reduction::remote_families())
    {
        let terminal = package_name(prefix, &format!("query_{family}_terminal"))?;
        let result = package_name(prefix, result_suffix)?;
        let pending = package_name(prefix, &format!("query_{family}_remote_pending"))?;
        let claim = package_name(prefix, &format!("query_{family}_remote_claim"))?;
        let _ = write!(
            output,
            r#"type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_remote_prepare_{family}(
  const {context} *context, const {terminal} *terminal,
  const type_bridge_cancellation_t *cancellation,
  {pending} **out_pending,
  type_bridge_execution_diagnostics_t **out_diagnostics) {{
  return type_bridge_query_remote_prepare_v1(
      (const type_bridge_query_remote_context_t *)context,
      (const type_bridge_query_terminal_t *)terminal, {terminal_kind},
      cancellation, (type_bridge_query_remote_pending_t **)out_pending,
      out_diagnostics);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_request_bytes(
  const {pending} *pending, type_bridge_byte_view_t *out_request) {{
  return type_bridge_query_remote_pending_request_bytes(
      (const type_bridge_query_remote_pending_t *)pending, out_request);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_response_snapshot_limit(
  const {pending} *pending, size_t *out_limit) {{
  return type_bridge_query_remote_pending_response_snapshot_limit(
      (const type_bridge_query_remote_pending_t *)pending, out_limit);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_claim(
  const {pending} *pending,
  const type_bridge_cancellation_t *cancellation,
  {claim} **out_claim,
  type_bridge_execution_diagnostics_t **out_diagnostics) {{
  return type_bridge_query_remote_pending_claim(
      (const type_bridge_query_remote_pending_t *)pending, cancellation,
      (type_bridge_query_remote_claim_t **)out_claim, out_diagnostics);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_pending_close(
  {pending} **pending) {{
  return type_bridge_query_remote_pending_close(
      (type_bridge_query_remote_pending_t **)pending);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_claim_decode(
  const {claim} *claim,
  const type_bridge_cancellation_t *cancellation,
  type_bridge_byte_view_t response, {result} **out_result,
  type_bridge_execution_diagnostics_t **out_diagnostics) {{
  return type_bridge_query_remote_claim_decode_v1(
      (const type_bridge_query_remote_claim_t *)claim, {terminal_kind}, cancellation,
      response, (type_bridge_query_result_t **)out_result, out_diagnostics);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_remote_claim_close(
  {claim} **claim) {{
  return type_bridge_query_remote_claim_close(
      (type_bridge_query_remote_claim_t **)claim);
}}

"#,
        );
    }
    Ok(())
}

fn render_model_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let session = package_name(prefix, "query_session")?;
    let predicate = package_name(prefix, "query_predicate")?;
    let order = package_name(prefix, "query_order")?;
    let [
        binding_ref,
        field_ref,
        role_ref,
        selection_ref,
        root_ref,
        row_result_ref,
    ] = bridge_type_names(prefix)?;
    let model_token = model_token_symbol(projection, prefix, model.id())?;

    for (mode, mode_constant) in QUERY_MODES {
        let binding = model_name(model, &format!("query_{mode}_binding"))?;
        let open = generated_symbol(&binding, "open")?;
        let to_ref = generated_symbol(&binding, "ref")?;
        let to_root = generated_symbol(&binding, "root")?;
        let iid = generated_symbol(&binding, "iid")?;
        let iid_in = generated_symbol(&binding, "iid_in")?;
        let close = generated_symbol(&binding, "close")?;
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {open}(\n\
               const {session} *session, {binding} **out_binding,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_binding_open_v1(\n\
                   (const type_bridge_query_session_t *)session, &{model_token},\n\
                   {mode_constant}, (type_bridge_query_binding_t **)out_binding,\n\
                   out_diagnostics);\n\
             }}\n\n\
             {binding_ref} TYPE_BRIDGE_CALL {to_ref}(const {binding} *binding) {{\n\
               {binding_ref} value = {{\n\
                 (const type_bridge_query_binding_t *)binding, &{model_token},\n\
                 {mode_constant}, {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n\
             {root_ref} TYPE_BRIDGE_CALL {to_root}(\n\
               const {prefix}_query *query, const {binding} *binding) {{\n\
               {root_ref} value = {{\n\
                 (const type_bridge_query_t *)query,\n\
                 (const type_bridge_query_binding_t *)binding, &{model_token},\n\
                 {mode_constant}, {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {iid}(\n\
               const {binding} *binding, type_bridge_byte_view_t iid,\n\
               {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_binding_iid_v1(\n\
                   (const type_bridge_query_binding_t *)binding, &{model_token},\n\
                   {mode_constant}, iid,\n\
                   (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {iid_in}(\n\
               const {binding} *binding, const type_bridge_byte_view_t *iids,\n\
               size_t iid_count, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_binding_iid_in_v1(\n\
                   (const type_bridge_query_binding_t *)binding, &{model_token},\n\
                   {mode_constant}, iids, iid_count,\n\
                   (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {close}({binding} **binding) {{\n\
               return type_bridge_query_binding_close(\n\
                   (type_bridge_query_binding_t **)binding);\n\
             }}\n\n",
        );
        for (kind, kind_constant) in SELECTION_KINDS {
            let selection = model_name(model, &format!("query_{mode}_{kind}_selection"))?;
            let selection_open = generated_symbol(&selection, "open")?;
            let selection_to_ref = generated_symbol(&selection, "ref")?;
            let selection_close = generated_symbol(&selection, "close")?;
            let (parameters, distinct, orders, order_count) = if kind == "one" {
                (String::new(), "0u", "NULL", "0u")
            } else {
                (
                    format!(
                        "  uint8_t distinct, const {order} *const *orders, size_t order_count,\n"
                    ),
                    "distinct",
                    "(const type_bridge_query_order_t *const *)orders",
                    "order_count",
                )
            };
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {selection_open}(\n\
                   const {binding} *binding,\n{parameters}\
                   {selection} **out_selection,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   type_bridge_query_selection_descriptor_v1_t descriptor = {{\n\
                     sizeof(type_bridge_query_selection_descriptor_v1_t),\n\
                     TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION,\n\
                     (const type_bridge_query_binding_t *)binding, &{model_token},\n\
                     {mode_constant}, {kind_constant}, {distinct},\n\
                     {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
                     {orders}, {order_count}, {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return type_bridge_query_selection_open_v1(\n\
                       &descriptor, (type_bridge_query_selection_t **)out_selection,\n\
                       out_diagnostics);\n\
                 }}\n\n\
                 {selection_ref} TYPE_BRIDGE_CALL {selection_to_ref}(\n\
                   const {selection} *selection) {{\n\
                   {selection_ref} value = {{\n\
                     (const type_bridge_query_selection_t *)selection, &{model_token},\n\
                     {mode_constant}, {kind_constant}, {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {selection_close}(\n\
                   {selection} **selection) {{\n\
                   return type_bridge_query_selection_close(\n\
                       (type_bridge_query_selection_t **)selection);\n\
                 }}\n\n",
            );
        }
    }

    for field in model.query_tokens().fields().values() {
        let base = field_base(model, prefix, field)?;
        let token = field_token_symbol(projection, prefix, model.id(), field.id())?;
        for (mode, _) in QUERY_MODES {
            let binding = model_name(model, &format!("query_{mode}_binding"))?;
            let function = generated_symbol(&base, &format!("from_{mode}"))?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
                   const {binding} *binding, {base} **out_field,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_field_open(\n\
                       (const type_bridge_query_binding_t *)binding, &{token},\n\
                       (type_bridge_query_field_t **)out_field, out_diagnostics);\n\
                 }}\n\n",
            );
        }
        let attribute_model = projection
            .models()
            .get(&super::attribute_type_id(field.id())?)
            .expect("projected query field attribute exists");
        let attribute = attribute_model.target_name().as_str();
        let domain = field_domain(projection, field)?;
        let domain_field = domain_field_ref_name(prefix, domain)?;
        let domain_ref = generated_symbol(&base, &format!("{}_field_ref", scalar_suffix(domain)))?;
        let _ = write!(
            output,
            "{field_ref} TYPE_BRIDGE_CALL {base}_ref(const {base} *field) {{\n\
               {field_ref} value = {{\n\
                 (const type_bridge_query_field_t *)field, &{token},\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n\
             {domain_field} TYPE_BRIDGE_CALL {domain_ref}(const {base} *field) {{\n\
               {domain_field} value = {{\n\
                 (const type_bridge_query_field_t *)field, &{token},\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_compare_value(\n\
               const {base} *field, type_bridge_query_comparison_t comparison,\n\
               const {attribute} *value, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_field_compare_value(\n\
                   (const type_bridge_query_field_t *)field, &{token}, comparison,\n\
                   (const type_bridge_projected_value_t *)value,\n\
                   (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_presence(\n\
               const {base} *field, uint8_t present, {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_field_presence(\n\
                   (const type_bridge_query_field_t *)field, &{token}, present,\n\
                   (type_bridge_query_predicate_t **)out_predicate, out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_order(\n\
               const {base} *field, type_bridge_query_sort_direction_t direction,\n\
               type_bridge_query_missing_order_t missing, {order} **out_order,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               type_bridge_query_order_descriptor_v1_t descriptor = {{\n\
                 sizeof(type_bridge_query_order_descriptor_v1_t),\n\
                 TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION,\n\
                 (const type_bridge_query_field_t *)field, &{token}, direction,\n\
                 missing, 0u, {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return type_bridge_query_order_open_v1(\n\
                   &descriptor, (type_bridge_query_order_t **)out_order,\n\
                   out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_close({base} **field) {{\n\
               return type_bridge_query_field_close((type_bridge_query_field_t **)field);\n\
             }}\n\n",
        );
    }

    for role in model.query_tokens().roles().values() {
        let base = role_base(model, prefix, role)?;
        let player_witness = role_player_binding_name(&base)?;
        let token = role_token_symbol(projection, prefix, model.id(), role.role())?;
        for (mode, _) in QUERY_MODES {
            let binding = model_name(model, &format!("query_{mode}_binding"))?;
            let function = generated_symbol(&base, &format!("from_{mode}"))?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
                   const {binding} *binding, {base} **out_role,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_role_open(\n\
                       (const type_bridge_query_binding_t *)binding, &{token},\n\
                       (type_bridge_query_role_t **)out_role, out_diagnostics);\n\
                 }}\n\n",
            );
        }
        let _ = write!(
            output,
            "{role_ref} TYPE_BRIDGE_CALL {base}_ref(const {base} *role) {{\n\
               {role_ref} value = {{\n\
                 (const type_bridge_query_role_t *)role, &{token},\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {base}_close({base} **role) {{\n\
               return type_bridge_query_role_close((type_bridge_query_role_t **)role);\n\
             }}\n\n",
        );
        for player in role.accepted_players() {
            let player_model = projection
                .models()
                .get(player)
                .expect("projected query role player exists");
            let player_token = model_token_symbol(projection, prefix, player)?;
            for (mode, mode_constant) in QUERY_MODES {
                let player_binding = model_name(player_model, &format!("query_{mode}_binding"))?;
                let constructor =
                    role_player_binding_constructor(&base, prefix, player_model, mode)?;
                let _ = write!(
                    output,
                    "{player_witness} TYPE_BRIDGE_CALL {constructor}(\n\
                       const {player_binding} *player) {{\n\
                       {player_witness} value = {{\n\
                         (const type_bridge_query_binding_t *)player, &{player_token},\n\
                         {mode_constant}, {{ 0u, 0u, 0u, 0u }}\n\
                       }};\n\
                       return value;\n\
                     }}\n\n",
                );
            }
        }
        if !role.accepted_players().is_empty() {
            let connects = generated_symbol(&base, "connects")?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {connects}(\n\
                   const {base} *role, {player_witness} player,\n\
                   {predicate} **out_predicate,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_role_connects(\n\
                       (const type_bridge_query_role_t *)role, &{token},\n\
                       player.handle, player.expected_model, player.expected_mode,\n\
                       (type_bridge_query_predicate_t **)out_predicate,\n\
                       out_diagnostics);\n\
                 }}\n\n",
            );
        }
    }

    if model.id().kind() == TypeKind::Relation && has_reachable_endpoint(model) {
        let relation_token = model_token_symbol(projection, prefix, model.id())?;
        let endpoint = reachable_endpoint_name(model)?;
        for role in model.query_tokens().roles().values() {
            if role.accepted_players().is_empty() {
                continue;
            }
            let role_token = role_token_symbol(projection, prefix, model.id(), role.role())?;
            let constructor = reachable_endpoint_constructor(model, prefix, role)?;
            let player = role_player_binding_name(&role_base(model, prefix, role)?)?;
            let _ = write!(
                output,
                "{endpoint} TYPE_BRIDGE_CALL {constructor}({player} player) {{\n\
                   {endpoint} value = {{\n\
                     &{role_token}, player.handle, player.expected_model,\n\
                     player.expected_mode, 0u, {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n",
            );
        }
        let function = reachable_name(model)?;
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
               const {session} *session, {endpoint} source, {endpoint} target,\n\
               uint8_t min_depth, uint8_t max_depth,\n\
               {predicate} **out_predicate,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_session_reachable(\n\
                   (const type_bridge_query_session_t *)session,\n\
                   &{relation_token}, source.role, target.role,\n\
                   source.player, source.expected_player_model,\n\
                   source.expected_player_mode, target.player,\n\
                   target.expected_player_model, target.expected_player_mode,\n\
                   min_depth, max_depth,\n\
                   (type_bridge_query_predicate_t **)out_predicate,\n\
                   out_diagnostics);\n\
             }}\n\n",
        );
    }

    render_result_definitions(
        output,
        projection,
        model,
        prefix,
        &row_result_ref,
        &model_token,
    )?;
    Ok(())
}

fn render_result_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
    _row_result_ref: &str,
    model_token: &str,
) -> Result<(), Diagnostic> {
    for (mode, mode_constant) in QUERY_MODES {
        let result_base = model_name(model, &format!("query_{mode}"))?;
        for (kind, kind_constant) in SELECTION_KINDS {
            let view = slot_view_name(model, mode, kind)?;
            for (source, result_type, result_kind) in [
                (
                    "rows",
                    package_name(prefix, "query_rows_result")?,
                    "TYPE_BRIDGE_QUERY_RESULT_ROWS",
                ),
                (
                    "page",
                    package_name(prefix, "query_page_result")?,
                    "TYPE_BRIDGE_QUERY_RESULT_PAGE",
                ),
            ] {
                let function = generated_symbol(&view, source)?;
                let _ = write!(
                    output,
                    "{view} TYPE_BRIDGE_CALL {function}(\n\
                       const {result_type} *result, uint32_t slot_index) {{\n\
                       {view} value = {{\n\
                         (const type_bridge_query_result_t *)result, {result_kind},\n\
                         slot_index, &{model_token}, {mode_constant}, {kind_constant},\n\
                         {{ 0u, 0u, 0u, 0u }}\n\
                       }};\n\
                       return value;\n\
                     }}\n\n",
                );
            }
        }
        let one_at = generated_symbol(&result_base, "one_at")?;
        let collect_count = generated_symbol(&result_base, "collect_count")?;
        let collect_at = generated_symbol(&result_base, "collect_at")?;
        let result_target = if mode == "exact" {
            model.target_name().as_str().to_owned()
        } else {
            model_name(model, "query_subtypes_result")?
        };
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {one_at}(\n\
               {one_view} slot, size_t row_index,\n\
               {result_target} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_result_row_slot_thing_at(\n\
                   slot.handle, slot.expected_result_kind, row_index,\n\
                   slot.slot_index, 0u, slot.expected_model, slot.expected_mode,\n\
                   slot.expected_kind,\n\
                   (type_bridge_projected_thing_t **)out_value, out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {collect_count}(\n\
               {collect_view} slot, size_t row_index,\n\
               size_t *out_count,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_result_row_slot_count(\n\
                   slot.handle, slot.expected_result_kind, row_index, slot.slot_index,\n\
                   slot.expected_model, slot.expected_mode, slot.expected_kind,\n\
                   out_count, out_diagnostics);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {collect_at}(\n\
               {collect_view} slot, size_t row_index,\n\
               size_t value_index, {result_target} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               return type_bridge_query_result_row_slot_thing_at(\n\
                   slot.handle, slot.expected_result_kind, row_index, slot.slot_index,\n\
                   value_index, slot.expected_model, slot.expected_mode,\n\
                   slot.expected_kind,\n\
                   (type_bridge_projected_thing_t **)out_value, out_diagnostics);\n\
             }}\n\n",
            one_view = slot_view_name(model, mode, "one")?,
            collect_view = slot_view_name(model, mode, "collect")?,
        );
    }
    render_subtype_union_definitions(output, projection, model, prefix)
}

fn render_subtype_union_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let union = model_name(model, "query_subtypes_result")?;
    let kind_type = generated_symbol(&union, "kind_t")?;
    let unknown = generated_symbol(&union, "kind_unknown")?;
    let kind = generated_symbol(&union, "kind")?;
    let close = generated_symbol(&union, "close")?;
    let descendants = concrete_descendants(projection, model.id());
    let add_input = generated_symbol(prefix, "generated_alias_input_v1")?;
    let preflight = generated_symbol(prefix, "generated_alias_preflight_v1")?;
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {kind}(\n\
           const {union} *value, {kind_type} *out_kind,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_generated_opaque_input_v1_t alias_inputs[1];\n\
           size_t alias_input_count = 0u;\n\
           uint32_t ordinal = 0u;\n\
           type_bridge_execution_diagnostics_t *local_diagnostics = NULL;\n\
           type_bridge_status_t status;\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_THING, value, 1u);\n\
           status = {preflight}(alias_inputs, alias_input_count,\n\
               out_kind, out_kind == NULL ? 0u : sizeof(*out_kind),\n\
               out_diagnostics, out_diagnostics == NULL ? 0u : sizeof(*out_diagnostics));\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}\n\
           if (out_kind == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}\n\
           *out_kind = {unknown};\n\
           status = type_bridge_projected_thing_model_ordinal(\n\
               (const type_bridge_projected_thing_t *)value, &ordinal,\n\
               &local_diagnostics);\n\
           if (out_diagnostics != NULL) {{\n\
             *out_diagnostics = local_diagnostics;\n\
           }} else if (local_diagnostics != NULL) {{\n\
             (void)type_bridge_execution_diagnostics_close(&local_diagnostics);\n\
           }}\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n",
    );
    for (index, descendant) in descendants.iter().enumerate() {
        let ordinal = model_token_ordinal(projection, descendant.id())?;
        let member = projected_member_suffix(prefix, descendant.target_name())?;
        let constant = generated_symbol(&union, &format!("kind_{member}"))?;
        let _ = writeln!(
            output,
            "  if (ordinal == {ordinal}u) {{ *out_kind = {constant}; return TYPE_BRIDGE_STATUS_OK; }}"
        );
        let _ = index;
    }
    output.push_str("  return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n}\n\n");
    for descendant in &descendants {
        let member = projected_member_suffix(prefix, descendant.target_name())?;
        let accessor = generated_symbol(&union, &format!("as_{member}"))?;
        let token = model_token_symbol(projection, prefix, descendant.id())?;
        let target = descendant.target_name().as_str();
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {accessor}(\n\
               const {union} *value, {target} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               type_bridge_generated_opaque_input_v1_t alias_inputs[1];\n\
               size_t alias_input_count = 0u;\n\
               type_bridge_projected_thing_t *local_value = NULL;\n\
               type_bridge_execution_diagnostics_t *local_diagnostics = NULL;\n\
               type_bridge_status_t status;\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_THING, value, 1u);\n\
               status = {preflight}(alias_inputs, alias_input_count,\n\
                   out_value, out_value == NULL ? 0u : sizeof(*out_value),\n\
                   out_diagnostics, out_diagnostics == NULL ? 0u : sizeof(*out_diagnostics));\n\
               if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
               if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}\n\
               if (out_value == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}\n\
               *out_value = NULL;\n\
               status = type_bridge_projected_thing_validate_model(\n\
                   (const type_bridge_projected_thing_t *)value, &{token},\n\
                   &local_diagnostics);\n\
               if (status == TYPE_BRIDGE_STATUS_OK) {{\n\
                 status = type_bridge_projected_thing_clone(\n\
                   (const type_bridge_projected_thing_t *)value,\n\
                   &local_value, &local_diagnostics);\n\
               }}\n\
               *out_value = ({target} *)local_value;\n\
               if (out_diagnostics != NULL) {{\n\
                 *out_diagnostics = local_diagnostics;\n\
               }} else if (local_diagnostics != NULL) {{\n\
                 (void)type_bridge_execution_diagnostics_close(&local_diagnostics);\n\
               }}\n\
               return status;\n\
             }}\n\n",
        );
    }
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {close}({union} **value) {{\n\
           return type_bridge_projected_thing_close(\n\
               (type_bridge_projected_thing_t **)value);\n\
         }}\n\n",
    );
    Ok(())
}

fn render_shape_definition(
    output: &mut String,
    prefix: &str,
    arity: usize,
    named: bool,
    session: &str,
    selection_ref: &str,
    query: &str,
) -> Result<(), Diagnostic> {
    let shape = if named { "named" } else { "positional" };
    let function = package_name(prefix, &format!("query_{shape}_{arity}"))?;
    let mut parameters = String::new();
    let mut initializers = String::new();
    for index in 0..arity {
        if named {
            let _ = writeln!(
                parameters,
                "  type_bridge_byte_view_t name_{index}, {selection_ref} slot_{index},"
            );
        } else {
            let _ = writeln!(parameters, "  {selection_ref} slot_{index},");
        }
        let name_assignment = if named {
            format!("slots[{index}].name = name_{index};")
        } else {
            format!("slots[{index}].name.data = NULL;\n  slots[{index}].name.length = 0u;")
        };
        let _ = write!(
            initializers,
            "  slots[{index}].struct_size = sizeof(type_bridge_query_shape_slot_v1_t);\n\
             \x20 slots[{index}].version = TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION;\n\
             \x20 slots[{index}].selection = slot_{index}.handle;\n\
             \x20 slots[{index}].expected_model = slot_{index}.expected_model;\n\
             \x20 slots[{index}].expected_mode = slot_{index}.expected_mode;\n\
             \x20 slots[{index}].expected_kind = slot_{index}.expected_kind;\n\
             \x20 slots[{index}].reserved0 = 0u;\n\
             \x20 {name_assignment}\n\
             \x20 slots[{index}].reserved[0] = 0u;\n\
             \x20 slots[{index}].reserved[1] = 0u;\n\
             \x20 slots[{index}].reserved[2] = 0u;\n\
             \x20 slots[{index}].reserved[3] = 0u;\n"
        );
    }
    let shape_kind = if named {
        "TYPE_BRIDGE_QUERY_SHAPE_NAMED"
    } else {
        "TYPE_BRIDGE_QUERY_SHAPE_POSITIONAL"
    };
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
           const {session} *session,\n{parameters}\
           {query} **out_query,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_shape_slot_v1_t slots[{arity}];\n\
           type_bridge_query_descriptor_v1_t descriptor;\n\
         {initializers}\
           descriptor.struct_size = sizeof(type_bridge_query_descriptor_v1_t);\n\
           descriptor.version = TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION;\n\
           descriptor.shape_kind = {shape_kind};\n\
           descriptor.reserved0 = 0u;\n\
           descriptor.slots = slots;\n\
           descriptor.slot_count = {arity}u;\n\
           descriptor.reserved[0] = 0u;\n\
           descriptor.reserved[1] = 0u;\n\
           descriptor.reserved[2] = 0u;\n\
           descriptor.reserved[3] = 0u;\n\
           return type_bridge_query_open_v1(\n\
               (const type_bridge_query_session_t *)session, &descriptor,\n\
               (type_bridge_query_t **)out_query, out_diagnostics);\n\
         }}\n\n",
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_terminal_definition(
    output: &mut String,
    prefix: &str,
    suffix: &str,
    kind: &str,
    cardinality: &str,
    query: &str,
    root_ref: &str,
    order: &str,
    terminal: &str,
    has_window: bool,
    is_page: bool,
) {
    let has_root = is_page;
    let root_parameter = if has_root {
        format!("  {root_ref} root,\n")
    } else {
        String::new()
    };
    let query_parameter = if has_root {
        String::new()
    } else {
        format!("  const {query} *query,\n")
    };
    let window_parameters = if has_window {
        "  uint64_t offset, uint64_t limit,\n"
    } else {
        ""
    };
    let total_parameter = if is_page {
        "  uint8_t include_total,\n"
    } else {
        ""
    };
    let offset = if has_window { "offset" } else { "0u" };
    let limit = if has_window {
        "limit"
    } else if matches!(suffix, "one" | "first") {
        "1u"
    } else {
        "0u"
    };
    let include_total = if is_page { "include_total" } else { "0u" };
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{suffix}(\n\
         {query_parameter}{root_parameter}\
           const {order} *const *orders, size_t order_count,\n\
         {window_parameters}{total_parameter}\
           {terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
             sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, {kind}, {cardinality},\n\
             {root_handle}, {root_model}, {root_mode}, 0u,\n\
             (const type_bridge_query_order_t *const *)orders, order_count,\n\
             {offset}, {limit}, {include_total},\n\
             {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
             NULL, NULL, 0u, 0u, NULL, 0u, NULL, 0u,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return type_bridge_query_terminal_open_v1(\n\
               {query_handle}, &descriptor,\n\
               (type_bridge_query_terminal_t **)out_terminal, out_diagnostics);\n\
         }}\n\n",
        root_handle = if has_root { "root.root" } else { "NULL" },
        root_model = if has_root {
            "root.expected_model"
        } else {
            "NULL"
        },
        root_mode = if has_root { "root.expected_mode" } else { "0u" },
        query_handle = if has_root {
            "root.query"
        } else {
            "(const type_bridge_query_t *)query"
        },
    );
}
