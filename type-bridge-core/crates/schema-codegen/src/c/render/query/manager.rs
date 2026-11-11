use std::collections::BTreeSet;
use std::fmt::Write as _;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{ModelProjection, RuntimeProjection};

use super::super::{
    attribute_type_id, field_token_symbol, generated_symbol, model_token_symbol,
    projected_member_suffix,
};
use super::{model_name, query_models};

const OPERATIONS: [(&str, &str); 6] = [
    ("eq", "TYPE_BRIDGE_QUERY_COMPARE_EQUAL"),
    ("ne", "TYPE_BRIDGE_QUERY_COMPARE_NOT_EQUAL"),
    ("lt", "TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN"),
    ("le", "TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN_OR_EQUAL"),
    ("gt", "TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN"),
    ("ge", "TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL"),
];

fn manager_name(model: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(model, "manager")
}

fn filter_name(model: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(model, "manager_filter")
}

fn result_name(model: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(model, "manager_all_result")
}

fn filter_ref_name(model: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(model, "manager_filter_ref_v1_t")
}

fn field_member(
    prefix: &str,
    field: &type_bridge_contract::projection::FieldTokenProjection,
) -> Result<String, Diagnostic> {
    projected_member_suffix(prefix, field.target_name()).map(str::to_owned)
}

fn predicate_name(
    model: &ModelProjection,
    prefix: &str,
    field: &type_bridge_contract::projection::FieldTokenProjection,
    operation: &str,
) -> Result<String, Diagnostic> {
    generated_symbol(
        &manager_name(model)?,
        &format!("filter_{}_{operation}", field_member(prefix, field)?),
    )
}

pub(super) fn nominal_names(
    projection: &RuntimeProjection,
    _prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for model in query_models(projection) {
        names.insert(manager_name(model)?);
        names.insert(filter_name(model)?);
        names.insert(result_name(model)?);
    }
    Ok(names)
}

pub(super) fn auxiliary_type_names(
    projection: &RuntimeProjection,
) -> Result<BTreeSet<String>, Diagnostic> {
    query_models(projection).map(filter_ref_name).collect()
}

pub(super) fn function_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for model in query_models(projection) {
        let manager = manager_name(model)?;
        for suffix in [
            "open",
            "close",
            "filter_root",
            "filter_ref",
            "filter_close",
            "filter_iid",
            "filter_iids",
            "database_all",
            "database_first",
            "database_count",
            "database_exists",
            "read_transaction_all",
            "read_transaction_first",
            "read_transaction_count",
            "read_transaction_exists",
            "write_transaction_all",
            "write_transaction_first",
            "write_transaction_count",
            "write_transaction_exists",
            "proxy_connection_all",
            "proxy_connection_first",
            "proxy_connection_count",
            "proxy_connection_exists",
        ] {
            names.insert(generated_symbol(&manager, suffix)?);
        }
        if model.create().target_name().is_some() {
            for scope in ["database", "write_transaction", "proxy_connection"] {
                for operation in [
                    "insert",
                    "put",
                    "update",
                    "delete",
                    "delete_returning",
                    "delete_many_returning",
                ] {
                    names.insert(generated_symbol(&manager, &format!("{scope}_{operation}"))?);
                }
            }
        }
        let result = result_name(model)?;
        for suffix in ["count", "at", "close"] {
            names.insert(generated_symbol(&result, suffix)?);
        }
        for field in model.query_tokens().fields().values() {
            for (operation, _) in OPERATIONS {
                names.insert(predicate_name(model, prefix, field, operation)?);
            }
        }
    }
    Ok(names)
}

