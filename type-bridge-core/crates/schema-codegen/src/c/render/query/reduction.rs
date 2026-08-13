use std::collections::BTreeSet;
use std::fmt::Write as _;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{ModelProjection, ProjectedContainer, RuntimeProjection};
use type_bridge_contract::value::ValueTypeTag;

use super::{QUERY_MODES, field_base, model_name, package_name, query_models};
use crate::c::render::{
    field_token_symbol, generated_symbol, model_token_symbol, projected_attribute_model,
};

const RESULT_FAMILIES: [(&str, &str, &str, &str); 3] = [
    (
        "reduction",
        "reduce",
        "TYPE_BRIDGE_QUERY_TERMINAL_REDUCE",
        "TYPE_BRIDGE_QUERY_RESULT_REDUCTION",
    ),
    (
        "field_reduction",
        "reduce_field",
        "TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELD",
        "TYPE_BRIDGE_QUERY_RESULT_FIELD_REDUCTION",
    ),
    (
        "field_tuple_reduction",
        "reduce_fields",
        "TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELDS",
        "TYPE_BRIDGE_QUERY_RESULT_FIELD_TUPLE_REDUCTION",
    ),
];

pub(super) fn remote_families() -> impl Iterator<Item = (&'static str, &'static str, &'static str)>
{
    RESULT_FAMILIES.into_iter().map(|(family, _, terminal, _)| {
        let result = match family {
            "reduction" => "query_reduction_result",
            "field_reduction" => "query_field_reduction_result",
            "field_tuple_reduction" => "query_field_tuple_reduction_result",
            _ => unreachable!("closed reduction result family"),
        };
        (family, terminal, result)
    })
}

fn root_ref(prefix: &str) -> Result<String, Diagnostic> {
    package_name(prefix, "query_root_v1_t")
}

fn reducer_ref(prefix: &str) -> Result<String, Diagnostic> {
    package_name(prefix, "query_reducer_ref_v1_t")
}

fn group_ref(prefix: &str) -> Result<String, Diagnostic> {
    package_name(prefix, "query_binding_group_ref_v1_t")
}

fn field_group_ref(prefix: &str) -> Result<String, Diagnostic> {
    package_name(prefix, "query_field_group_ref_v1_t")
}

fn reduction_result_ref(prefix: &str) -> Result<String, Diagnostic> {
    package_name(prefix, "query_reduction_result_ref_v1_t")
}

fn value_slot(prefix: &str, kind: &str) -> Result<String, Diagnostic> {
    package_name(prefix, &format!("query_reduced_{kind}_slot_v1_t"))
}

fn group_slot(model: &ModelProjection, mode: &str, prefix: &str) -> Result<String, Diagnostic> {
    let _ = prefix;
    model_name(model, &format!("query_{mode}_reduction_group_slot_v1_t"))
}

fn field_group_slot(base: &str) -> Result<String, Diagnostic> {
    generated_symbol(base, "reduction_group_slot_v1_t")
}

fn field_is_scalar_numeric(
    projection: &RuntimeProjection,
    field: &type_bridge_contract::projection::FieldTokenProjection,
) -> Result<Option<ValueTypeTag>, Diagnostic> {
    if field.multiplicity().container() != ProjectedContainer::Scalar {
        return Ok(None);
    }
    let attribute = projected_attribute_model(
        projection,
        &type_bridge_contract::projection::ProjectedTypeRef::Model(
            type_bridge_contract::projection::ProjectedModelUse::new(
                super::super::attribute_type_id(field.id())?,
                type_bridge_contract::projection::ProjectedModelForm::Complete,
            ),
        ),
    )?;
    Ok(match attribute.declaration().value_type() {
        Some(value @ (ValueTypeTag::Long | ValueTypeTag::Double)) => Some(value),
        _ => None,
    })
}

pub(super) fn nominal_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for suffix in [
        "query_reduction_terminal",
        "query_field_reduction_terminal",
        "query_field_tuple_reduction_terminal",
        "query_reduction_result",
        "query_field_reduction_result",
        "query_field_tuple_reduction_result",
    ] {
        names.insert(package_name(prefix, suffix)?);
    }
    for kind in ["count", "long", "double"] {
        names.insert(value_slot(prefix, kind)?);
    }
    for model in query_models(projection) {
        for (mode, _) in QUERY_MODES {
            names.insert(group_slot(model, mode, prefix)?);
        }
        for field in model.query_tokens().fields().values() {
            names.insert(field_group_slot(&field_base(model, prefix, field)?)?);
        }
    }
    Ok(names)
}

