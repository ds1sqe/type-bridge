use std::collections::BTreeSet;
use std::fmt::Write as _;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{
    FunctionProjection, FunctionReturnProjection, ModelProjection, ProjectedTypeRef,
    RuntimeProjection,
};
use type_bridge_contract::value::ValueTypeTag;

use super::{QUERY_MODES, field_base, model_name, package_name, query_models};
use crate::c::render::{
    field_token_symbol, function_token_symbol, generated_symbol, projected_member_suffix,
};

const FUNCTION_ARGUMENT_MAX: usize = 256;
const FUNCTION_ARGUMENT_BYTES: usize = 72;

const FUNCTION_COMPARISONS: [(&str, &str); 6] = [
    ("equal", "TYPE_BRIDGE_QUERY_COMPARE_EQUAL"),
    ("not_equal", "TYPE_BRIDGE_QUERY_COMPARE_NOT_EQUAL"),
    ("less_than", "TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN"),
    (
        "less_than_or_equal",
        "TYPE_BRIDGE_QUERY_COMPARE_LESS_THAN_OR_EQUAL",
    ),
    ("greater_than", "TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN"),
    (
        "greater_than_or_equal",
        "TYPE_BRIDGE_QUERY_COMPARE_GREATER_THAN_OR_EQUAL",
    ),
];

#[derive(Clone, Copy)]
struct SupportedFunction<'a> {
    projection: &'a FunctionProjection,
    return_domain: ValueTypeTag,
}

fn supported_function(function: &FunctionProjection) -> Option<SupportedFunction<'_>> {
    if function.parameters().len() > FUNCTION_ARGUMENT_MAX
        || function
            .parameters()
            .iter()
            .any(|parameter| matches!(parameter.type_ref(), ProjectedTypeRef::Struct(_)))
    {
        return None;
    }
    let FunctionReturnProjection::Scalar(result) = function.returns() else {
        return None;
    };
    if result.optional() {
        return None;
    }
    let ProjectedTypeRef::Scalar(return_domain) = result.type_ref() else {
        return None;
    };
    Some(SupportedFunction {
        projection: function,
        return_domain: *return_domain,
    })
}

fn supported_functions(
    projection: &RuntimeProjection,
) -> impl Iterator<Item = SupportedFunction<'_>> {
    projection
        .functions()
        .values()
        .filter_map(supported_function)
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

fn used_domains(projection: &RuntimeProjection) -> BTreeSet<ValueTypeTag> {
    let mut domains = BTreeSet::new();
    for function in public_functions(projection) {
        domains.insert(function.return_domain);
        for parameter in function.projection.parameters() {
            if let ProjectedTypeRef::Scalar(domain) = parameter.type_ref() {
                domains.insert(*domain);
            }
        }
    }
    domains
}

fn domain_name(prefix: &str, domain: ValueTypeTag, suffix: &str) -> Result<String, Diagnostic> {
    package_name(
        prefix,
        &format!("query_function_{}_{}", scalar_suffix(domain), suffix),
    )
}

fn domain_input_name(prefix: &str, domain: ValueTypeTag) -> Result<String, Diagnostic> {
    domain_name(prefix, domain, "input")
}

fn domain_argument_name(prefix: &str, domain: ValueTypeTag) -> Result<String, Diagnostic> {
    domain_name(prefix, domain, "argument_v1_t")
}

fn domain_call_name(prefix: &str, domain: ValueTypeTag) -> Result<String, Diagnostic> {
    domain_name(prefix, domain, "call_v1_t")
}

fn domain_field_name(prefix: &str, domain: ValueTypeTag) -> Result<String, Diagnostic> {
    domain_name(prefix, domain, "field_v1_t")
}

fn model_argument_name(model: &ModelProjection) -> Result<String, Diagnostic> {
    model_name(model, "query_function_argument_v1_t")
}

fn function_call_name(function: &FunctionProjection) -> Result<String, Diagnostic> {
    generated_symbol(function.target_name().as_str(), "query_call")
}