pub(super) fn render_declarations(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    for model in query_models(projection) {
        let target = model.target_name().as_str();
        let manager = manager_name(model)?;
        let filter = filter_name(model)?;
        let result = result_name(model)?;
        let filter_ref = filter_ref_name(model)?;
        let _ = write!(
            output,
            "\n/* Exact-model immutable manager/filter facade for `{target}`. */\n\
             struct {manager} {{\n\
               type_bridge_manager_filter_t *root_filter;\n\
               uint64_t reserved[4];\n\
             }};\n\
             typedef struct {filter_ref} {{\n\
               const type_bridge_manager_filter_t *value;\n\
               uint64_t reserved[4];\n\
             }} {filter_ref};\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {manager}_open(\n\
               const type_bridge_schema_package_t *package, {manager} *out_manager,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {manager}_close({manager} *manager);\n\
             {filter_ref} TYPE_BRIDGE_CALL {manager}_filter_root(\n\
               const {manager} *manager);\n\
             {filter_ref} TYPE_BRIDGE_CALL {manager}_filter_ref(\n\
               const {filter} *filter);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {manager}_filter_close(\n\
               {filter} **filter);\n"
        );
        for (suffix, arguments) in [
            ("iid", "const type_bridge_iid_t *iid"),
            (
                "iids",
                "const type_bridge_iid_t *const *iids, size_t iid_count",
            ),
        ] {
            let _ = write!(
                output,
                r#"
type_bridge_status_t TYPE_BRIDGE_CALL {manager}_filter_{suffix}(
    const {manager} *manager, {filter_ref} source,
    {arguments}, {filter} **out_filter,
    type_bridge_execution_diagnostics_t **out_diagnostics);
"#
            );
        }
        for field in model.query_tokens().fields().values() {
            let attribute = projection
                .models()
                .get(&attribute_type_id(field.id())?)
                .expect("projected manager field attribute exists")
                .target_name()
                .as_str();
            for (operation, _) in OPERATIONS {
                let function = predicate_name(model, prefix, field, operation)?;
                let _ = write!(
                    output,
                    "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
                       const {manager} *manager, {filter_ref} source,\n\
                       const {attribute} *value, {filter} **out_filter,\n\
                       type_bridge_execution_diagnostics_t **out_diagnostics);\n"
                );
            }
        }
        for (scope, owner) in [
            ("database", "type_bridge_database_t"),
            ("read_transaction", "type_bridge_read_transaction_t"),
            ("write_transaction", "type_bridge_write_transaction_t"),
            ("proxy_connection", "type_bridge_proxy_connection_t"),
        ] {
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_all(\n\
                   const {owner} *owner, const {manager} *manager, {filter_ref} filter,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation,\n\
                   {result} **out_result,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_first(\n\
                   const {owner} *owner, const {manager} *manager, {filter_ref} filter,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation, {target} **out_value,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_count(\n\
                   const {owner} *owner, const {manager} *manager, {filter_ref} filter,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation, uint64_t *out_count,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_exists(\n\
                   const {owner} *owner, const {manager} *manager, {filter_ref} filter,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation, uint8_t *out_exists,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n"
            );
        }
        render_writes(output, projection, model, prefix, false)?;
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {result}_count(\n\
               const {result} *result, size_t *out_count,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {result}_at(\n\
               const {result} *result, size_t index, {target} **out_value,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {result}_close({result} **result);\n"
        );
    }
    Ok(())
}

pub(super) fn render_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    for model in query_models(projection) {
        render_manager_definitions(output, projection, model, prefix)?;
    }
    Ok(())
}

/// Emit both outer and deeply borrowed input fences before touching caller outputs.
#[allow(clippy::too_many_arguments)]
fn input_fence(
    output: &mut String,
    source: &str,
    model_token: &str,
    add_input: &str,
    preflight: &str,
    extra: &str,
    out: &str,
) {
    let out_size = if out == "NULL" {
        "0u".to_owned()
    } else {
        format!("sizeof(*{out})")
    };
    let _ = write!(
        output,
        r#"
  type_bridge_generated_opaque_input_v1_t alias_inputs[8];
  size_t alias_input_count = 0u;
  type_bridge_status_t status;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_BYTES,
      manager, manager == NULL ? 0u : sizeof(*manager));
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_MANAGER_FILTER,
      {source}.value, 1u);
{extra}
  status = {preflight}(alias_inputs, alias_input_count,
      {out}, {out_size}, out_diagnostics, sizeof(*out_diagnostics));
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  if (manager == NULL || manager->reserved[0] != 0u || manager->reserved[1] != 0u ||
      manager->reserved[2] != 0u || manager->reserved[3] != 0u ||
      {source}.reserved[0] != 0u || {source}.reserved[1] != 0u ||
      {source}.reserved[2] != 0u || {source}.reserved[3] != 0u) {{
    return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
  }}
  alias_input_count = 0u;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_MANAGER_FILTER,
      manager->root_filter, 1u);
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN,
      &{model_token}, 1u);
  status = {preflight}(alias_inputs, alias_input_count,
      {out}, {out_size}, out_diagnostics, sizeof(*out_diagnostics));
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
"#
    );
}

