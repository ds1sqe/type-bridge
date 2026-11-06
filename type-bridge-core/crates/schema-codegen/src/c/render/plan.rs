//! Nominal C authoring wrappers; the native recorder owns all plan rules.
use super::*;
mod roles;
mod terminals;
mod views;
use type_bridge_contract::projection::FieldTokenProjection;

fn models(projection: &RuntimeProjection) -> impl Iterator<Item = &ModelProjection> {
    projection
        .models()
        .values()
        .filter(|model| matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation))
}
fn name(model: &ModelProjection, suffix: &str) -> Result<String, Diagnostic> {
    generated_symbol(model.target_name().as_str(), suffix)
}
const AUTHORING_KEYWORDS: &[&str] = &[
    "_Alignas",
    "_Alignof",
    "_Atomic",
    "_Bool",
    "_Complex",
    "_Generic",
    "_Imaginary",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "auto",
    "bitand",
    "bitor",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "char16_t",
    "char32_t",
    "class",
    "compl",
    "const",
    "const_cast",
    "constexpr",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "dynamic_cast",
    "else",
    "enum",
    "explicit",
    "export",
    "extern",
    "false",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "register",
    "reinterpret_cast",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "static_cast",
    "struct",
    "switch",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typedef",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "wchar_t",
    "while",
    "xor",
    "xor_eq",
];
fn raw_member<'a>(prefix: &str, field: &'a FieldTokenProjection) -> Result<&'a str, Diagnostic> {
    projected_member_suffix(prefix, field.target_name())
}
fn member(prefix: &str, field: &FieldTokenProjection) -> Result<String, Diagnostic> {
    let raw = raw_member(prefix, field)?;
    // Encoded physical labels escape every z, so a keyword's zk suffix is distinct.
    Ok(if AUTHORING_KEYWORDS.contains(&raw) {
        format!("{raw}zk")
    } else {
        raw.to_owned()
    })
}
fn domain(
    projection: &RuntimeProjection,
    field: &FieldTokenProjection,
) -> Result<&'static str, Diagnostic> {
    Ok(match query::field_domain(projection, field)? {
        ValueTypeTag::Boolean => "bool",
        ValueTypeTag::Long => "long",
        ValueTypeTag::Double => "double",
        ValueTypeTag::String => "string",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime_tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    })
}
fn writable(field: &FieldTokenProjection) -> bool {
    !field.is_key() && field.multiplicity().container() == ProjectedContainer::Scalar
}

fn has_values(model: &ModelProjection) -> bool {
    model.id().kind() == TypeKind::Entity && crate::supports_scalar_plan_writes(model)
}
fn has_insert(model: &ModelProjection) -> bool {
    crate::supports_scalar_plan_writes(model)
}
fn render_owned_members(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
    members: &mut BTreeSet<String>,
) -> Result<(), Diagnostic> {
    for create in model.create().fields() {
        let field = &model.query_tokens().fields()[create.token()];
        let member = member(prefix, field)?;
        if !members.insert(member.clone()) {
            return Err(invalid(
                "c_emitter_generated_symbol_collision",
                "generated C struct members collide",
            ));
        }
        let _ = writeln!(output, "{} {member};", host_type(projection, field)?);
        if !create.multiplicity().required() {
            let presence = bounded_generated_identifier(&format!("has_{member}"))?;
            if !members.insert(presence.clone()) {
                return Err(invalid(
                    "c_emitter_generated_symbol_collision",
                    "generated C struct members collide",
                ));
            }
            let _ = writeln!(output, "bool {presence};");
        }
    }
    Ok(())
}
fn host_type(
    projection: &RuntimeProjection,
    field: &FieldTokenProjection,
) -> Result<&'static str, Diagnostic> {
    Ok(match query::field_domain(projection, field)? {
        ValueTypeTag::Long => "int64_t",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "bool",
        _ => "const char *",
    })
}