fn function_arguments_name(function: &FunctionProjection) -> Result<String, Diagnostic> {
    generated_symbol(function.target_name().as_str(), "query_arguments_v1_t")
}

fn function_members_name(function: &FunctionProjection) -> Result<String, Diagnostic> {
    generated_symbol(function.target_name().as_str(), "query_argument_members_v1")
}

fn function_name(function: &FunctionProjection, suffix: &str) -> Result<String, Diagnostic> {
    generated_symbol(function.target_name().as_str(), &format!("query_{suffix}"))
}

fn type_is_same_or_subtype(
    projection: &RuntimeProjection,
    candidate: &TypeId,
    expected: &TypeId,
) -> bool {
    if candidate.kind() != expected.kind() {
        return false;
    }
    let mut current = Some(candidate);
    while let Some(id) = current {
        if id == expected {
            return true;
        }
        current = projection
            .models()
            .get(id)
            .and_then(|model| model.declaration().parent());
    }
    false
}

fn model_parameters(projection: &RuntimeProjection) -> BTreeSet<&TypeId> {
    public_functions(projection)
        .flat_map(|function| function.projection.parameters())
        .filter_map(|parameter| match parameter.type_ref() {
            ProjectedTypeRef::Model(model)
                if matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation)
                    && projection
                        .models()
                        .get(model.id())
                        .is_some_and(|projected| {
                            projected.query_tokens().target_name().is_some()
                        }) =>
            {
                Some(model.id())
            }
            _ => None,
        })
        .collect()
}

fn function_is_fully_supported(
    projection: &RuntimeProjection,
    function: SupportedFunction<'_>,
) -> bool {
    function
        .projection
        .parameters()
        .iter()
        .all(|parameter| match parameter.type_ref() {
            ProjectedTypeRef::Scalar(_) => true,
            ProjectedTypeRef::Model(model) => {
                matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation)
                    && projection
                        .models()
                        .get(model.id())
                        .is_some_and(|projected| projected.query_tokens().target_name().is_some())
            }
            ProjectedTypeRef::Struct(_) => false,
        })
}

fn public_functions(projection: &RuntimeProjection) -> impl Iterator<Item = SupportedFunction<'_>> {
    supported_functions(projection)
        .filter(|function| function_is_fully_supported(projection, *function))
}

fn attribute_domain(model: &ModelProjection) -> Option<ValueTypeTag> {
    (model.id().kind() == TypeKind::Attribute)
        .then(|| model.declaration().value_type())
        .flatten()
}

fn query_fields(
    projection: &RuntimeProjection,
    domain: ValueTypeTag,
) -> Vec<(
    &ModelProjection,
    &type_bridge_contract::projection::FieldTokenProjection,
)> {
    let mut fields = Vec::new();
    for owner in query_models(projection) {
        for field in owner.query_tokens().fields().values() {
            let Ok(attribute_id) = crate::c::render::attribute_type_id(field.id()) else {
                continue;
            };
            if projection
                .models()
                .get(&attribute_id)
                .and_then(attribute_domain)
                == Some(domain)
            {
                fields.push((owner, field));
            }
        }
    }
    fields
}

fn parameter_member_name(
    prefix: &str,
    target: &type_bridge_contract::projection::TargetIdentifier,
) -> Result<String, Diagnostic> {
    Ok(format!(
        "argument_{}",
        projected_member_suffix(prefix, target)?
    ))
}

pub(super) fn nominal_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for domain in used_domains(projection) {
        names.insert(domain_input_name(prefix, domain)?);
    }
    for function in public_functions(projection) {
        names.insert(function_call_name(function.projection)?);
    }
    Ok(names)
}