fn render_manager_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    render_writes(output, projection, model, prefix, true)?;
    let target = model.target_name().as_str();
    let manager = manager_name(model)?;
    let filter = filter_name(model)?;
    let result = result_name(model)?;
    let filter_ref = filter_ref_name(model)?;
    let model_token = model_token_symbol(projection, prefix, model.id())?;
    let add_input = generated_symbol(prefix, "generated_alias_input_v1")?;
    let preflight = generated_symbol(prefix, "generated_alias_preflight_v1")?;
    let _ = write!(
        output,
        r#"
type_bridge_status_t TYPE_BRIDGE_CALL {manager}_open(
    const type_bridge_schema_package_t *package, {manager} *out_manager,
    type_bridge_execution_diagnostics_t **out_diagnostics) {{
  type_bridge_generated_opaque_input_v1_t alias_inputs[1];
  size_t alias_input_count = 0u;
  type_bridge_status_t status;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_SCHEMA_PACKAGE, package, 1u);
  status = {preflight}(alias_inputs, alias_input_count,
      out_manager, sizeof(*out_manager), out_diagnostics, sizeof(*out_diagnostics));
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  if (out_manager != NULL) {{
    out_manager->root_filter = NULL;
    out_manager->reserved[0] = out_manager->reserved[1] = 0u;
    out_manager->reserved[2] = out_manager->reserved[3] = 0u;
  }}
  if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}
  if (out_manager == NULL || out_diagnostics == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  return type_bridge_manager_filter_open(package, &{model_token}, &out_manager->root_filter, out_diagnostics);
}}

type_bridge_status_t TYPE_BRIDGE_CALL {manager}_close({manager} *manager) {{
  type_bridge_manager_filter_t *generic_filter;
  type_bridge_generated_opaque_input_v1_t alias_inputs[1];
  size_t alias_input_count = 0u;
  type_bridge_status_t status;
  if (manager == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  generic_filter = manager->root_filter;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_MANAGER_FILTER, generic_filter, 1u);
  status = {preflight}(alias_inputs, alias_input_count, manager, sizeof(*manager), NULL, 0u);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  if (manager->reserved[0] != 0u || manager->reserved[1] != 0u ||
      manager->reserved[2] != 0u || manager->reserved[3] != 0u) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  status = type_bridge_manager_filter_close(&generic_filter);
  manager->root_filter = generic_filter;
  return status;
}}

{filter_ref} TYPE_BRIDGE_CALL {manager}_filter_root(const {manager} *manager) {{
  {filter_ref} value = {{ NULL, {{0u, 0u, 0u, 0u}} }};
  if (manager != NULL) {{ value.value = manager->root_filter; }}
  return value;
}}
{filter_ref} TYPE_BRIDGE_CALL {manager}_filter_ref(const {filter} *filter) {{
  {filter_ref} value = {{ (const type_bridge_manager_filter_t *)filter, {{0u, 0u, 0u, 0u}} }};
  return value;
}}

