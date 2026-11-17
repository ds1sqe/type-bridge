//! Borrowed nominal markers; the native recording owns compilation and staging.
use super::*;

pub(super) fn symbols(projection: &RuntimeProjection) -> Result<Vec<String>, Diagnostic> {
    let mut symbols = Vec::new();
    for model in models(projection) {
        for suffix in [
            "plan_t",
            "pending_t",
            "result_t",
            "row",
            "column",
            "delete",
            "delete_returning",
            "stage",
            "pending_result",
            "pending_free",
            "result_count",
            "result_free",
        ] {
            symbols.push(name(model, suffix)?);
        }
        if has_values(model) {
            for suffix in ["insert_many", "put_many"] {
                symbols.push(name(model, suffix)?);
            }
        }
        if has_insert(model) {
            for suffix in ["insert", "put"] {
                symbols.push(name(model, suffix)?);
            }
        }
    }
    Ok(symbols)
}
pub(super) fn render_types(
    output: &mut String,
    projection: &RuntimeProjection,
) -> Result<(), Diagnostic> {
    output.push_str("/* A plan marker borrows its recording until that recording is freed.\n * The first typed stage freezes the recording. Pending and result owners\n * retain the compiled source and may outlive the recording. */\n");
    for model in models(projection) {
        let plan = name(model, "plan_t")?;
        let pending = name(model, "pending_t")?;
        let result = name(model, "result_t")?;
        let _ = writeln!(output, "typedef struct {plan} {{ type_bridge_plan_builder_t *recording; uint32_t selection; uint32_t _reserved; }} {plan};
            typedef struct {pending} {pending};
            typedef struct {result} {result};
#ifdef __cplusplus
static_assert(sizeof({plan}) == 16u, \"plan marker size\");
static_assert(alignof({plan}) == 8u, \"plan marker alignment\");
#else
_Static_assert(sizeof({plan}) == 16u, \"plan marker size\");
_Static_assert(_Alignof({plan}) == 8u, \"plan marker alignment\");
#endif");
    }
    Ok(())
}
pub(super) fn render_functions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let [add, preflight] = generated_alias_helper_symbols(prefix)?;
    for model in models(projection) {
        let handle = name(model, "handle_t")?;
        let plan = name(model, "plan_t")?;
        let pending = name(model, "pending_t")?;
        let result = name(model, "result_t")?;
        let row = name(model, "row")?;
        let _ = writeln!(output, "/* Project this handle as a model in the plan's answer. */
            static inline type_bridge_status_t {row}(type_bridge_plan_builder_t *p, {handle} h, {plan} *out) {{
                type_bridge_generated_opaque_input_v1_t inputs[2]; size_t count = 0u; type_bridge_status_t status; uint32_t selection = 0u;
                {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
                {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, h.recording, 1u);
                status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u);
                if (status != TYPE_BRIDGE_STATUS_OK) return status;
                memset(out, 0, sizeof(*out));
                status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                status = type_bridge_plan_builder_hold(p, 3u, h.binding, h.model_ordinal, &selection);
                if (status == TYPE_BRIDGE_STATUS_OK) {{ {plan} held; memset(&held, 0, sizeof(held)); held.recording = p; held.selection = selection; memcpy(out, &held, sizeof(held)); }}
                return status;
            }}");
        if has_insert(model) {
            let create = name(model, "plan_create_handle_checked")?;
            let insert = name(model, "insert_t")?;
            for (suffix, put) in [("insert", "false"), ("put", "true")] {
                let function = name(model, suffix)?;
                let _ = writeln!(output, "static inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, const {insert} *values, {plan} *out) {{
                    {handle} h; type_bridge_status_t status;
                    if (out == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
                    status = {create}(p, {put}, values, &h, out, sizeof(*out));
                    if (status != TYPE_BRIDGE_STATUS_OK) return status;
                    return {row}(p, h, out);
                }}");
            }
        }
        if has_values(model) {
            render_bulk(output, projection, model, prefix)?;
        }
        let column = name(model, "column")?;
        let _ = writeln!(
            output,
            "static inline type_bridge_plan_column_t {column}({handle} h) {{ return type_bridge_plan_model_column(h.recording, h.binding, h.model_ordinal); }}"
        );
        let delete = name(model, "delete")?;
        let snapshot = name(model, "delete_returning")?;
        for (function, terminal, matched) in [
            (&delete, "type_bridge_matched_plan_t", true),
            (&snapshot, &plan, false),
        ] {
            let _ = writeln!(output, "static inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, {handle} h, {terminal} *out) {{
                type_bridge_generated_opaque_input_v1_t inputs[2]; size_t count = 0u; type_bridge_status_t status; uint32_t selection = 0u;
                {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
                {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, h.recording, 1u);
                status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                memset(out, 0, sizeof(*out));
                status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                status = type_bridge_plan_builder_delete(p, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                status = type_bridge_plan_builder_hold(p, {}u, h.binding, h.model_ordinal, &selection);
                if (status == TYPE_BRIDGE_STATUS_OK) {{ {terminal} held; memset(&held, 0, sizeof(held)); held.recording = p; held.selection = selection; memcpy(out, &held, sizeof(held)); }}
                return status;
            }}", if matched { 0 } else { 3 });
        }
        let stage = name(model, "stage")?;
        let get = name(model, "pending_result")?;
        let free_pending = name(model, "pending_free")?;
        let count = name(model, "result_count")?;
        let free = name(model, "result_free")?;
        let _ = writeln!(output, "static inline type_bridge_status_t {stage}(type_bridge_staged_transaction_t *tx, {plan} plan, {pending} **out, type_bridge_execution_diagnostics_t **diagnostics) {{ return type_bridge_staged_transaction_stage_selection(tx, plan.recording, plan.selection, (type_bridge_pending_t **)out, diagnostics); }}
            static inline type_bridge_status_t {get}(const {pending} *pending, {result} **out, type_bridge_execution_diagnostics_t **diagnostics) {{ return type_bridge_pending_plan_result((const type_bridge_pending_t *)pending, (type_bridge_plan_result_t **)out, diagnostics); }}
            static inline void {free_pending}({pending} **pending) {{ type_bridge_pending_free((type_bridge_pending_t **)pending); }}
            static inline size_t {count}(const {result} *result) {{ size_t count = 0u; (void)type_bridge_plan_result_count((const type_bridge_plan_result_t *)result, &count); return count; }}
            static inline void {free}({result} **result) {{ type_bridge_plan_result_free((type_bridge_plan_result_t **)result); }}");
    }
    Ok(())
}

fn render_bulk(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let [add, preflight] = generated_alias_helper_symbols(prefix)?;
    let plan = name(model, "plan_t")?;
    let handle = name(model, "handle_t")?;
    let values = name(model, "values_t")?;
    let row = name(model, "row")?;
    let bind = name(model, "plan_bind_rows")?;
    let token = model_token_symbol(projection, prefix, model.id())?;
    let opener = generated_symbol(prefix, "plan_open")?;
    let text_fields = model
        .create()
        .fields()
        .iter()
        .filter_map(|create| {
            let field = &model.query_tokens().fields()[create.token()];
            (!matches!(
                query::field_domain(projection, field),
                Ok(ValueTypeTag::Long | ValueTypeTag::Double | ValueTypeTag::Boolean)
            ))
            .then_some((create, field))
        })
        .collect::<Vec<_>>();
    let capacity = 2 + text_fields.len();
    for (suffix, native) in [
        ("insert_many", "type_bridge_plan_builder_insert"),
        ("put_many", "type_bridge_plan_builder_put"),
    ] {
        let function = name(model, suffix)?;
        let _ = writeln!(output, "/* Rows and present strings are borrowed only during this call. */
            static inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, const {values} *rows, size_t row_count, {plan} *out) {{
                type_bridge_generated_opaque_input_v1_t inputs[{capacity}]; size_t count = 0u; size_t index; type_bridge_status_t status; uint32_t model = 0u; {handle} h;
                if (row_count > SIZE_MAX / sizeof(*rows) || (row_count != 0u && rows == NULL)) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
                {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
                {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, rows, row_count * sizeof(*rows));
                status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                for (index = 0u; index < row_count; ++index) {{ count = 0u;");
        for (create, field) in &text_fields {
            let member = member(prefix, field)?;
            let present = if create.multiplicity().required() {
                "true".to_owned()
            } else {
                format!("rows[index].has_{member}")
            };
            let _ = writeln!(
                output,
                "if ({present} && rows[index].{member} != NULL) {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, rows[index].{member}, strlen(rows[index].{member}) + 1u);"
            );
        }
        let _ = writeln!(output, "status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u); if (status != TYPE_BRIDGE_STATUS_OK) return status; }}
            memset(out, 0, sizeof(*out)); memset(&h, 0, sizeof(h)); h.recording = p; h.model_ordinal = {token}.ordinal;
            {{ type_bridge_byte_view_t opener = {{ (const uint8_t *){opener:?}, {}u }}; status = type_bridge_plan_builder_type_token(p, &{token}, opener, &model); }}
            if (status != TYPE_BRIDGE_STATUS_OK) return status;
            status = {native}(p, model, &h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;
            status = {bind}(p, h, rows, row_count); if (status != TYPE_BRIDGE_STATUS_OK) return status;
            return {row}(p, h, out);
        }}", opener.len());
    }
    Ok(())
}