pub(super) fn auxiliary_type_names(prefix: &str) -> Result<BTreeSet<String>, Diagnostic> {
    Ok(BTreeSet::from([
        reducer_ref(prefix)?,
        group_ref(prefix)?,
        field_group_ref(prefix)?,
        reduction_result_ref(prefix)?,
    ]))
}

pub(super) fn function_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for suffix in [
        "query_reducer_count",
        "query_reduce",
        "query_reduce_grouped",
        "query_reduce_field",
        "query_reduce_fields",
        "query_reduction_terminal_close",
        "query_field_reduction_terminal_close",
        "query_field_tuple_reduction_terminal_close",
        "database_query_execute_reduction",
        "database_query_execute_field_reduction",
        "database_query_execute_field_tuple_reduction",
        "read_transaction_query_execute_reduction",
        "read_transaction_query_execute_field_reduction",
        "read_transaction_query_execute_field_tuple_reduction",
        "query_reduction_result_ref",
        "query_field_reduction_result_ref",
        "query_field_tuple_reduction_result_ref",
        "query_reduction_result_row_count",
        "query_reduction_group_kind",
        "query_reduction_group_field_count",
        "query_reduced_count_slot",
        "query_reduced_long_slot",
        "query_reduced_double_slot",
        "query_reduced_count_value",
        "query_reduced_long_metadata",
        "query_reduced_long_value",
        "query_reduced_double_metadata",
        "query_reduced_double_bits",
        "query_reduction_result_close",
        "query_field_reduction_result_close",
        "query_field_tuple_reduction_result_close",
    ] {
        names.insert(package_name(prefix, suffix)?);
    }
    for model in query_models(projection) {
        for (mode, _) in QUERY_MODES {
            let slot = group_slot(model, mode, prefix)?;
            names.insert(generated_symbol(&slot, "from")?);
            let thing = generated_symbol(&slot, "thing")?;
            names.insert(generated_symbol(&thing, "at")?);
            names.insert(thing);
        }
        for field in model.query_tokens().fields().values() {
            let base = field_base(model, prefix, field)?;
            let slot = field_group_slot(&base)?;
            names.insert(generated_symbol(&slot, "from")?);
            names.insert(generated_symbol(&slot, "from_result")?);
            names.insert(generated_symbol(&slot, "value")?);
            if let Some(value_type) = field_is_scalar_numeric(projection, field)? {
                for reducer in ["sum", "min", "max", "mean", "median", "std"] {
                    let result = match (value_type, reducer) {
                        (ValueTypeTag::Long, "sum" | "min" | "max") => "long",
                        _ => "double",
                    };
                    names.insert(generated_symbol(
                        &base,
                        &format!("reduce_{reducer}_{result}"),
                    )?);
                }
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
    let reducer = reducer_ref(prefix)?;
    let group = group_ref(prefix)?;
    let field_group = field_group_ref(prefix)?;
    let result_ref = reduction_result_ref(prefix)?;
    let root = root_ref(prefix)?;
    let query = package_name(prefix, "query")?;

    let _ = write!(
        output,
        "\n/* Typed reduction witnesses. These values borrow their referenced query\n\
         * handles; they never own or independently close them. Result refs and\n\
         * result-derived group/value slots remain valid only until their source\n\
         * result is closed. */\n\
         typedef type_bridge_query_reducer_v1_t {reducer};\n\
         typedef struct {group} {{\n\
           const type_bridge_query_binding_t *binding;\n\
           const type_bridge_projected_token_v1_t *expected_model;\n\
           type_bridge_query_match_mode_t expected_mode;\n\
           uint32_t reserved[4];\n\
         }} {group};\n\
         typedef type_bridge_query_field_reference_v1_t {field_group};\n\
         typedef struct {result_ref} {{\n\
           const type_bridge_query_result_t *handle;\n\
           type_bridge_query_result_kind_t expected_kind;\n\
           uint32_t reserved[4];\n\
         }} {result_ref};\n\
         {reducer} TYPE_BRIDGE_CALL {prefix}_query_reducer_count(void);\n",
    );

    for kind in ["count", "long", "double"] {
        let slot = value_slot(prefix, kind)?;
        let _ = write!(
            output,
            "struct {slot} {{\n\
               const type_bridge_query_result_t *handle;\n\
               type_bridge_query_result_kind_t expected_result_kind;\n\
               size_t value_index;\n\
               type_bridge_query_reduced_value_kind_t expected_value_kind;\n\
               uint64_t reserved[4];\n\
             }};\n",
        );
    }

    for model in query_models(projection) {
        let model_token = model_token_symbol(projection, prefix, model.id())?;
        for (mode, _) in QUERY_MODES {
            let binding = model_name(model, &format!("query_{mode}_binding"))?;
            let slot = group_slot(model, mode, prefix)?;
            let from = generated_symbol(&slot, "from")?;
            let thing = generated_symbol(&slot, "thing")?;
            let result = if mode == "exact" {
                model.target_name().as_str().to_owned()
            } else {
                model_name(model, "query_subtypes_result")?
            };
            let _ = write!(
                output,
                "struct {slot} {{\n\
                   const type_bridge_query_result_t *handle;\n\
                   type_bridge_query_result_kind_t expected_result_kind;\n\
                   const type_bridge_projected_token_v1_t *expected_model;\n\
                   type_bridge_query_match_mode_t expected_mode;\n\
                   uint64_t reserved[4];\n\
                 }};\n\
                 {group} TYPE_BRIDGE_CALL {from}(const {binding} *binding);\n\
                 {slot} TYPE_BRIDGE_CALL {thing}(\n\
                   {result_ref} result);\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {thing}_at(\n\
                   {slot} group, size_t row_index, {result} **out_value,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n",
            );
            let _ = model_token;
        }
        for field in model.query_tokens().fields().values() {
            let base = field_base(model, prefix, field)?;
            let slot = field_group_slot(&base)?;
            let attribute = projected_attribute_model(
                projection,
                &type_bridge_contract::projection::ProjectedTypeRef::Model(
                    type_bridge_contract::projection::ProjectedModelUse::new(
                        super::super::attribute_type_id(field.id())?,
                        type_bridge_contract::projection::ProjectedModelForm::Complete,
                    ),
                ),
            )?;
            let _ = write!(
                output,
                "struct {slot} {{\n\
                   const type_bridge_query_result_t *handle;\n\
                   type_bridge_query_result_kind_t expected_result_kind;\n\
                   size_t field_index;\n\
                   const type_bridge_projected_token_v1_t *expected_field;\n\
                   uint64_t reserved[4];\n\
                 }};\n\
                 {field_group} TYPE_BRIDGE_CALL {slot}_from(const {base} *field);\n\
                 {slot} TYPE_BRIDGE_CALL {slot}_from_result(\n\
                   {result_ref} result, size_t field_index);\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {slot}_value(\n\
                   {slot} group, size_t row_index, {} **out_value,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n",
                attribute.target_name().as_str(),
            );
            if let Some(value_type) = field_is_scalar_numeric(projection, field)? {
                for (operation, constant) in [
                    ("sum", "TYPE_BRIDGE_QUERY_REDUCER_SUM"),
                    ("min", "TYPE_BRIDGE_QUERY_REDUCER_MIN"),
                    ("max", "TYPE_BRIDGE_QUERY_REDUCER_MAX"),
                    ("mean", "TYPE_BRIDGE_QUERY_REDUCER_MEAN"),
                    ("median", "TYPE_BRIDGE_QUERY_REDUCER_MEDIAN"),
                    ("std", "TYPE_BRIDGE_QUERY_REDUCER_STD"),
                ] {
                    let value_kind = match (value_type, operation) {
                        (ValueTypeTag::Long, "sum" | "min" | "max") => "long",
                        _ => "double",
                    };
                    let function =
                        generated_symbol(&base, &format!("reduce_{operation}_{value_kind}"))?;
                    let _ = writeln!(
                        output,
                        "{reducer} TYPE_BRIDGE_CALL {function}(const {base} *field);"
                    );
                    let _ = constant;
                }
            }
        }
    }

    let reduction_terminal = package_name(prefix, "query_reduction_terminal")?;
    let field_terminal = package_name(prefix, "query_field_reduction_terminal")?;
    let tuple_terminal = package_name(prefix, "query_field_tuple_reduction_terminal")?;
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce(\n\
           {root} root,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {reduction_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce_grouped(\n\
           {root} root, {group} group,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {reduction_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce_field(\n\
           {root} root, {field_group} group,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {field_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce_fields(\n\
           {root} root, const {field_group} *groups, size_t group_count,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {tuple_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n",
    );
    render_common_declarations(output, prefix, &result_ref)?;
    let _ = query;
    Ok(())
}

fn render_common_declarations(
    output: &mut String,
    prefix: &str,
    result_ref: &str,
) -> Result<(), Diagnostic> {
    for (family, _, _, _) in RESULT_FAMILIES {
        let terminal = package_name(prefix, &format!("query_{family}_terminal"))?;
        let result = package_name(prefix, &format!("query_{family}_result"))?;
        let _ = writeln!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_terminal_close({terminal} **terminal);"
        );
        for (scope, owner) in [
            ("database", "type_bridge_database_t"),
            ("read_transaction", "type_bridge_read_transaction_t"),
        ] {
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_{scope}_query_execute_{family}(\n\
                   const {owner} *owner, const {terminal} *terminal,\n\
                   const type_bridge_query_execution_limits_v1_t *limits,\n\
                   const type_bridge_cancellation_t *cancellation,\n\
                   {result} **out_result,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics);\n",
            );
        }
        let _ = write!(
            output,
            "{result_ref} TYPE_BRIDGE_CALL {prefix}_query_{family}_result_ref(\n\
               const {result} *result);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_result_close(\n\
               {result} **result);\n",
        );
    }
    let count_slot = value_slot(prefix, "count")?;
    let long_slot = value_slot(prefix, "long")?;
    let double_slot = value_slot(prefix, "double")?;
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduction_result_row_count(\n\
           {result_ref} result, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduction_group_kind(\n\
           {result_ref} result, size_t row_index,\n\
           type_bridge_query_reduction_group_kind_t *out_kind,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduction_group_field_count(\n\
           {result_ref} result, size_t row_index, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         {count_slot} TYPE_BRIDGE_CALL {prefix}_query_reduced_count_slot(\n\
           {result_ref} result, size_t value_index);\n\
         {long_slot} TYPE_BRIDGE_CALL {prefix}_query_reduced_long_slot(\n\
           {result_ref} result, size_t value_index);\n\
         {double_slot} TYPE_BRIDGE_CALL {prefix}_query_reduced_double_slot(\n\
           {result_ref} result, size_t value_index);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_count_value(\n\
           {count_slot} slot, size_t row_index, uint64_t *out_value,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_long_metadata(\n\
           {long_slot} slot, size_t row_index,\n\
           type_bridge_query_reduced_value_metadata_v1_t *out_metadata,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_long_value(\n\
           {long_slot} slot, size_t row_index, int64_t *out_value,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_double_metadata(\n\
           {double_slot} slot, size_t row_index,\n\
           type_bridge_query_reduced_value_metadata_v1_t *out_metadata,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_double_bits(\n\
           {double_slot} slot, size_t row_index, uint64_t *out_bits,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics);\n",
    );
    Ok(())
}

pub(super) fn render_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    render_reducer_definitions(output, projection, prefix)?;
    render_terminal_definitions(output, prefix)?;
    render_result_definitions(output, projection, prefix)
}

fn render_reducer_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let reducer = reducer_ref(prefix)?;
    let group = group_ref(prefix)?;
    let field_group = field_group_ref(prefix)?;
    let result_ref = reduction_result_ref(prefix)?;
    let _ = write!(
        output,
        "{reducer} TYPE_BRIDGE_CALL {prefix}_query_reducer_count(void) {{\n\
           {reducer} value = {{\n\
             sizeof(type_bridge_query_reducer_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, TYPE_BRIDGE_QUERY_REDUCER_COUNT,\n\
             0u, NULL, NULL, {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return value;\n\
         }}\n\n",
    );
    for model in query_models(projection) {
        let model_token = model_token_symbol(projection, prefix, model.id())?;
        for (mode, mode_constant) in QUERY_MODES {
            let binding = model_name(model, &format!("query_{mode}_binding"))?;
            let slot = group_slot(model, mode, prefix)?;
            let from = generated_symbol(&slot, "from")?;
            let thing = generated_symbol(&slot, "thing")?;
            let target = if mode == "exact" {
                model.target_name().as_str().to_owned()
            } else {
                model_name(model, "query_subtypes_result")?
            };
            let _ = write!(
                output,
                "{group} TYPE_BRIDGE_CALL {from}(const {binding} *binding) {{\n\
                   {group} value = {{\n\
                     (const type_bridge_query_binding_t *)binding, &{model_token},\n\
                     {mode_constant}, {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n\
                 {slot} TYPE_BRIDGE_CALL {thing}({result_ref} result) {{\n\
                   {slot} value = {{\n\
                     result.handle, result.expected_kind, &{model_token}, {mode_constant},\n\
                     {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {thing}_at(\n\
                   {slot} group, size_t row_index, {target} **out_value,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_result_reduction_group_thing(\n\
                       group.handle, group.expected_result_kind, row_index,\n\
                       group.expected_model, group.expected_mode,\n\
                       (type_bridge_projected_thing_t **)out_value, out_diagnostics);\n\
                 }}\n\n",
            );
        }
        for field in model.query_tokens().fields().values() {
            let base = field_base(model, prefix, field)?;
            let slot = field_group_slot(&base)?;
            let token = field_token_symbol(projection, prefix, model.id(), field.id())?;
            let attribute = projected_attribute_model(
                projection,
                &type_bridge_contract::projection::ProjectedTypeRef::Model(
                    type_bridge_contract::projection::ProjectedModelUse::new(
                        super::super::attribute_type_id(field.id())?,
                        type_bridge_contract::projection::ProjectedModelForm::Complete,
                    ),
                ),
            )?;
            let _ = write!(
                output,
                "{field_group} TYPE_BRIDGE_CALL {slot}_from(const {base} *field) {{\n\
                   {field_group} value = {{\n\
                     sizeof(type_bridge_query_field_reference_v1_t),\n\
                     TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION,\n\
                     (const type_bridge_query_field_t *)field, &{token},\n\
                     {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n\
                 {slot} TYPE_BRIDGE_CALL {slot}_from_result(\n\
                   {result_ref} result, size_t field_index) {{\n\
                   {slot} value = {{\n\
                     result.handle, result.expected_kind, field_index, &{token},\n\
                     {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n\
                 type_bridge_status_t TYPE_BRIDGE_CALL {slot}_value(\n\
                   {slot} group, size_t row_index, {} **out_value,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_result_reduction_group_field_at(\n\
                       group.handle, group.expected_result_kind, row_index,\n\
                       group.field_index, group.expected_field,\n\
                       (type_bridge_projected_value_t **)out_value, out_diagnostics);\n\
                 }}\n\n",
                attribute.target_name().as_str(),
            );
            if let Some(value_type) = field_is_scalar_numeric(projection, field)? {
                for (operation, constant) in [
                    ("sum", "TYPE_BRIDGE_QUERY_REDUCER_SUM"),
                    ("min", "TYPE_BRIDGE_QUERY_REDUCER_MIN"),
                    ("max", "TYPE_BRIDGE_QUERY_REDUCER_MAX"),
                    ("mean", "TYPE_BRIDGE_QUERY_REDUCER_MEAN"),
                    ("median", "TYPE_BRIDGE_QUERY_REDUCER_MEDIAN"),
                    ("std", "TYPE_BRIDGE_QUERY_REDUCER_STD"),
                ] {
                    let value_kind = match (value_type, operation) {
                        (ValueTypeTag::Long, "sum" | "min" | "max") => "long",
                        _ => "double",
                    };
                    let function =
                        generated_symbol(&base, &format!("reduce_{operation}_{value_kind}"))?;
                    let _ = write!(
                        output,
                        "{reducer} TYPE_BRIDGE_CALL {function}(const {base} *field) {{\n\
                           {reducer} value = {{\n\
                             sizeof(type_bridge_query_reducer_v1_t),\n\
                             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, {constant}, 0u,\n\
                             (const type_bridge_query_field_t *)field, &{token},\n\
                             {{ 0u, 0u, 0u, 0u }}\n\
                           }};\n\
                           return value;\n\
                         }}\n\n",
                    );
                }
            }
        }
    }
    Ok(())
}

fn render_terminal_definitions(output: &mut String, prefix: &str) -> Result<(), Diagnostic> {
    let root = root_ref(prefix)?;
    let group = group_ref(prefix)?;
    let field_group = field_group_ref(prefix)?;
    let reducer = reducer_ref(prefix)?;
    let reduction_terminal = package_name(prefix, "query_reduction_terminal")?;
    let field_terminal = package_name(prefix, "query_field_reduction_terminal")?;
    let tuple_terminal = package_name(prefix, "query_field_tuple_reduction_terminal")?;
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce(\n\
           {root} root,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {reduction_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
             sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, TYPE_BRIDGE_QUERY_TERMINAL_REDUCE,\n\
             TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY, root.root, root.expected_model,\n\
             root.expected_mode, 0u, NULL, 0u, 0u, 0u, 0u,\n\
             {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
             NULL, NULL, 0u, 0u, NULL, 0u, reducers, reducer_count,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return type_bridge_query_terminal_open_v1(\n\
               root.query, &descriptor,\n\
               (type_bridge_query_terminal_t **)out_terminal, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce_grouped(\n\
           {root} root, {group} group,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {reduction_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
             sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, TYPE_BRIDGE_QUERY_TERMINAL_REDUCE,\n\
             TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY, root.root, root.expected_model,\n\
             root.expected_mode, 0u, NULL, 0u, 0u, 0u, 0u,\n\
             {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
             group.binding, group.expected_model, group.expected_mode, 0u,\n\
             NULL, 0u, reducers, reducer_count, {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return type_bridge_query_terminal_open_v1(\n\
               root.query, &descriptor,\n\
               (type_bridge_query_terminal_t **)out_terminal, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce_field(\n\
           {root} root, {field_group} group,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {field_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
             sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION,\n\
             TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELD,\n\
             TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY, root.root, root.expected_model,\n\
             root.expected_mode, 0u, NULL, 0u, 0u, 0u, 0u,\n\
             {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
             NULL, NULL, 0u, 0u, &group, 1u, reducers, reducer_count,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return type_bridge_query_terminal_open_v1(\n\
               root.query, &descriptor,\n\
               (type_bridge_query_terminal_t **)out_terminal, out_diagnostics);\n\
         }}\n\n",
    );

    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduce_fields(\n\
           {root} root, const {field_group} *groups, size_t group_count,\n\
           const {reducer} *reducers, size_t reducer_count,\n\
           {tuple_terminal} **out_terminal,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
             sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION,\n\
             TYPE_BRIDGE_QUERY_TERMINAL_REDUCE_FIELDS,\n\
             TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY, root.root, root.expected_model,\n\
             root.expected_mode, 0u, NULL, 0u, 0u, 0u, 0u,\n\
             {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
             NULL, NULL, 0u, 0u, groups, group_count, reducers, reducer_count,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return type_bridge_query_terminal_open_v1(\n\
               root.query, &descriptor,\n\
               (type_bridge_query_terminal_t **)out_terminal, out_diagnostics);\n\
         }}\n\n",
    );
    Ok(())
}

fn render_result_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let result_ref = reduction_result_ref(prefix)?;
    for (family, _, terminal_kind, result_kind) in RESULT_FAMILIES {
        let terminal = package_name(prefix, &format!("query_{family}_terminal"))?;
        let result = package_name(prefix, &format!("query_{family}_result"))?;
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
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_{scope}_query_execute_{family}(\n\
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
        let _ = write!(
            output,
            "{result_ref} TYPE_BRIDGE_CALL {prefix}_query_{family}_result_ref(\n\
               const {result} *result) {{\n\
               {result_ref} value = {{\n\
                 (const type_bridge_query_result_t *)result, {result_kind},\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_terminal_close(\n\
               {terminal} **terminal) {{\n\
               return type_bridge_query_terminal_close(\n\
                   (type_bridge_query_terminal_t **)terminal);\n\
             }}\n\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_{family}_result_close(\n\
               {result} **result) {{\n\
               return type_bridge_query_result_close(\n\
                   (type_bridge_query_result_t **)result);\n\
             }}\n\n",
        );
    }

    let count_slot = value_slot(prefix, "count")?;
    let long_slot = value_slot(prefix, "long")?;
    let double_slot = value_slot(prefix, "double")?;
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduction_result_row_count(\n\
           {result_ref} result, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_row_count(\n\
               result.handle, result.expected_kind, out_count, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduction_group_kind(\n\
           {result_ref} result, size_t row_index,\n\
           type_bridge_query_reduction_group_kind_t *out_kind,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_group_kind(\n\
               result.handle, result.expected_kind, row_index, out_kind, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduction_group_field_count(\n\
           {result_ref} result, size_t row_index, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_group_field_count(\n\
               result.handle, result.expected_kind, row_index, out_count, out_diagnostics);\n\
         }}\n\n",
    );
    for (kind, slot, constant) in [
        ("count", &count_slot, "TYPE_BRIDGE_QUERY_REDUCED_COUNT"),
        ("long", &long_slot, "TYPE_BRIDGE_QUERY_REDUCED_LONG"),
        ("double", &double_slot, "TYPE_BRIDGE_QUERY_REDUCED_DOUBLE"),
    ] {
        let _ = write!(
            output,
            "{slot} TYPE_BRIDGE_CALL {prefix}_query_reduced_{kind}_slot(\n\
               {result_ref} result, size_t value_index) {{\n\
               {slot} value = {{\n\
                 result.handle, result.expected_kind, value_index, {constant},\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               return value;\n\
             }}\n\n",
        );
    }
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_count_value(\n\
           {count_slot} slot, size_t row_index, uint64_t *out_value,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_value_count(\n\
               slot.handle, slot.expected_result_kind, row_index, slot.value_index,\n\
               out_value, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_long_metadata(\n\
           {long_slot} slot, size_t row_index,\n\
           type_bridge_query_reduced_value_metadata_v1_t *out_metadata,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_value_metadata_v1(\n\
               slot.handle, slot.expected_result_kind, row_index, slot.value_index,\n\
               slot.expected_value_kind, out_metadata, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_long_value(\n\
           {long_slot} slot, size_t row_index, int64_t *out_value,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_value_long(\n\
               slot.handle, slot.expected_result_kind, row_index, slot.value_index,\n\
               out_value, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_double_metadata(\n\
           {double_slot} slot, size_t row_index,\n\
           type_bridge_query_reduced_value_metadata_v1_t *out_metadata,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_value_metadata_v1(\n\
               slot.handle, slot.expected_result_kind, row_index, slot.value_index,\n\
               slot.expected_value_kind, out_metadata, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {prefix}_query_reduced_double_bits(\n\
           {double_slot} slot, size_t row_index, uint64_t *out_bits,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_reduction_value_double_bits(\n\
               slot.handle, slot.expected_result_kind, row_index, slot.value_index,\n\
               out_bits, out_diagnostics);\n\
         }}\n\n",
    );
    let _ = projection;
    Ok(())
}