type_bridge_status_t TYPE_BRIDGE_CALL {manager}_filter_close({filter} **filter) {{
  type_bridge_manager_filter_t *generic_filter;
  type_bridge_generated_opaque_input_v1_t alias_inputs[1];
  size_t alias_input_count = 0u;
  type_bridge_status_t status;
  if (filter == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  generic_filter = (type_bridge_manager_filter_t *)*filter;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_MANAGER_FILTER, generic_filter, 1u);
  status = {preflight}(alias_inputs, alias_input_count, filter, sizeof(*filter), NULL, 0u);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  status = type_bridge_manager_filter_close(&generic_filter);
  *filter = ({filter} *)generic_filter;
  return status;
}}
"#
    );

    for (suffix, arguments, call_arguments) in [
        ("iid", "const type_bridge_iid_t *iid", "iid"),
        (
            "iids",
            "const type_bridge_iid_t *const *iids, size_t iid_count",
            "iids, iid_count",
        ),
    ] {
        let _ = write!(
            output,
            r#"
type_bridge_status_t TYPE_BRIDGE_CALL {manager}_filter_{suffix}(
    const {manager} *manager, {filter_ref} source,
    {arguments}, {filter} **out_filter,
    type_bridge_execution_diagnostics_t **out_diagnostics) {{
  type_bridge_manager_filter_t *generic_filter = NULL;
"#
        );
        let extra = if suffix == "iid" {
            format!(
                "  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_IID, iid, 1u);\n"
            )
        } else {
            let _ = writeln!(
                output,
                "  size_t iid_index;\n  if (iid_count > SIZE_MAX / sizeof(*iids) || (iid_count != 0u && iids == NULL)) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}"
            );
            format!(
                "  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, iids, iid_count * sizeof(*iids));\n"
            )
        };
        input_fence(
            output,
            "source",
            &model_token,
            &add_input,
            &preflight,
            &extra,
            "out_filter",
        );
        if suffix == "iids" {
            let _ = write!(
                output,
                r#"
  for (iid_index = 0u; iid_index < iid_count; ++iid_index) {{
    const type_bridge_iid_t *iid;
    memcpy(&iid, (const unsigned char *)iids + iid_index * sizeof(*iids), sizeof(iid));
    alias_input_count = 0u;
    {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_IID, iid, 1u);
    status = {preflight}(alias_inputs, alias_input_count,
        out_filter, sizeof(*out_filter), out_diagnostics, sizeof(*out_diagnostics));
    if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  }}
"#
            );
        }
        let _ = write!(
            output,
            r#"
  if (out_filter != NULL) {{ *out_filter = NULL; }}
  if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}
  if (out_filter == NULL || out_diagnostics == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  status = type_bridge_manager_filter_validate_model(source.value, manager->root_filter, &{model_token}, out_diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  status = type_bridge_manager_filter_{suffix}(source.value, {call_arguments}, &generic_filter, out_diagnostics);
  if (status == TYPE_BRIDGE_STATUS_OK) {{ *out_filter = ({filter} *)generic_filter; }}
  return status;
}}
"#
        );
    }

    for field in model.query_tokens().fields().values() {
        let field_token = field_token_symbol(projection, prefix, model.id(), field.id())?;
        let attribute = projection
            .models()
            .get(&attribute_type_id(field.id())?)
            .expect("projected manager field attribute exists")
            .target_name()
            .as_str();
        for (operation, comparison) in OPERATIONS {
            let function = predicate_name(model, prefix, field, operation)?;
            let _ = write!(
                output,
                r#"
type_bridge_status_t TYPE_BRIDGE_CALL {function}(
    const {manager} *manager, {filter_ref} source,
    const {attribute} *value, {filter} **out_filter,
    type_bridge_execution_diagnostics_t **out_diagnostics) {{
  type_bridge_manager_filter_t *generic_filter = NULL;
"#
            );
            let extra = format!(
                r#"
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_VALUE, value, 1u);
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &{field_token}, 1u);
"#
            );
            input_fence(
                output,
                "source",
                &model_token,
                &add_input,
                &preflight,
                &extra,
                "out_filter",
            );
            let _ = write!(
                output,
                r#"
  if (out_filter != NULL) {{ *out_filter = NULL; }}
  if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}
  if (out_filter == NULL || out_diagnostics == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  status = type_bridge_manager_filter_validate_model(source.value, manager->root_filter, &{model_token}, out_diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  status = type_bridge_manager_filter_compare(source.value, &{field_token}, {comparison},
      (const type_bridge_projected_value_t *)value, &generic_filter, out_diagnostics);
  if (status == TYPE_BRIDGE_STATUS_OK) {{ *out_filter = ({filter} *)generic_filter; }}
  return status;
}}
"#
            );
        }
    }

    for (scope, owner, owner_kind) in [
        (
            "database",
            "type_bridge_database_t",
            "TYPE_BRIDGE_GENERATED_INPUT_DATABASE",
        ),
        (
            "read_transaction",
            "type_bridge_read_transaction_t",
            "TYPE_BRIDGE_GENERATED_INPUT_READ_TRANSACTION",
        ),
        (
            "write_transaction",
            "type_bridge_write_transaction_t",
            "TYPE_BRIDGE_GENERATED_INPUT_WRITE_TRANSACTION",
        ),
        (
            "proxy_connection",
            "type_bridge_proxy_connection_t",
            "TYPE_BRIDGE_GENERATED_INPUT_PROXY_CONNECTION",
        ),
    ] {
        for (operation, nominal, generic, zero) in [
            (
                "all",
                format!("{result} *"),
                "type_bridge_manager_result_t *",
                "NULL",
            ),
            (
                "first",
                format!("{target} *"),
                "type_bridge_projected_thing_t *",
                "NULL",
            ),
            ("count", "uint64_t".to_owned(), "uint64_t", "0u"),
            ("exists", "uint8_t".to_owned(), "uint8_t", "0u"),
        ] {
            let _ = write!(
                output,
                r#"
type_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_{operation}(
    const {owner} *owner, const {manager} *manager, {filter_ref} filter,
    const type_bridge_query_execution_limits_v1_t *limits,
    const type_bridge_cancellation_t *cancellation, {nominal} *out,
    type_bridge_execution_diagnostics_t **out_diagnostics) {{
  {generic} value = {zero};
"#
            );
            let extra = format!(
                r#"
  {add_input}(alias_inputs, &alias_input_count, {owner_kind}, owner, 1u);
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, limits, limits == NULL ? 0u : sizeof(*limits));
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION, cancellation, 1u);
"#
            );
            input_fence(
                output,
                "filter",
                &model_token,
                &add_input,
                &preflight,
                &extra,
                "out",
            );
            let _ = write!(
                output,
                r#"
  if (out != NULL) {{ *out = {zero}; }}
  if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}
  if (out == NULL || out_diagnostics == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  status = type_bridge_manager_filter_validate_model(filter.value, manager->root_filter, &{model_token}, out_diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  status = type_bridge_{scope}_manager_{operation}(owner, filter.value, limits, cancellation, &value, out_diagnostics);
  if (status == TYPE_BRIDGE_STATUS_OK) {{ *out = ({nominal})value; }}
  return status;
}}
"#
            );
        }
    }

    let _ = write!(
        output,
        r#"
type_bridge_status_t TYPE_BRIDGE_CALL {result}_count(
    const {result} *result, size_t *out_count,
    type_bridge_execution_diagnostics_t **out_diagnostics) {{
  return type_bridge_manager_result_count((const type_bridge_manager_result_t *)result,
      &{model_token}, out_count, out_diagnostics);
}}
type_bridge_status_t TYPE_BRIDGE_CALL {result}_at(
    const {result} *result, size_t index, {target} **out_value,
    type_bridge_execution_diagnostics_t **out_diagnostics) {{
  type_bridge_projected_thing_t *value = NULL;
  type_bridge_generated_opaque_input_v1_t alias_inputs[2];
  size_t alias_input_count = 0u;
  type_bridge_status_t status;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_MANAGER_RESULT, result, 1u);
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &{model_token}, 1u);
  status = {preflight}(alias_inputs, alias_input_count, out_value, sizeof(*out_value), out_diagnostics, sizeof(*out_diagnostics));
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  if (out_value != NULL) {{ *out_value = NULL; }}
  if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}
  if (out_value == NULL || out_diagnostics == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  status = type_bridge_manager_result_at((const type_bridge_manager_result_t *)result,
      &{model_token}, index, &value, out_diagnostics);
  if (status == TYPE_BRIDGE_STATUS_OK) {{ *out_value = ({target} *)value; value = NULL; }}
  (void)type_bridge_projected_thing_close(&value);
  return status;
}}
type_bridge_status_t TYPE_BRIDGE_CALL {result}_close({result} **result) {{
  type_bridge_manager_result_t *generic_result;
  type_bridge_generated_opaque_input_v1_t alias_inputs[1];
  size_t alias_input_count = 0u;
  type_bridge_status_t status;
  if (result == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  generic_result = (type_bridge_manager_result_t *)*result;
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_MANAGER_RESULT, generic_result, 1u);
  status = {preflight}(alias_inputs, alias_input_count, result, sizeof(*result), NULL, 0u);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  status = type_bridge_manager_result_close(&generic_result);
  *result = ({result} *)generic_result;
  return status;
}}
"#
    );
    Ok(())
}