pub(super) fn runtime_symbols() -> BTreeSet<String> {
    let mut names = BTreeSet::from(["tb_cond".to_owned(), "tb_cond_t".to_owned()]);
    for domain in [
        "long",
        "double",
        "bool",
        "string",
        "date",
        "datetime",
        "datetime_tz",
        "decimal",
        "duration",
    ] {
        names.insert(format!("tb_{domain}"));
        names.insert(format!("tb_{domain}_t"));
        names.insert(format!("tb_{domain}_column"));
        for comparison in ["eq", "ne", "lt", "le", "gt", "ge"] {
            names.insert(format!("tb_{domain}_{comparison}"));
        }
        if matches!(domain, "long" | "double") {
            for operation in ["add", "sub", "mul", "div"] {
                names.insert(format!("tb_{domain}_{operation}"));
            }
        }
    }
    for comparison in ["contains", "starts_with", "ends_with", "like"] {
        names.insert(format!("tb_string_{comparison}"));
    }
    names
}

pub(super) fn symbols(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<Vec<String>, Diagnostic> {
    let mut symbols = Vec::new();
    for model in models(projection) {
        for suffix in [
            "handle_t",
            "match_t",
            "set_t",
            "unset_t",
            "plan_handle_init",
            "match",
            "match_by",
            "subtypes",
            "subtypes_by",
            "by_iid",
            "where",
            "set",
            "unset",
        ] {
            symbols.push(name(model, suffix)?);
        }
        if has_values(model) {
            symbols.push(name(model, "values_t")?);
            symbols.push(name(model, "plan_bind_rows")?);
        }
        if has_insert(model) {
            symbols.push(name(model, "insert_t")?);
            symbols.push(name(model, "plan_create_handle")?);
            symbols.push(name(model, "plan_create_handle_checked")?);
        }
        for field in model.query_tokens().fields().values() {
            symbols.push(name(
                model,
                &format!("plan_{}", raw_member(prefix, field)?),
            )?);
        }
    }
    symbols.extend(roles::symbols(projection, prefix)?);
    symbols.extend(terminals::symbols(projection)?);
    symbols.extend(views::symbols(projection, prefix)?);
    Ok(symbols)
}

pub(super) fn render(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let [add_input, preflight] = generated_alias_helper_symbols(prefix)?;
    output.push_str("\n/* Plan-value getters use <model>_plan_<field>; hydrated getters retain <model>_<field>. */\n");
    for model in models(projection) {
        let handle = name(model, "handle_t")?;
        let _ = writeln!(output, "typedef struct {handle} {{ type_bridge_plan_builder_t *recording; uint32_t binding; uint32_t model_ordinal; }} {handle};
#ifdef __cplusplus
static_assert(sizeof({handle}) == 16u, \"plan handle size\");
static_assert(alignof({handle}) == 8u, \"plan handle alignment\");
#else
_Static_assert(sizeof({handle}) == 16u, \"plan handle size\");
_Static_assert(_Alignof({handle}) == 8u, \"plan handle alignment\");
#endif");
    }
    roles::render_types(output, projection, prefix)?;
    terminals::render_types(output, projection)?;
    views::render_types(output, projection, prefix)?;
    for model in models(projection) {
        let matched = name(model, "match_t")?;
        let set = name(model, "set_t")?;
        let unset = name(model, "unset_t")?;
        let fields = model.query_tokens().fields();
        if has_values(model) {
            let values = name(model, "values_t")?;
            let _ = writeln!(output, "typedef struct {values} {{");
            render_owned_members(output, projection, model, prefix, &mut BTreeSet::new())?;
            if model.create().fields().is_empty() {
                output.push_str("unsigned char reserved;\n");
            }
            let _ = writeln!(output, "}} {values};");
        }
        if has_insert(model) {
            let insert = name(model, "insert_t")?;
            if has_values(model) {
                let values = name(model, "values_t")?;
                let _ = writeln!(output, "typedef {values} {insert};");
            } else {
                let _ = writeln!(
                    output,
                    "/* Input arrays and strings are borrowed only during the insert/put call. */\ntypedef struct {insert} {{"
                );
                let mut members = BTreeSet::new();
                render_owned_members(output, projection, model, prefix, &mut members)?;
                roles::render_create_members(output, projection, model, prefix, &mut members)?;
                if members.is_empty() {
                    output.push_str("unsigned char reserved;\n");
                }
                let _ = writeln!(output, "}} {insert};");
            }
        }
        if let Some(field) = fields.values().find(|field| writable(field)) {
            let member = member(prefix, field)?;
            let _ = writeln!(
                output,
                "/* Zero means no change; ({unset}){{ .{member} = true }} removes that ownership. */"
            );
        }
        let _ = writeln!(output, "typedef struct {unset} {{");
        let mut count = 0;
        for field in fields.values().filter(|field| writable(field)) {
            let _ = writeln!(output, "bool {};", member(prefix, field)?);
            count += 1;
        }
        if count == 0 {
            output.push_str("unsigned char reserved;\n");
        }
        let _ = writeln!(output, "}} {unset};");
        for (ty, only_writable) in [(&matched, false), (&set, true)] {
            let _ = writeln!(output, "typedef struct {ty} {{");
            let mut count = 0;
            for field in fields
                .values()
                .filter(|field| !only_writable || writable(field))
            {
                let _ = writeln!(
                    output,
                    "tb_{}_t {};",
                    domain(projection, field)?,
                    member(prefix, field)?
                );
                count += 1;
            }
            if count == 0 {
                output.push_str("unsigned char reserved;\n");
            }
            let _ = writeln!(output, "}} {ty};");
        }
    }
    for model in models(projection) {
        let handle = name(model, "handle_t")?;
        let matched = name(model, "match_t")?;
        let set = name(model, "set_t")?;
        let init = name(model, "plan_handle_init")?;
        let token = model_token_symbol(projection, prefix, model.id())?;
        let opener = generated_symbol(prefix, "plan_open")?;
        let fields = model.query_tokens().fields();
        let _ = writeln!(output, "static inline type_bridge_status_t {init}(type_bridge_plan_builder_t *p, bool subtypes, {handle} *out) {{
          type_bridge_generated_opaque_input_v1_t inputs[1]; size_t count = 0u;
          type_bridge_status_t status; uint32_t model = 0u; {handle} value;
          memset(&value, 0, sizeof(value));
          {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
          status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u);
          if (status != TYPE_BRIDGE_STATUS_OK) return status;
          memset(out, 0, sizeof(*out));
          if (p == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
          value.recording = p;
          {{ type_bridge_byte_view_t opener = {{(const uint8_t *){opener:?}, {}}};
             status = type_bridge_plan_builder_type_token(p, &{token}, opener, &model); }}
          if (status != TYPE_BRIDGE_STATUS_OK) return status;
          status = subtypes ? type_bridge_plan_builder_subtypes(p, model, &value.binding) : type_bridge_plan_builder_exact(p, model, &value.binding);
          if (status != TYPE_BRIDGE_STATUS_OK) return status;", opener.len());
        let _ = writeln!(output, "value.model_ordinal = {token}.ordinal;");
        output.push_str("status = type_bridge_plan_builder_check_handle(p, p, value.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status; memcpy(out, &value, sizeof(value)); return status;\n}\n");
        for (index, field) in fields.values().enumerate() {
            let getter = name(model, &format!("plan_{}", raw_member(prefix, field)?))?;
            let domain = domain(projection, field)?;
            let _ = writeln!(output, "static inline tb_{domain}_t {getter}(type_bridge_plan_builder_t *p, {handle} h) {{
                if (type_bridge_plan_builder_check_handle(p, h.recording, h.binding) != TYPE_BRIDGE_STATUS_OK)
                    return type_bridge_plan_{domain}_field(p, UINT32_MAX, 0u);
                return type_bridge_plan_{domain}_field(p, h.binding, type_bridge_plan_builder_model_field(p, h.model_ordinal, {index}u));
            }}");
        }
        if has_values(model) {
            let bulk = name(model, "plan_bind_rows")?;
            let values = name(model, "values_t")?;
            let _ = writeln!(output, "/* Rows and their strings are borrowed only for this call; the recording copies their values. No generated allocation or temporary cell frame. */\nstatic inline type_bridge_status_t {bulk}(type_bridge_plan_builder_t *p, {handle} h, const {values} *rows, size_t count) {{
                type_bridge_status_t status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status; status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;");
            if model.create().fields().is_empty() {
                let _ = writeln!(
                    output,
                    "return type_bridge_plan_builder_model_host_rows(p, h.binding, NULL, 0u, rows, count, sizeof(*rows));\n}}"
                );
            } else {
                output.push_str("type_bridge_plan_bulk_host_column_t columns[] = {\n");
                for create in model.create().fields() {
                    let field = &fields[create.token()];
                    let index = fields
                        .keys()
                        .position(|id| id == create.token())
                        .expect("create field is a query token");
                    let member = member(prefix, field)?;
                    let domain = match query::field_domain(projection, field)? {
                        ValueTypeTag::Long => "LONG",
                        ValueTypeTag::Double => "DOUBLE",
                        ValueTypeTag::Boolean => "BOOLEAN",
                        ValueTypeTag::String => "STRING",
                        ValueTypeTag::Date => "DATE",
                        ValueTypeTag::DateTime => "DATETIME",
                        ValueTypeTag::DateTimeTz => "DATETIME_TZ",
                        ValueTypeTag::Decimal => "DECIMAL",
                        ValueTypeTag::Duration => "DURATION",
                    };
                    let presence = if create.multiplicity().required() {
                        "SIZE_MAX".to_owned()
                    } else {
                        format!("offsetof({values}, has_{member})")
                    };
                    let _ = writeln!(
                        output,
                        "{{ type_bridge_plan_builder_model_field(p, h.model_ordinal, {index}u), TYPE_BRIDGE_PROJECTED_VALUE_{domain}, offsetof({values}, {member}), {presence} }},"
                    );
                }
                output.push_str("};\nreturn type_bridge_plan_builder_model_host_rows(p, h.binding, columns, sizeof(columns) / sizeof(columns[0]), rows, count, sizeof(*rows));\n}\n");
            }
        }
        let where_ = name(model, "where")?;
        let _ = writeln!(
            output,
            "static inline type_bridge_status_t {where_}(type_bridge_plan_builder_t *p, {handle} h, tb_cond_t condition) {{ type_bridge_status_t status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status; status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status; return type_bridge_plan_require(p, condition); }}"
        );
        let set_fn = name(model, "set")?;
        let _ = writeln!(
            output,
            "static inline type_bridge_status_t {set_fn}(type_bridge_plan_builder_t *p, {handle} h, const {set} *values) {{ type_bridge_status_t status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status; status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status; if (values == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;"
        );
        for (index, field) in fields
            .values()
            .enumerate()
            .filter(|(_, field)| writable(field))
        {
            let domain = domain(projection, field)?;
            let member = member(prefix, field)?;
            let _ = writeln!(
                output,
                "status = type_bridge_plan_{domain}_set(p, h.binding, type_bridge_plan_builder_model_field(p, h.model_ordinal, {index}u), values->{member}); if (status != TYPE_BRIDGE_STATUS_OK) return status;"
            );
        }
        output.push_str("return status;\n}\n");
        let unset = name(model, "unset_t")?;
        let unset_fn = name(model, "unset")?;
        let _ = writeln!(
            output,
            "static inline type_bridge_status_t {unset_fn}(type_bridge_plan_builder_t *p, {handle} h, const {unset} *fields) {{ type_bridge_status_t status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status; status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status; if (fields == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;"
        );
        for (index, field) in fields
            .values()
            .enumerate()
            .filter(|(_, field)| writable(field))
        {
            let member = member(prefix, field)?;
            let _ = writeln!(
                output,
                "if (fields->{member}) {{ status = type_bridge_plan_builder_delete_field(p, h.binding, type_bridge_plan_builder_model_field(p, h.model_ordinal, {index}u)); if (status != TYPE_BRIDGE_STATUS_OK) return status; }}"
            );
        }
        output.push_str("return status;\n}\n");
        for (suffix, subtypes) in [("match_by", false), ("subtypes_by", true)] {
            let function = name(model, suffix)?;
            let capacity = 2 + fields.len() * 2;
            let _ = writeln!(output, "static inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, const {matched} *values, {handle} *out) {{
                type_bridge_generated_opaque_input_v1_t inputs[{capacity}]; size_t count = 0u; type_bridge_status_t status; {handle} h;
                {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
                {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, values, sizeof(*values));
                if (values != NULL) {{");
            for field in fields.values() {
                let member = member(prefix, field)?;
                let _ = writeln!(
                    output,
                    "{add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, values->{member}._value._builder, 1u);"
                );
                if !matches!(
                    query::field_domain(projection, field)?,
                    ValueTypeTag::Long | ValueTypeTag::Double | ValueTypeTag::Boolean
                ) {
                    let _ = writeln!(
                        output,
                        "if (values->{member}._value._kind == 1u && values->{member}._literal.data != NULL) {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, values->{member}._literal.data, values->{member}._literal.length);"
                    );
                }
            }
            let _ = writeln!(output, "}}\nstatus = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u);
                if (status != TYPE_BRIDGE_STATUS_OK) return status;
                memset(out, 0, sizeof(*out)); if (values == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
                status = {init}(p, {}, &h); if (status != TYPE_BRIDGE_STATUS_OK) return status;", if subtypes { "true" } else { "false" });
            for field in fields.values() {
                let member = member(prefix, field)?;
                let domain = domain(projection, field)?;
                let getter = name(model, &format!("plan_{}", raw_member(prefix, field)?))?;
                let _ = writeln!(
                    output,
                    "if (values->{member}._value._kind != 0u) {{ status = type_bridge_plan_require(p, tb_{domain}_eq({getter}(p, h), values->{member})); if (status != TYPE_BRIDGE_STATUS_OK) return status; }}"
                );
            }
            output.push_str("memcpy(out, &h, sizeof(h)); return status;\n}\n");
        }
        let subtypes = name(model, "subtypes")?;
        let _ = writeln!(
            output,
            "static inline type_bridge_status_t {subtypes}(type_bridge_plan_builder_t *p, {handle} *out) {{ return {init}(p, true, out); }}"
        );
        let function = name(model, "by_iid")?;
        let _ = writeln!(output, "static inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, const char *iid, {handle} *out) {{
            type_bridge_generated_opaque_input_v1_t inputs[2]; size_t count = 0u; type_bridge_status_t status; {handle} h;
            type_bridge_byte_view_t view = {{(const uint8_t *)iid, iid == NULL ? 1u : strlen(iid)}};
            {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
            {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, iid, iid == NULL ? 0u : view.length + 1u);
            status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u); if (status != TYPE_BRIDGE_STATUS_OK) return status;
            memset(out, 0, sizeof(*out)); if (iid == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
            status = {init}(p, false, &h); if (status != TYPE_BRIDGE_STATUS_OK) return status;
            status = type_bridge_plan_builder_by_iid(p, h.binding, view); if (status != TYPE_BRIDGE_STATUS_OK) return status;
            memcpy(out, &h, sizeof(h)); return status;
        }}");
        let match_fn = name(model, "match")?;
        let keys = fields
            .values()
            .enumerate()
            .filter(|(_, field)| field.is_key())
            .collect::<Vec<_>>();
        if let [(index, field)] = keys.as_slice() {
            let tag = query::field_domain(projection, field)?;
            let domain = domain(projection, field)?;
            let host = match tag {
                ValueTypeTag::Long => "int64_t",
                ValueTypeTag::Double => "double",
                ValueTypeTag::Boolean => "bool",
                _ => "const char *",
            };
            let textual = !matches!(
                tag,
                ValueTypeTag::Long | ValueTypeTag::Double | ValueTypeTag::Boolean
            );
            let _ = writeln!(output, "static inline type_bridge_status_t {match_fn}(type_bridge_plan_builder_t *p, {host} key, {handle} *out) {{
                type_bridge_generated_opaque_input_v1_t inputs[2]; size_t count = 0u; type_bridge_status_t status; {handle} h;
                {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);");
            if textual {
                let _ = writeln!(
                    output,
                    "type_bridge_byte_view_t view = {{(const uint8_t *)key, key == NULL ? 1u : strlen(key)}}; {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, key, key == NULL ? 0u : view.length + 1u);"
                );
            }
            let _ = writeln!(output, "status = {preflight}(inputs, count, out, sizeof(*out), NULL, 0u); if (status != TYPE_BRIDGE_STATUS_OK) return status; memset(out, 0, sizeof(*out));
                status = {init}(p, false, &h); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                status = type_bridge_plan_{domain}_key(p, h.binding, type_bridge_plan_builder_model_field(p, h.model_ordinal, {index}u), tb_{domain}({}));
                if (status != TYPE_BRIDGE_STATUS_OK) {{ return status; }}
                memcpy(out, &h, sizeof(h)); return status;
}}", if textual { "view" } else { "key" });
        } else {
            let by = name(model, "match_by")?;
            let _ = writeln!(
                output,
                "static inline type_bridge_status_t {match_fn}(type_bridge_plan_builder_t *p, const {matched} *values, {handle} *out) {{ return {by}(p, values, out); }}"
            );
        }
        if has_insert(model) {
            render_create_handle(output, projection, model, prefix)?;
        }
    }
    roles::render_functions(output, projection, prefix)?;
    terminals::render_functions(output, projection, prefix)?;
    views::render_functions(output, projection, prefix)?;
    Ok(())
}

pub(super) fn render_open(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    _ordered: bool,
) -> Result<(), Diagnostic> {
    let opener = generated_symbol(prefix, "plan_open")?;
    let digest_name = generated_symbol(prefix, "plan_view_layout_digest")?;
    let digest =
        type_bridge_contract::projection::c_view::CViewLayouts::for_projection(projection)?
            .digest();
    let [add, preflight] = generated_alias_helper_symbols(prefix)?;
    let diagnostics = "type_bridge_execution_diagnostics_t";
    let close = "type_bridge_execution_diagnostics_close";
    let package_open = generated_symbol(prefix, "schema_package_open")?;
    let _ = writeln!(
        output,
        "static const uint8_t {digest_name}[32] = {{ {} }};",
        digest
            .iter()
            .map(|byte| format!("0x{byte:02x}u"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(output, "type_bridge_status_t TYPE_BRIDGE_CALL {opener}(const type_bridge_plan_options_t *options, type_bridge_plan_builder_t **out, type_bridge_execution_diagnostics_t **out_diagnostics) {{
        type_bridge_generated_opaque_input_v1_t inputs[1]; size_t count = 0u;
        type_bridge_status_t status; type_bridge_schema_package_t *package = NULL;
        {diagnostics} *package_diagnostics = NULL; type_bridge_plan_builder_t *empty = NULL; type_bridge_execution_diagnostics_t *empty_diagnostics = NULL;
        type_bridge_byte_view_t digest = {{ {digest_name}, sizeof({digest_name}) }};
        {add}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_OPTIONS, options, 1u);
        status = {preflight}(inputs, count, out, sizeof(*out), out_diagnostics, sizeof(*out_diagnostics));
        if (status != TYPE_BRIDGE_STATUS_OK) return status;
        if (out != NULL) memcpy(out, &empty, sizeof(empty));
        if (out_diagnostics != NULL) memcpy(out_diagnostics, &empty_diagnostics, sizeof(empty_diagnostics));
        if (out == NULL || out_diagnostics == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
        status = {package_open}(&package, &package_diagnostics);
        if (package_diagnostics != NULL) (void){close}(&package_diagnostics);
        if (status != TYPE_BRIDGE_STATUS_OK) return status;
        status = type_bridge_plan_builder_new_with_layout(package, options, digest, out, out_diagnostics);
        (void)type_bridge_schema_package_close(&package);
        return status;
    }}");
    Ok(())
}

fn render_create_handle(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let wrapper = name(model, "plan_create_handle")?;
    let function = name(model, "plan_create_handle_checked")?;
    let input = name(model, "insert_t")?;
    let handle = name(model, "handle_t")?;
    let token = model_token_symbol(projection, prefix, model.id())?;
    let opener = generated_symbol(prefix, "plan_open")?;
    let [add_input, preflight] = generated_alias_helper_symbols(prefix)?;
    let capacity = 2 + model.create().fields().len() + model.create().roles().len();
    let _ = writeln!(output, "/* Internal recording helper shared by insert and put. Borrowed strings and player arrays are copied during this call. */\nstatic inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, bool put, const {input} *values, {handle} *out, void *marker_output, size_t marker_output_size) {{
        type_bridge_generated_opaque_input_v1_t inputs[{capacity}]; size_t count = 0u; type_bridge_status_t status; uint32_t model = 0u; {handle} h;
        {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, p, 1u);
        {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, values, sizeof(*values));
        if (values != NULL) {{");
    for create in model.create().fields() {
        let field = &model.query_tokens().fields()[create.token()];
        if matches!(
            query::field_domain(projection, field)?,
            ValueTypeTag::Long | ValueTypeTag::Double | ValueTypeTag::Boolean
        ) {
            continue;
        }
        let member = member(prefix, field)?;
        let present = if create.multiplicity().required() {
            "true".to_owned()
        } else {
            format!("values->has_{member}")
        };
        let _ = writeln!(
            output,
            "if ({present} && values->{member} != NULL) {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, values->{member}, strlen(values->{member}) + 1u);"
        );
    }
    for role in model.query_tokens().roles().values().filter(|role| {
        model.create().roles().contains_key(role.role())
            && role.multiplicity().container() == ProjectedContainer::Sequence
            && !role.accepted_players().is_empty()
    }) {
        let member = roles::escaped_member(prefix, role.target_name())?;
        let _ = writeln!(output, "if (values->{member}_count > SIZE_MAX / sizeof(*values->{member}) || (values->{member}_count != 0u && values->{member} == NULL)) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
            {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_BYTES, values->{member}, values->{member}_count * sizeof(*values->{member}));");
    }
    let _ = writeln!(output, "}}\nstatus = {preflight}(inputs, count, out, sizeof(*out), marker_output, marker_output_size); if (status != TYPE_BRIDGE_STATUS_OK) return status;
        if (values != NULL) {{");
    roles::render_assignments(output, projection, model, prefix, None, true)?;
    let _ = writeln!(output, "}}\nmemset(out, 0, sizeof(*out)); if (marker_output != NULL) memset(marker_output, 0, marker_output_size); if (p == NULL || values == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;
        memset(&h, 0, sizeof(h)); h.recording = p; h.model_ordinal = {token}.ordinal;
        {{ type_bridge_byte_view_t opener = {{(const uint8_t *){opener:?}, {}}}; status = type_bridge_plan_builder_type_token(p, &{token}, opener, &model); }}
        if (status != TYPE_BRIDGE_STATUS_OK) return status;
        status = put ? type_bridge_plan_builder_put(p, model, &h.binding) : type_bridge_plan_builder_insert(p, model, &h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;", opener.len());
    for create in model.create().fields() {
        let field = &model.query_tokens().fields()[create.token()];
        let member = member(prefix, field)?;
        let domain = domain(projection, field)?;
        let index = model
            .query_tokens()
            .fields()
            .keys()
            .position(|id| id == create.token())
            .expect("create field is a query token");
        let present = if create.multiplicity().required() {
            "true".to_owned()
        } else {
            format!("values->has_{member}")
        };
        let _ = writeln!(output, "if ({present}) {{");
        let argument = if matches!(
            query::field_domain(projection, field)?,
            ValueTypeTag::Long | ValueTypeTag::Double | ValueTypeTag::Boolean
        ) {
            format!("values->{member}")
        } else {
            let _ = writeln!(
                output,
                "type_bridge_byte_view_t literal = {{(const uint8_t *)values->{member}, values->{member} == NULL ? 1u : strlen(values->{member})}};"
            );
            "literal".to_owned()
        };
        let _ = writeln!(
            output,
            "status = type_bridge_plan_{domain}_create_field(p, h.binding, type_bridge_plan_builder_model_field(p, h.model_ordinal, {index}u), tb_{domain}({argument})); if (status != TYPE_BRIDGE_STATUS_OK) return status; }}"
        );
    }
    roles::render_assignments(
        output,
        projection,
        model,
        prefix,
        Some("type_bridge_plan_builder_thing_player"),
        true,
    )?;
    output.push_str("memcpy(out, &h, sizeof(h)); return status;\n}\n");
    let _ = writeln!(
        output,
        "static inline type_bridge_status_t {wrapper}(type_bridge_plan_builder_t *p, bool put, const {input} *values, {handle} *out) {{ return {function}(p, put, values, out, NULL, 0u); }}"
    );
    Ok(())
}