pub(super) fn auxiliary_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for domain in used_domains(projection) {
        names.insert(domain_argument_name(prefix, domain)?);
        names.insert(domain_call_name(prefix, domain)?);
        names.insert(domain_field_name(prefix, domain)?);
    }
    for model in model_parameters(projection) {
        let projected = projection
            .models()
            .get(model)
            .expect("function model parameter is projected");
        names.insert(model_argument_name(projected)?);
    }
    for function in public_functions(projection) {
        names.insert(function_arguments_name(function.projection)?);
        if !function.projection.parameters().is_empty() {
            names.insert(function_members_name(function.projection)?);
        }
    }
    Ok(names)
}

pub(super) fn function_names(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<BTreeSet<String>, Diagnostic> {
    let mut names = BTreeSet::new();
    for domain in used_domains(projection) {
        names.insert(domain_name(prefix, domain, "argument_from_input")?);
        names.insert(domain_name(prefix, domain, "argument_from_call")?);
        names.insert(domain_name(prefix, domain, "input_close")?);
        for (operator, _) in FUNCTION_COMPARISONS {
            names.insert(domain_name(
                prefix,
                domain,
                &format!("field_{operator}_call"),
            )?);
        }
        for attribute in projection
            .models()
            .values()
            .filter(|model| attribute_domain(model) == Some(domain))
        {
            names.insert(generated_symbol(
                attribute.target_name().as_str(),
                "query_function_input_open",
            )?);
        }
        for (owner, field) in query_fields(projection, domain) {
            names.insert(generated_symbol(
                &field_base(owner, prefix, field)?,
                "function_field",
            )?);
        }
    }
    for expected in model_parameters(projection) {
        let expected_model = &projection.models()[expected];
        let base = model_argument_name(expected_model)?;
        for candidate in query_models(projection)
            .filter(|candidate| type_is_same_or_subtype(projection, candidate.id(), expected))
        {
            let member = projected_member_suffix(prefix, candidate.target_name())?;
            for (mode, _) in QUERY_MODES {
                names.insert(generated_symbol(&base, &format!("from_{member}_{mode}"))?);
            }
        }
    }
    for function in public_functions(projection) {
        for suffix in ["open", "close", "call_open", "call_ref", "call_close"] {
            names.insert(function_name(function.projection, suffix)?);
        }
        for (operator, _) in FUNCTION_COMPARISONS {
            for operand in ["field", "value", "call"] {
                names.insert(function_name(
                    function.projection,
                    &format!("call_{operator}_{operand}"),
                )?);
            }
        }
    }
    Ok(names)
}

fn render_argument_layout_assertions(output: &mut String, witness: &str) {
    let _ = write!(
        output,
        "#if defined(__cplusplus)\n\
         static_assert(sizeof({witness}) == {FUNCTION_ARGUMENT_BYTES}u,\n\
             \"generated function argument witness size drifted\");\n\
         static_assert(alignof({witness}) == alignof(type_bridge_query_function_argument_v1_t),\n\
             \"generated function argument witness alignment drifted\");\n\
         #else\n\
         _Static_assert(sizeof({witness}) == {FUNCTION_ARGUMENT_BYTES}u,\n\
             \"generated function argument witness size drifted\");\n\
         _Static_assert(_Alignof({witness}) == _Alignof(type_bridge_query_function_argument_v1_t),\n\
             \"generated function argument witness alignment drifted\");\n\
         #endif\n\
         #if defined(__cplusplus)\n\
         static_assert(offsetof({witness}, binding) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, binding) &&\n\
             offsetof({witness}, value) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, value) &&\n\
             offsetof({witness}, call) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, call) &&\n\
             offsetof({witness}, reserved) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, reserved),\n\
             \"generated function argument witness offsets drifted\");\n\
         #else\n\
         _Static_assert(offsetof({witness}, binding) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, binding) &&\n\
             offsetof({witness}, value) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, value) &&\n\
             offsetof({witness}, call) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, call) &&\n\
             offsetof({witness}, reserved) ==\n\
                 offsetof(type_bridge_query_function_argument_v1_t, reserved),\n\
             \"generated function argument witness offsets drifted\");\n\
         #endif\n\n"
    );
}

fn render_argument_struct(output: &mut String, witness: &str) {
    let _ = write!(
        output,
        "typedef struct {witness} {{\n\
           uint32_t struct_size;\n\
           uint32_t version;\n\
           type_bridge_query_function_argument_kind_t kind;\n\
           uint32_t reserved0;\n\
           const type_bridge_query_binding_t *binding;\n\
           const type_bridge_query_function_value_t *value;\n\
           const type_bridge_query_function_call_t *call;\n\
           uint64_t reserved[4];\n\
         }} {witness};\n"
    );
    render_argument_layout_assertions(output, witness);
}

pub(super) fn render_declarations(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    if public_functions(projection).next().is_none() {
        return Ok(());
    }
    let session = package_name(prefix, "query_session")?;
    let predicate = package_name(prefix, "query_predicate")?;
    output.push_str(
        "\n/* Generated schema-function facade. Argument witnesses are exact\n\
         * common-layout values; opaque inputs/calls own their native handles. */\n",
    );
    for domain in used_domains(projection) {
        let argument = domain_argument_name(prefix, domain)?;
        let call = domain_call_name(prefix, domain)?;
        let field = domain_field_name(prefix, domain)?;
        render_argument_struct(output, &argument);
        let _ = write!(
            output,
            "typedef struct {call} {{\n\
               const type_bridge_query_function_call_t *handle;\n\
               uint64_t reserved[4];\n\
             }} {call};\n\
             typedef struct {field} {{\n\
               const type_bridge_query_field_t *handle;\n\
               const type_bridge_projected_token_v1_t *expected_field;\n\
               uint64_t reserved[4];\n\
             }} {field};\n"
        );
    }
    for expected in model_parameters(projection) {
        render_argument_struct(
            output,
            &model_argument_name(&projection.models()[expected])?,
        );
    }
    for function in public_functions(projection) {
        let arguments = function_arguments_name(function.projection)?;
        let call = function_call_name(function.projection)?;
        let _ = write!(
            output,
            "typedef struct {arguments} {{\n\
               type_bridge_query_function_arguments_header_v1_t header;\n"
        );
        for parameter in function.projection.parameters() {
            let member = parameter_member_name(prefix, parameter.target_name())?;
            let witness = match parameter.type_ref() {
                ProjectedTypeRef::Scalar(domain) => domain_argument_name(prefix, *domain)?,
                ProjectedTypeRef::Model(model) => {
                    model_argument_name(&projection.models()[model.id()])?
                }
                ProjectedTypeRef::Struct(_) => unreachable!("unsupported functions were filtered"),
            };
            let _ = writeln!(output, "  {witness} {member};");
        }
        let _ = write!(
            output,
            "}} {arguments};\n\
             #if defined(__cplusplus)\n\
             static_assert(offsetof({arguments}, header) == 0u,\n\
                 \"generated function arguments header must be first\");\n\
             static_assert(sizeof({arguments}) <= TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX,\n\
                 \"generated function arguments exceed the hosted-object ceiling\");\n\
             #else\n\
             _Static_assert(offsetof({arguments}, header) == 0u,\n\
                 \"generated function arguments header must be first\");\n\
             _Static_assert(sizeof({arguments}) <= TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX,\n\
                 \"generated function arguments exceed the hosted-object ceiling\");\n\
             #endif\n"
        );
        if !function.projection.parameters().is_empty() {
            let members = function_members_name(function.projection)?;
            let _ = writeln!(
                output,
                "static const type_bridge_query_function_argument_member_v1_t {members}[] = {{"
            );
            for parameter in function.projection.parameters() {
                let member = parameter_member_name(prefix, parameter.target_name())?;
                let _ = writeln!(
                    output,
                    "  {{ sizeof(type_bridge_query_function_argument_member_v1_t), TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, offsetof({arguments}, {member}), {{ 0u, 0u, 0u, 0u }} }},"
                );
            }
            let _ = write!(
                output,
                "}};\n\
                 #if defined(__cplusplus)\n\
                 static_assert(sizeof({members}) <= TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX,\n\
                     \"generated function argument member table exceeds the hosted-object ceiling\");\n\
                 #else\n\
                 _Static_assert(sizeof({members}) <= TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENTS_BYTES_MAX,\n\
                     \"generated function argument member table exceeds the hosted-object ceiling\");\n\
                 #endif\n"
            );
        }
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {open}(\n\
               const {session} *session, {target} **out_function,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {close}({target} **function);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {call_open}(\n\
               const {target} *function, const {arguments} *arguments,\n\
               {call} **out_call,\n\
               type_bridge_execution_diagnostics_t **out_diagnostics);\n\
             {domain_call} TYPE_BRIDGE_CALL {call_ref}(const {call} *call);\n\
             type_bridge_status_t TYPE_BRIDGE_CALL {call_close}({call} **call);\n",
            open = function_name(function.projection, "open")?,
            target = function.projection.target_name().as_str(),
            close = function_name(function.projection, "close")?,
            call_open = function_name(function.projection, "call_open")?,
            domain_call = domain_call_name(prefix, function.return_domain)?,
            call_ref = function_name(function.projection, "call_ref")?,
            call_close = function_name(function.projection, "call_close")?,
        );
        for (operator, _) in FUNCTION_COMPARISONS {
            let base = function.projection;
            let domain_field = domain_field_name(prefix, function.return_domain)?;
            let domain_input = domain_input_name(prefix, function.return_domain)?;
            let domain_call = domain_call_name(prefix, function.return_domain)?;
            for (operand, operand_type) in [
                ("field", domain_field),
                ("value", format!("const {domain_input} *")),
                ("call", domain_call),
            ] {
                let parameter = if operand == "value" {
                    format!("{operand_type} value")
                } else {
                    format!("{operand_type} {operand}")
                };
                let _ = write!(
                    output,
                    "type_bridge_status_t TYPE_BRIDGE_CALL {name}(\n\
                       const {call} *call, {parameter}, {predicate} **out_predicate,\n\
                       type_bridge_execution_diagnostics_t **out_diagnostics);\n",
                    name = function_name(base, &format!("call_{operator}_{operand}"))?,
                );
            }
        }
    }
    Ok(())
}

fn argument_initializer(kind: &str, binding: &str, value: &str, call: &str) -> String {
    format!(
        "{{ {FUNCTION_ARGUMENT_BYTES}u, TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, {kind}, 0u, {binding}, {value}, {call}, {{ 0u, 0u, 0u, 0u }} }}"
    )
}

pub(super) fn render_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    if public_functions(projection).next().is_none() {
        return Ok(());
    }
    let session = package_name(prefix, "query_session")?;
    let predicate = package_name(prefix, "query_predicate")?;
    for domain in used_domains(projection) {
        let input = domain_input_name(prefix, domain)?;
        let argument = domain_argument_name(prefix, domain)?;
        let call = domain_call_name(prefix, domain)?;
        let field = domain_field_name(prefix, domain)?;
        for attribute in projection
            .models()
            .values()
            .filter(|model| attribute_domain(model) == Some(domain))
        {
            let function = generated_symbol(
                attribute.target_name().as_str(),
                "query_function_input_open",
            )?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {function}(\n\
                   const {session} *session, const {attribute} *value,\n\
                   {input} **out_input,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_function_value_open(\n\
                       (const type_bridge_query_session_t *)session,\n\
                       (const type_bridge_projected_value_t *)value,\n\
                       (type_bridge_query_function_value_t **)out_input,\n\
                       out_diagnostics);\n\
                 }}\n\n",
                attribute = attribute.target_name().as_str(),
            );
        }
        let _ = write!(
            output,
            "type_bridge_status_t TYPE_BRIDGE_CALL {input_close}({input} **input) {{\n\
               return type_bridge_query_function_value_close(\n\
                   (type_bridge_query_function_value_t **)input);\n\
             }}\n\n\
             {argument} TYPE_BRIDGE_CALL {from_input}(const {input} *input) {{\n\
               {argument} value = {value_initializer};\n\
               return value;\n\
             }}\n\n\
             {argument} TYPE_BRIDGE_CALL {from_call}({call} call) {{\n\
               {argument} value = {call_initializer};\n\
               return value;\n\
             }}\n\n",
            input_close = domain_name(prefix, domain, "input_close")?,
            from_input = domain_name(prefix, domain, "argument_from_input")?,
            value_initializer = argument_initializer(
                "TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_VALUE",
                "NULL",
                "(const type_bridge_query_function_value_t *)input",
                "NULL",
            ),
            from_call = domain_name(prefix, domain, "argument_from_call")?,
            call_initializer = argument_initializer(
                "TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_CALL",
                "NULL",
                "NULL",
                "call.handle",
            ),
        );
        for (owner, projected_field) in query_fields(projection, domain) {
            let base = field_base(owner, prefix, projected_field)?;
            let constructor = generated_symbol(&base, "function_field")?;
            let token = field_token_symbol(projection, prefix, owner.id(), projected_field.id())?;
            let _ = write!(
                output,
                "{field} TYPE_BRIDGE_CALL {constructor}(const {base} *field) {{\n\
                   {field} value = {{\n\
                     (const type_bridge_query_field_t *)field, &{token},\n\
                     {{ 0u, 0u, 0u, 0u }}\n\
                   }};\n\
                   return value;\n\
                 }}\n\n"
            );
        }
        for (operator, constant) in FUNCTION_COMPARISONS {
            let name = domain_name(prefix, domain, &format!("field_{operator}_call"))?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {name}(\n\
                   {field} field, {call} call, {predicate} **out_predicate,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return type_bridge_query_field_compare_function(\n\
                       field.handle, field.expected_field, {constant}, call.handle,\n\
                       (type_bridge_query_predicate_t **)out_predicate,\n\
                       out_diagnostics);\n\
                 }}\n\n"
            );
        }
    }
    for expected in model_parameters(projection) {
        let expected_model = &projection.models()[expected];
        let argument = model_argument_name(expected_model)?;
        for candidate in query_models(projection)
            .filter(|candidate| type_is_same_or_subtype(projection, candidate.id(), expected))
        {
            let member = projected_member_suffix(prefix, candidate.target_name())?;
            for (mode, _) in QUERY_MODES {
                let binding = model_name(candidate, &format!("query_{mode}_binding"))?;
                let constructor = generated_symbol(&argument, &format!("from_{member}_{mode}"))?;
                let initializer = argument_initializer(
                    "TYPE_BRIDGE_QUERY_FUNCTION_ARGUMENT_BINDING",
                    "(const type_bridge_query_binding_t *)binding",
                    "NULL",
                    "NULL",
                );
                let _ = write!(
                    output,
                    "{argument} TYPE_BRIDGE_CALL {constructor}(const {binding} *binding) {{\n\
                       {argument} value = {initializer};\n\
                       return value;\n\
                     }}\n\n"
                );
            }
        }
    }
    for function in public_functions(projection) {
        render_function_definitions(output, projection, prefix, function, &session, &predicate)?;
    }
    Ok(())
}