fn render_writes(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
    definitions: bool,
) -> Result<(), Diagnostic> {
    let Some(create) = model.create().target_name() else {
        return Ok(());
    };
    let create = create.as_str();
    let target = model.target_name().as_str();
    let manager = manager_name(model)?;
    let filter_ref = filter_ref_name(model)?;
    let model_token = model_token_symbol(projection, prefix, model.id())?;
    let add_input = generated_symbol(prefix, "generated_alias_input_v1")?;
    let preflight = generated_symbol(prefix, "generated_alias_preflight_v1")?;
    for (scope, owner, owner_kind) in [
        (
            "database",
            "type_bridge_database_t",
            "TYPE_BRIDGE_GENERATED_INPUT_DATABASE",
        ),
        (
            "write_transaction",
            "type_bridge_write_transaction_t",
            "TYPE_BRIDGE_GENERATED_INPUT_WRITE_TRANSACTION",
        ),
        (
            "proxy_connection",
            "type_bridge_proxy_connection_t",
            "TYPE_BRIDGE_GENERATED_INPUT_PROXY_CONNECTION",
        ),
    ] {
        for operation in [
            "insert",
            "put",
            "update",
            "delete",
            "delete_returning",
            "delete_many_returning",
        ] {
            let selected = matches!(operation, "update" | "delete");
            let returns = operation != "delete";
            let returning = operation.ends_with("_returning");
            let many = operation == "delete_many_returning";
            let has_create = returns && !returning;
            let result_operation = if returning {
                "delete_returning"
            } else {
                operation
            };
            let result = format!("{target}_{result_operation}_batch_result");
            let _ = write!(
                output,
                "\ntype_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_{operation}(\n    const {owner} *owner, const {manager} *manager,"
            );
            if selected {
                let _ = write!(output, " {filter_ref} filter,");
            }
            if has_create {
                let _ = write!(output, "\n    const {create} *input,");
            } else if many {
                output.push_str("\n    const type_bridge_iid_t *const *iids, size_t iid_count,");
            } else if returning {
                output.push_str("\n    const type_bridge_iid_t *iid,");
            }
            output.push_str("\n    const type_bridge_query_execution_limits_v1_t *limits,\n    const type_bridge_cancellation_t *cancellation,");
            if returns {
                let _ = write!(output, " {result} **out,");
            }
            output.push_str("\n    type_bridge_execution_diagnostics_t **out_diagnostics)");
            if !definitions {
                output.push_str(";\n");
                continue;
            }
            output.push_str(" {\n");
            if returns {
                output.push_str("  type_bridge_projected_batch_result_t *value = NULL;\n");
            }
            // The root pointer is read only after fencing the complete manager below.
            if !selected {
                let _ = writeln!(
                    output,
                    "  {filter_ref} filter = {{ NULL, {{0u, 0u, 0u, 0u}} }};"
                );
            }
            if many {
                output.push_str("  size_t iid_index;\n  if (iid_count > SIZE_MAX / sizeof(*iids) || (iid_count != 0u && iids == NULL)) { return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }\n");
            }
            let mut extra = format!(
                r#"
  {add_input}(alias_inputs, &alias_input_count, {owner_kind}, owner, 1u);
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, limits, limits == NULL ? 0u : sizeof(*limits));
  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION, cancellation, 1u);
"#
            );
            if has_create {
                let _ = writeln!(
                    extra,
                    "  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_CREATE, input, 1u);"
                );
            } else if many {
                let _ = writeln!(
                    extra,
                    "  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, iids, iid_count * sizeof(*iids));"
                );
            } else if returning {
                let _ = writeln!(
                    extra,
                    "  {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_IID, iid, 1u);"
                );
            }
            input_fence(
                output,
                "filter",
                &model_token,
                &add_input,
                &preflight,
                &extra,
                if returns { "out" } else { "NULL" },
            );
            if many {
                let _ = write!(
                    output,
                    r#"
  for (iid_index = 0u; iid_index < iid_count; ++iid_index) {{
    const type_bridge_iid_t *iid;
    memcpy(&iid, (const unsigned char *)iids + iid_index * sizeof(*iids), sizeof(iid));
    alias_input_count = 0u;
    {add_input}(alias_inputs, &alias_input_count, TYPE_BRIDGE_GENERATED_INPUT_IID, iid, 1u);
    status = {preflight}(alias_inputs, alias_input_count, out, sizeof(*out), out_diagnostics, sizeof(*out_diagnostics));
    if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
  }}
"#
                );
            }
            if !selected {
                output.push_str("  filter.value = manager->root_filter;\n");
            }
            if returns {
                output.push_str("  if (out != NULL) { *out = NULL; }\n");
            }
            output.push_str("  if (out_diagnostics != NULL) { *out_diagnostics = NULL; }\n");
            if returns {
                output.push_str(
                    "  if (out == NULL) { return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }\n",
                );
            }
            let _ = write!(
                output,
                r#"
  if (out_diagnostics == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}
  status = type_bridge_manager_filter_validate_model(filter.value, manager->root_filter, &{model_token}, out_diagnostics);
  if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
"#
            );
            let selection = if selected {
                "filter.value".to_owned()
            } else {
                format!("&{model_token}")
            };
            let _ = write!(
                output,
                "  status = type_bridge_{scope}_manager_{operation}(owner, {selection},"
            );
            if has_create {
                output.push_str(" (const type_bridge_projected_create_t *)input,");
            } else if many {
                output.push_str(" iids, iid_count,");
            } else if returning {
                output.push_str(" iid,");
            }
            output.push_str(" limits, cancellation,");
            if returns {
                output.push_str(" &value,");
            }
            output.push_str(" out_diagnostics);\n");
            if returns {
                let _ = writeln!(
                    output,
                    "  if (status == TYPE_BRIDGE_STATUS_OK) {{ *out = ({result} *)value; }}"
                );
            }
            output.push_str("  return status;\n}\n");
        }
    }
    Ok(())
}
