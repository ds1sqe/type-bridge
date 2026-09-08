use std::collections::BTreeSet;
use std::fmt::Write as _;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::projection::{ModelProjection, RuntimeProjection};

use super::super::{
    attribute_type_id, field_token_symbol, generated_symbol, model_token_symbol,
    projected_member_suffix,
};
use super::{field_base, model_name, package_name, query_models};

const OPERATIONS: [(&str, &str); 6] = [
    ("eq", "TYPE_BRIDGE_QUERY_COMPARE_EQUAL"),
    ("ne", "TYPE_BRIDGE_QUERY_COMPARE_NOT_EQUAL"),
    ("lt", "TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN"),
    ("lte", "TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN_OR_EQUAL"),
    ("gt", "TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN"),
    ("gte", "TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL"),
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
            "database_all",
            "database_first",
            "database_count",
            "database_exists",
            "read_transaction_all",
            "read_transaction_first",
            "read_transaction_count",
            "read_transaction_exists",
        ] {
            names.insert(generated_symbol(&manager, suffix)?);
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
        let binding = model_name(model, "query_exact_binding")?;
        let query = package_name(prefix, "query")?;
        let _ = write!(
            output,
            "\n/* Exact-model immutable manager/filter facade for `{target}`. */\n\
             struct {manager} {{\n\
               {binding} *binding;\n\
               {query} *root_query;\n\
               uint64_t reserved[4];\n\
             }};\n\
             typedef struct {filter_ref} {{\n\
               const {query} *query;\n\
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

fn render_manager_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let target = model.target_name().as_str();
    let manager = manager_name(model)?;
    let filter = filter_name(model)?;
    let result = result_name(model)?;
    let filter_ref = filter_ref_name(model)?;
    let session = package_name(prefix, "query_session")?;
    let binding = model_name(model, "query_exact_binding")?;
    let selection = model_name(model, "query_exact_one_selection")?;
    let query = package_name(prefix, "query")?;
    let predicate = package_name(prefix, "query_predicate")?;
    let model_token = model_token_symbol(projection, prefix, model.id())?;
    let add_input = generated_symbol(prefix, "generated_alias_input_v1")?;
    let preflight = generated_symbol(prefix, "generated_alias_preflight_v1")?;

    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {manager}_open(\n\
           const type_bridge_schema_package_t *package, {manager} *out_manager,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           {session} *session = NULL;\n\
           {binding} *binding = NULL;\n\
           {selection} *selection = NULL;\n\
           {query} *root_query = NULL;\n\
           type_bridge_execution_diagnostics_t *diagnostics = NULL;\n\
           type_bridge_generated_opaque_input_v1_t alias_inputs[1];\n\
           size_t alias_input_count = 0u;\n\
           type_bridge_status_t status;\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_SCHEMA_PACKAGE, package, 1u);\n\
           status = {preflight}(alias_inputs, alias_input_count,\n\
               out_manager, out_manager == NULL ? 0u : sizeof(*out_manager),\n\
               out_diagnostics, sizeof(*out_diagnostics));\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           if (out_manager != NULL) {{\n\
             out_manager->binding = NULL;\n\
             out_manager->root_query = NULL;\n\
             out_manager->reserved[0] = 0u;\n\
             out_manager->reserved[1] = 0u;\n\
             out_manager->reserved[2] = 0u;\n\
             out_manager->reserved[3] = 0u;\n\
           }}\n\
           if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}\n\
           if (package == NULL || out_manager == NULL || out_diagnostics == NULL) {{\n\
             return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
           }}\n\
           status = {prefix}_query_session_open(package, &session, &diagnostics);\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
           status = {binding}_open(session, &binding, &diagnostics);\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
           status = {selection}_open(binding, &selection, &diagnostics);\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
           status = {prefix}_query_positional_1(session, {selection}_ref(selection),\n\
               &root_query, &diagnostics);\n\
         cleanup:\n\
           if ({selection}_close(&selection) != TYPE_BRIDGE_STATUS_OK &&\n\
               status == TYPE_BRIDGE_STATUS_OK) {{ status = TYPE_BRIDGE_STATUS_PANIC; }}\n\
           if ({prefix}_query_session_close(&session) != TYPE_BRIDGE_STATUS_OK &&\n\
               status == TYPE_BRIDGE_STATUS_OK) {{ status = TYPE_BRIDGE_STATUS_PANIC; }}\n\
           if (status == TYPE_BRIDGE_STATUS_OK) {{\n\
             out_manager->binding = binding;\n\
             out_manager->root_query = root_query;\n\
             binding = NULL;\n\
             root_query = NULL;\n\
           }}\n\
           {prefix}_query_close(&root_query);\n\
           {binding}_close(&binding);\n\
           *out_diagnostics = diagnostics;\n\
           return status;\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {manager}_close({manager} *manager) {{\n\
           type_bridge_query_binding_t *binding;\n\
           type_bridge_query_t *root_query;\n\
           type_bridge_generated_opaque_input_v1_t alias_inputs[2];\n\
           size_t alias_input_count = 0u;\n\
           type_bridge_status_t status;\n\
           type_bridge_status_t binding_status;\n\
           if (manager == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}\n\
           binding = (type_bridge_query_binding_t *)manager->binding;\n\
           root_query = (type_bridge_query_t *)manager->root_query;\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_QUERY_BINDING, binding, 1u);\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_QUERY, root_query, 1u);\n\
           status = {preflight}(alias_inputs, alias_input_count,\n\
               manager, sizeof(*manager), NULL, 0u);\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           if (manager->reserved[0] != 0u || manager->reserved[1] != 0u ||\n\
               manager->reserved[2] != 0u || manager->reserved[3] != 0u) {{\n\
             return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
           }}\n\
           status = type_bridge_query_close(&root_query);\n\
           manager->root_query = ({query} *)root_query;\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           binding_status = type_bridge_query_binding_close(&binding);\n\
           manager->binding = ({binding} *)binding;\n\
           return status == TYPE_BRIDGE_STATUS_OK ? binding_status : status;\n\
         }}\n\n\
         {filter_ref} TYPE_BRIDGE_CALL {manager}_filter_root(\n\
           const {manager} *manager) {{\n\
           {filter_ref} value = {{ NULL, {{ 0u, 0u, 0u, 0u }} }};\n\
           if (manager != NULL) {{ value.query = manager->root_query; }}\n\
           return value;\n\
         }}\n\n\
         {filter_ref} TYPE_BRIDGE_CALL {manager}_filter_ref(\n\
           const {filter} *filter) {{\n\
           {filter_ref} value = {{ (const {query} *)filter, {{ 0u, 0u, 0u, 0u }} }};\n\
           return value;\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {manager}_filter_close(\n\
           {filter} **filter) {{\n\
           {query} *query;\n\
           type_bridge_generated_opaque_input_v1_t alias_inputs[1];\n\
           size_t alias_input_count = 0u;\n\
           type_bridge_status_t status;\n\
           if (filter == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}\n\
           query = ({query} *)*filter;\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_QUERY, query, 1u);\n\
           status = {preflight}(alias_inputs, alias_input_count,\n\
               filter, sizeof(*filter), NULL, 0u);\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           status = {prefix}_query_close(&query);\n\
           *filter = ({filter} *)query;\n\
           return status;\n\
         }}\n\n"
    );

    for field in model.query_tokens().fields().values() {
        let field_type = field_base(model, prefix, field)?;
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
                "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
                   const {manager} *manager, {filter_ref} source,\n\
                   const {attribute} *value, {filter} **out_filter,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   {binding} *binding = NULL;\n\
                   {query} *root_query = NULL;\n\
                   {query} *query = NULL;\n\
                   {field_type} *field = NULL;\n\
                   {predicate} *predicate = NULL;\n\
                   type_bridge_execution_diagnostics_t *diagnostics = NULL;\n\
                   type_bridge_generated_opaque_input_v1_t alias_inputs[5];\n\
                   size_t alias_input_count = 0u;\n\
                   type_bridge_status_t status;\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_BYTES, manager,\n\
                       manager == NULL ? 0u : sizeof(*manager));\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_QUERY, source.query, 1u);\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_VALUE, value, 1u);\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &{model_token}, 1u);\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &{field_token}, 1u);\n\
                   status = {preflight}(alias_inputs, alias_input_count,\n\
                       out_filter, sizeof(*out_filter),\n\
                       out_diagnostics, sizeof(*out_diagnostics));\n\
                   if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
                   if (manager != NULL &&\n\
                       (manager->reserved[0] != 0u || manager->reserved[1] != 0u ||\n\
                        manager->reserved[2] != 0u || manager->reserved[3] != 0u)) {{\n\
                     return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
                   }}\n\
                   if (source.reserved[0] != 0u || source.reserved[1] != 0u ||\n\
                       source.reserved[2] != 0u || source.reserved[3] != 0u) {{\n\
                     return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
                   }}\n\
                   if (manager != NULL) {{\n\
                     binding = manager->binding;\n\
                     root_query = manager->root_query;\n\
                   }}\n\
                   alias_input_count = 0u;\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_QUERY_BINDING, binding, 1u);\n\
                   {add_input}(alias_inputs, &alias_input_count,\n\
                       TYPE_BRIDGE_GENERATED_INPUT_QUERY, root_query, 1u);\n\
                   status = {preflight}(alias_inputs, alias_input_count,\n\
                       out_filter, sizeof(*out_filter),\n\
                       out_diagnostics, sizeof(*out_diagnostics));\n\
                   if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
                   if (out_filter != NULL) {{ *out_filter = NULL; }}\n\
                   if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}\n\
                   if (manager == NULL || out_filter == NULL || out_diagnostics == NULL ||\n\
                       binding == NULL || root_query == NULL || source.query == NULL) {{\n\
                     return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
                   }}\n\
                   status = {field_type}_from_exact(binding, &field, &diagnostics);\n\
                   if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
                   status = {field_type}_compare_value(\n\
                       field, {comparison}, value, &predicate, &diagnostics);\n\
                   if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
                   status = {prefix}_query_where(\n\
                       source.query, predicate, &query, &diagnostics);\n\
                 cleanup:\n\
                   if ({prefix}_query_predicate_close(&predicate) != TYPE_BRIDGE_STATUS_OK &&\n\
                       status == TYPE_BRIDGE_STATUS_OK) {{ status = TYPE_BRIDGE_STATUS_PANIC; }}\n\
                   if ({field_type}_close(&field) != TYPE_BRIDGE_STATUS_OK &&\n\
                       status == TYPE_BRIDGE_STATUS_OK) {{ status = TYPE_BRIDGE_STATUS_PANIC; }}\n\
                   if (status == TYPE_BRIDGE_STATUS_OK) {{\n\
                     *out_filter = ({filter} *)query;\n\
                     query = NULL;\n\
                   }}\n\
                   {prefix}_query_close(&query);\n\
                   *out_diagnostics = diagnostics;\n\
                   return status;\n\
                 }}\n\n"
            );
        }
    }

    for (scope, owner, owner_kind, execute) in [
        (
            "database",
            "type_bridge_database_t",
            "TYPE_BRIDGE_GENERATED_INPUT_DATABASE",
            "type_bridge_database_query_execute_v1",
        ),
        (
            "read_transaction",
            "type_bridge_read_transaction_t",
            "TYPE_BRIDGE_GENERATED_INPUT_READ_TRANSACTION",
            "type_bridge_read_transaction_query_execute_v1",
        ),
    ] {
        render_terminal_definitions(
            output,
            model,
            prefix,
            &manager,
            &filter_ref,
            &result,
            &binding,
            &query,
            &model_token,
            &add_input,
            &preflight,
            scope,
            owner,
            owner_kind,
            execute,
        );
    }

    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {result}_count(\n\
           const {result} *result, size_t *out_count,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_result_row_count(\n\
               (const type_bridge_query_result_t *)result, TYPE_BRIDGE_QUERY_RESULT_ROWS,\n\
               out_count, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {result}_at(\n\
           const {result} *result, size_t index, {target} **out_value,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_projected_thing_t *generic_value = NULL;\n\
           type_bridge_execution_diagnostics_t *diagnostics = NULL;\n\
           type_bridge_generated_opaque_input_v1_t alias_inputs[2];\n\
           size_t alias_input_count = 0u;\n\
           type_bridge_status_t status;\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_QUERY_RESULT, result, 1u);\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &{model_token}, 1u);\n\
           status = {preflight}(alias_inputs, alias_input_count,\n\
               out_value, out_value == NULL ? 0u : sizeof(*out_value),\n\
               out_diagnostics,\n\
               out_diagnostics == NULL ? 0u : sizeof(*out_diagnostics));\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           if (out_value != NULL) {{ *out_value = NULL; }}\n\
           if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}\n\
           if (out_value == NULL || out_diagnostics == NULL) {{\n\
             return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
           }}\n\
           status = type_bridge_query_result_row_slot_thing_at(\n\
               (const type_bridge_query_result_t *)result, TYPE_BRIDGE_QUERY_RESULT_ROWS,\n\
               index, 0u, 0u, &{model_token}, TYPE_BRIDGE_QUERY_MATCH_EXACT,\n\
               TYPE_BRIDGE_QUERY_SELECTION_ONE,\n\
               &generic_value, &diagnostics);\n\
           if (status == TYPE_BRIDGE_STATUS_OK) {{\n\
             *out_value = ({target} *)generic_value;\n\
             generic_value = NULL;\n\
           }}\n\
           (void)type_bridge_projected_thing_close(&generic_value);\n\
           *out_diagnostics = diagnostics;\n\
           return status;\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {result}_close({result} **result) {{\n\
           type_bridge_query_result_t *generic_result;\n\
           type_bridge_generated_opaque_input_v1_t alias_inputs[1];\n\
           size_t alias_input_count = 0u;\n\
           type_bridge_status_t status;\n\
           if (result == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}\n\
           generic_result = (type_bridge_query_result_t *)*result;\n\
           {add_input}(alias_inputs, &alias_input_count,\n\
               TYPE_BRIDGE_GENERATED_INPUT_QUERY_RESULT, generic_result, 1u);\n\
           status = {preflight}(alias_inputs, alias_input_count,\n\
               result, sizeof(*result), NULL, 0u);\n\
           if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
           status = type_bridge_query_result_close(&generic_result);\n\
           *result = ({result} *)generic_result;\n\
           return status;\n\
         }}\n\n"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_terminal_definitions(
    output: &mut String,
    model: &ModelProjection,
    _prefix: &str,
    manager: &str,
    filter_ref: &str,
    result: &str,
    binding: &str,
    query: &str,
    model_token: &str,
    add_input: &str,
    preflight: &str,
    scope: &str,
    owner: &str,
    owner_kind: &str,
    execute: &str,
) {
    let target = model.target_name().as_str();
    for (operation, terminal_kind, cardinality, limit, output_type, output_name) in [
        (
            "all",
            "TYPE_BRIDGE_QUERY_TERMINAL_ROWS",
            "TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY",
            "0u",
            format!("{result} **"),
            "out_result",
        ),
        (
            "first",
            "TYPE_BRIDGE_QUERY_TERMINAL_FIRST",
            "TYPE_BRIDGE_QUERY_ROWS_BOUNDED_MANY",
            "0u",
            format!("{target} **"),
            "out_value",
        ),
        (
            "count",
            "TYPE_BRIDGE_QUERY_TERMINAL_COUNT",
            "TYPE_BRIDGE_QUERY_ROWS_EXACTLY_ONE",
            "0u",
            "uint64_t *".to_owned(),
            "out_count",
        ),
        (
            "exists",
            "TYPE_BRIDGE_QUERY_TERMINAL_EXISTS",
            "TYPE_BRIDGE_QUERY_ROWS_EXACTLY_ONE",
            "0u",
            "uint8_t *".to_owned(),
            "out_exists",
        ),
    ] {
        let scalar_declarations = match operation {
            "first" => "  type_bridge_projected_thing_t *thing = NULL;\n  size_t row_count = 0u;\n",
            "count" => "  type_bridge_projected_thing_t *thing = NULL;\n  uint64_t scalar = 0u;\n",
            "exists" => "  type_bridge_projected_thing_t *thing = NULL;\n  uint8_t scalar = 0u;\n",
            _ => "  type_bridge_projected_thing_t *thing = NULL;\n",
        };
        let consume = match operation {
            "first" => format!(
                "  status = type_bridge_query_result_row_count(\n\
                     generic_result, TYPE_BRIDGE_QUERY_RESULT_ROWS, &row_count, &diagnostics);\n\
                   if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
                   if (row_count > 1u) {{ status = TYPE_BRIDGE_STATUS_EXECUTION_FAILED; goto cleanup; }}\n\
                   if (row_count == 1u) {{\n\
                     status = type_bridge_query_result_row_slot_thing_at(\n\
                         generic_result, TYPE_BRIDGE_QUERY_RESULT_ROWS, 0u, 0u, 0u,\n\
                         &{model_token}, TYPE_BRIDGE_QUERY_MATCH_EXACT,\n\
                         TYPE_BRIDGE_QUERY_SELECTION_ONE, &thing, &diagnostics);\n\
                     if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
                   }}\n"
            ),
            "count" => "  status = type_bridge_query_result_count(generic_result, &scalar, &diagnostics);\n  if (status != TYPE_BRIDGE_STATUS_OK) { goto cleanup; }\n".to_owned(),
            "exists" => "  status = type_bridge_query_result_exists(generic_result, &scalar, &diagnostics);\n  if (status != TYPE_BRIDGE_STATUS_OK) { goto cleanup; }\n".to_owned(),
            _ => String::new(),
        };
        let publish = match operation {
            "all" => {
                format!("  *out_result = ({result} *)generic_result;\n  generic_result = NULL;\n")
            }
            "first" => format!("  *out_value = ({target} *)thing;\n  thing = NULL;\n"),
            "count" => "  *out_count = scalar;\n".to_owned(),
            "exists" => "  *out_exists = scalar;\n".to_owned(),
            _ => unreachable!(),
        };
        let scalar_default = match operation {
            "count" => format!("if ({output_name} != NULL) {{ *{output_name} = 0u; }}"),
            "exists" => format!("if ({output_name} != NULL) {{ *{output_name} = 0u; }}"),
            _ => format!("if ({output_name} != NULL) {{ *{output_name} = NULL; }}"),
        };
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {manager}_{scope}_{operation}(\n\
               const {owner} *owner, const {manager} *manager, {filter_ref} filter,\n\
               const type_bridge_query_execution_limits_v1_t *limits,\n\
               const type_bridge_cancellation_t *cancellation,\n\
               {output_type}{output_name},\n\
               type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
               {binding} *binding = NULL;\n\
               {query} *root_query = NULL;\n\
               type_bridge_query_terminal_t *terminal = NULL;\n\
               type_bridge_query_result_t *generic_result = NULL;\n\
             {scalar_declarations}\
               type_bridge_execution_diagnostics_t *diagnostics = NULL;\n\
               type_bridge_generated_opaque_input_v1_t alias_inputs[6];\n\
               size_t alias_input_count = 0u;\n\
               type_bridge_status_t status;\n\
               type_bridge_query_terminal_descriptor_v1_t descriptor = {{\n\
                 sizeof(type_bridge_query_terminal_descriptor_v1_t),\n\
                 TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, {terminal_kind}, {cardinality},\n\
                 NULL, &{model_token}, TYPE_BRIDGE_QUERY_MATCH_EXACT, 0u,\n\
                 NULL, 0u, 0u, {limit}, 0u,\n\
                 {{ 0u, 0u, 0u, 0u, 0u, 0u, 0u }},\n\
                 NULL, NULL, 0u, 0u, NULL, 0u, NULL, 0u,\n\
                 {{ 0u, 0u, 0u, 0u }}\n\
               }};\n\
               {add_input}(alias_inputs, &alias_input_count, {owner_kind}, owner, 1u);\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_BYTES, manager,\n\
                   manager == NULL ? 0u : sizeof(*manager));\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_QUERY, filter.query, 1u);\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_BYTES, limits,\n\
                   limits == NULL ? 0u : sizeof(*limits));\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_CANCELLATION, cancellation, 1u);\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_PROJECTED_TOKEN, &{model_token}, 1u);\n\
               status = {preflight}(alias_inputs, alias_input_count,\n\
                   {output_name}, sizeof(*{output_name}),\n\
                   out_diagnostics, sizeof(*out_diagnostics));\n\
               if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
               if (manager != NULL &&\n\
                   (manager->reserved[0] != 0u || manager->reserved[1] != 0u ||\n\
                    manager->reserved[2] != 0u || manager->reserved[3] != 0u)) {{\n\
                 return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
               }}\n\
               if (filter.reserved[0] != 0u || filter.reserved[1] != 0u ||\n\
                   filter.reserved[2] != 0u || filter.reserved[3] != 0u) {{\n\
                 return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;\n\
               }}\n\
               if (manager != NULL) {{\n\
                 binding = manager->binding;\n\
                 root_query = manager->root_query;\n\
               }}\n\
               alias_input_count = 0u;\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_QUERY_BINDING, binding, 1u);\n\
               {add_input}(alias_inputs, &alias_input_count,\n\
                   TYPE_BRIDGE_GENERATED_INPUT_QUERY, root_query, 1u);\n\
               status = {preflight}(alias_inputs, alias_input_count,\n\
                   {output_name}, sizeof(*{output_name}),\n\
                   out_diagnostics, sizeof(*out_diagnostics));\n\
               if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}\n\
               {scalar_default}\n\
               if (out_diagnostics != NULL) {{ *out_diagnostics = NULL; }}\n\
               if (owner == NULL || manager == NULL || {output_name} == NULL ||\n\
                   out_diagnostics == NULL || binding == NULL || root_query == NULL ||\n\
                   filter.query == NULL) {{ return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT; }}\n\
               descriptor.root = (const type_bridge_query_binding_t *)binding;\n\
               status = type_bridge_query_terminal_open_v1(\n\
                   (const type_bridge_query_t *)filter.query, &descriptor,\n\
                   &terminal, &diagnostics);\n\
               if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
               status = {execute}(owner, terminal, {terminal_kind}, limits, cancellation,\n\
                   &generic_result, &diagnostics);\n\
               if (status != TYPE_BRIDGE_STATUS_OK) {{ goto cleanup; }}\n\
             {consume}\
             cleanup:\n\
               if (type_bridge_query_terminal_close(&terminal) != TYPE_BRIDGE_STATUS_OK &&\n\
                   status == TYPE_BRIDGE_STATUS_OK) {{ status = TYPE_BRIDGE_STATUS_PANIC; }}\n\
               if (status == TYPE_BRIDGE_STATUS_OK) {{\n\
             {publish}\
               }}\n\
               type_bridge_projected_thing_close(&thing);\n\
               type_bridge_query_result_close(&generic_result);\n\
               *out_diagnostics = diagnostics;\n\
               return status;\n\
             }}\n\n",
            scalar_declarations = scalar_declarations,
            consume = consume,
            publish = publish,
            scalar_default = scalar_default,
        );
    }
}