fn render_function_definitions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    function: SupportedFunction<'_>,
    session: &str,
    predicate: &str,
) -> Result<(), Diagnostic> {
    let target = function.projection.target_name().as_str();
    let call = function_call_name(function.projection)?;
    let arguments = function_arguments_name(function.projection)?;
    let token = function_token_symbol(projection, prefix, function.projection.id())?;
    let members = function_members_name(function.projection)?;
    let (members_pointer, member_count) = if function.projection.parameters().is_empty() {
        ("NULL".to_owned(), "0u".to_owned())
    } else {
        (
            members.clone(),
            format!("sizeof({members}) / sizeof({members}[0])"),
        )
    };
    let _ = write!(
        output,
        "type_bridge_status_t TYPE_BRIDGE_CALL {open}(\n\
           const {session} *session, {target} **out_function,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           return type_bridge_query_function_open(\n\
               (const type_bridge_query_session_t *)session, &{token},\n\
               (type_bridge_query_function_t **)out_function, out_diagnostics);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {close}({target} **function) {{\n\
           return type_bridge_query_function_close(\n\
               (type_bridge_query_function_t **)function);\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {call_open}(\n\
           const {target} *function, const {arguments} *arguments,\n\
           {call} **out_call,\n\
           type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
           type_bridge_query_function_arguments_graph_v1_t graph = {{\n\
             sizeof(type_bridge_query_function_arguments_graph_v1_t),\n\
             TYPE_BRIDGE_QUERY_DESCRIPTOR_VERSION, arguments, sizeof(*arguments),\n\
             {members_pointer}, {member_count}, {{ 0u, 0u, 0u }}\n\
           }};\n\
           return type_bridge_query_function_call_open_v1(\n\
               (const type_bridge_query_function_t *)function, &{token}, &graph,\n\
               (type_bridge_query_function_call_t **)out_call, out_diagnostics);\n\
         }}\n\n\
         {domain_call} TYPE_BRIDGE_CALL {call_ref}(const {call} *call) {{\n\
           {domain_call} value = {{\n\
             (const type_bridge_query_function_call_t *)call,\n\
             {{ 0u, 0u, 0u, 0u }}\n\
           }};\n\
           return value;\n\
         }}\n\n\
         type_bridge_status_t TYPE_BRIDGE_CALL {call_close}({call} **call) {{\n\
           return type_bridge_query_function_call_close(\n\
               (type_bridge_query_function_call_t **)call);\n\
         }}\n\n",
        open = function_name(function.projection, "open")?,
        close = function_name(function.projection, "close")?,
        call_open = function_name(function.projection, "call_open")?,
        domain_call = domain_call_name(prefix, function.return_domain)?,
        call_ref = function_name(function.projection, "call_ref")?,
        call_close = function_name(function.projection, "call_close")?,
    );
    let domain_field = domain_field_name(prefix, function.return_domain)?;
    let domain_input = domain_input_name(prefix, function.return_domain)?;
    let domain_call = domain_call_name(prefix, function.return_domain)?;
    for (operator, constant) in FUNCTION_COMPARISONS {
        for (operand, parameter, body) in [
            (
                "field",
                format!("{domain_field} field"),
                format!(
                    "type_bridge_query_function_call_compare_field(\n        (const type_bridge_query_function_call_t *)call, {constant},\n        field.handle, field.expected_field,\n        (type_bridge_query_predicate_t **)out_predicate, out_diagnostics)"
                ),
            ),
            (
                "value",
                format!("const {domain_input} *value"),
                format!(
                    "type_bridge_query_function_call_compare_value(\n        (const type_bridge_query_function_call_t *)call, {constant},\n        (const type_bridge_query_function_value_t *)value,\n        (type_bridge_query_predicate_t **)out_predicate, out_diagnostics)"
                ),
            ),
            (
                "call",
                format!("{domain_call} other"),
                format!(
                    "type_bridge_query_function_call_compare_call(\n        (const type_bridge_query_function_call_t *)call, {constant}, other.handle,\n        (type_bridge_query_predicate_t **)out_predicate, out_diagnostics)"
                ),
            ),
        ] {
            let name = function_name(function.projection, &format!("call_{operator}_{operand}"))?;
            let _ = write!(
                output,
                "type_bridge_status_t TYPE_BRIDGE_CALL {name}(\n\
                   const {call} *call, {parameter}, {predicate} **out_predicate,\n\
                   type_bridge_execution_diagnostics_t **out_diagnostics) {{\n\
                   return {body};\n\
                 }}\n\n"
            );
        }
    }
    Ok(())
}
