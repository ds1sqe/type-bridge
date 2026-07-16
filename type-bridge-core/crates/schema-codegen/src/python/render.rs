use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{
    FunctionReturnElementProjection, FunctionReturnProjection, ModelProjection,
    ProjectedContainer, ProjectedModelForm, ProjectedModelUse, ProjectedMultiplicity,
    ProjectedTypeRef, RuntimeProjection,
};
use type_bridge_contract::value::ValueTypeTag;

use crate::{GeneratedPackage, invalid};

const PUBLIC_RUNTIME_NAMES: &[&str] = &["FieldToken", "FunctionRef", "RoleToken"];
const PUBLIC_SCHEMA_NAMES: &[&str] = &[
    "PLAYING_FACTS",
    "PROJECTION_FINGERPRINT_JSON",
    "RUNTIME_PROJECTION_JSON",
    "SEMANTIC_SCHEMA_FINGERPRINT_JSON",
];
const MODEL_RESERVED_NAMES: &[&str] = &[
    "value",
    "iid",
    "__model_form__",
    "__projection__",
    "__runtime_projection__",
    "__type_id__",
    "_attribute_value",
    "_iid",
    "_values",
    "attach_runtime_iid",
    "initialize_runtime_attribute",
    "initialize_runtime_reference",
    "initialize_runtime_values",
    "manager",
    "runtime_attribute_value",
    "runtime_values",
];

macro_rules! canonical_text {
    ($value:expr) => {{
        String::from_utf8(to_canonical_json($value)?)
            .expect("canonical JSON output is UTF-8")
    }};
}

pub(super) fn render(
    projection: &RuntimeProjection,
    runtime_source: &[u8],
    runtime_stub: &[u8],
    py_typed: &[u8],
) -> Result<GeneratedPackage, Diagnostic> {
    validate_projection(projection)?;
    GeneratedPackage::try_new([
        ("__init__.py".to_owned(), finish(render_init(projection, false))),
        ("__init__.pyi".to_owned(), finish(render_init(projection, true))),
        ("_models.py".to_owned(), finish(render_models(projection, false)?)),
        ("_models.pyi".to_owned(), finish(render_models(projection, true)?)),
        ("_runtime.py".to_owned(), runtime_source.to_vec()),
        ("_runtime.pyi".to_owned(), runtime_stub.to_vec()),
        ("_schema.py".to_owned(), finish(render_schema(projection)?)),
        ("py.typed".to_owned(), py_typed.to_vec()),
    ])
}

fn finish(mut source: String) -> Vec<u8> {
    while source.ends_with('\n') {
        source.pop();
    }
    source.push('\n');
    source.into_bytes()
}

fn render_init(projection: &RuntimeProjection, stub: bool) -> String {
    let mut output = String::from(
        "from ._runtime import FieldToken as FieldToken\n\
         from ._runtime import FunctionRef as FunctionRef\n\
         from ._runtime import RoleToken as RoleToken\n",
    );
    if stub {
        output.push_str(
            "from collections.abc import Mapping\nfrom typing import Final\n\n\
             SEMANTIC_SCHEMA_FINGERPRINT_JSON: Final[str]\n\
             PROJECTION_FINGERPRINT_JSON: Final[str]\n\
             RUNTIME_PROJECTION_JSON: Final[str]\n\
             PLAYING_FACTS: Final[Mapping[str, object]]\n",
        );
    } else {
        output.push_str(
            "from ._schema import PLAYING_FACTS as PLAYING_FACTS\n\
             from ._schema import PROJECTION_FINGERPRINT_JSON as PROJECTION_FINGERPRINT_JSON\n\
             from ._schema import RUNTIME_PROJECTION_JSON as RUNTIME_PROJECTION_JSON\n\
             from ._schema import SEMANTIC_SCHEMA_FINGERPRINT_JSON as SEMANTIC_SCHEMA_FINGERPRINT_JSON\n",
        );
    }
    let mut exports = PUBLIC_RUNTIME_NAMES
        .iter()
        .chain(PUBLIC_SCHEMA_NAMES)
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    for id in projection.emission().model_shells() {
        let model = &projection.models()[id];
        import(&mut output, model.target_name().as_str());
        exports.push(model.target_name().as_str().to_owned());
        if let Some(reference) = model.reference_read().target_name() {
            import(&mut output, reference.as_str());
            exports.push(reference.as_str().to_owned());
        }
    }
    for id in projection.emission().structs() {
        let name = projection.structs()[id].target_name().as_str();
        import(&mut output, name);
        exports.push(name.to_owned());
    }
    for id in projection.emission().functions() {
        let name = projection.functions()[id].target_name().as_str();
        import(&mut output, name);
        exports.push(name.to_owned());
    }
    exports.sort();
    output.push_str("\n__all__ = (\n");
    for name in exports {
        let _ = writeln!(output, "    \"{name}\",");
    }
    output.push_str(")\n");
    output
}

fn import(output: &mut String, name: &str) {
    let _ = writeln!(output, "from ._models import {name} as {name}");
}

fn render_schema(projection: &RuntimeProjection) -> Result<String, Diagnostic> {
    let semantic = canonical_text!(projection.semantic_fingerprint());
    let fingerprint = canonical_text!(projection.projection_fingerprint());
    let complete = canonical_text!(projection);
    let mut output = String::from(
        "from __future__ import annotations\n\n\
         from types import MappingProxyType as _MappingProxyType\nfrom typing import Final\n\
         from ._runtime import load_mapping as _load_mapping\n\n",
    );
    let _ = writeln!(
        output,
        "SEMANTIC_SCHEMA_FINGERPRINT_JSON: Final[str] = {}",
        python_string(&semantic)?
    );
    let _ = writeln!(
        output,
        "PROJECTION_FINGERPRINT_JSON: Final[str] = {}",
        python_string(&fingerprint)?
    );
    let _ = writeln!(
        output,
        "RUNTIME_PROJECTION_JSON: Final[str] = {}",
        python_string(&complete)?
    );
    output.push_str("\nPLAYING_FACTS = _MappingProxyType({\n");
    for (id, playing) in projection.playing_facts() {
        let id = canonical_text!(id);
        let playing = canonical_text!(playing);
        let _ = writeln!(
            output,
            "    {}: _load_mapping({}),",
            python_string(&id)?,
            python_string(&playing)?
        );
    }
    output.push_str("})\n");
    Ok(output)
}

fn render_models(
    projection: &RuntimeProjection,
    stub: bool,
) -> Result<String, Diagnostic> {
    let mut body = String::new();
    for id in projection.emission().model_shells() {
        render_model(&mut body, projection, &projection.models()[id], stub)?;
    }
    for id in projection.emission().structs() {
        render_struct(&mut body, &projection.structs()[id], stub)?;
    }
    if !stub {
        body.push_str("# Projection-owned dependency-first SCC link phase.\n");
        for component in projection.emission().model_link_components() {
            body.push_str("# link-component\n");
            for id in component {
                let model = &projection.models()[id];
                let metadata = canonical_text!(model);
                let reference = model
                    .reference_read()
                    .target_name()
                    .map_or("None", |name| name.as_str());
                let _ = writeln!(
                    body,
                    "_install_model({}, {reference}, _load_mapping({}))",
                    model.target_name().as_str(),
                    python_string(&metadata)?
                );
            }
        }
        body.push_str("_install_runtime_projection(\n    _RUNTIME_PROJECTION_JSON,\n    _SEMANTIC_SCHEMA_FINGERPRINT_JSON,\n    _PROJECTION_FINGERPRINT_JSON,\n    [\n");
        for id in projection.emission().model_shells() {
            let model = &projection.models()[id];
            let reference = model
                .reference_read()
                .target_name()
                .map_or("None", |name| name.as_str());
            let _ = writeln!(
                body,
                "        ({}, {reference}),",
                model.target_name().as_str()
            );
        }
        body.push_str("    ],\n)\n");
        body.push('\n');
    }
    for id in projection.emission().functions() {
        render_function(&mut body, projection, &projection.functions()[id], stub)?;
    }
    let mut output = render_model_header(&body, stub);
    output.push_str(&body);
    Ok(output)
}

fn render_model_header(body: &str, stub: bool) -> String {
    let mut output = if stub {
        String::new()
    } else {
        String::from("from __future__ import annotations\n\n")
    };
    let mut collections = Vec::new();
    if body.contains("Iterator[") { collections.push("Iterator"); }
    if body.contains("Sequence[") { collections.push("Sequence"); }
    if !collections.is_empty() {
        let _ = writeln!(output, "from collections.abc import {}", collections.join(", "));
    }
    let mut temporal = Vec::new();
    if body.contains(": date") || body.contains("[date") { temporal.push("date"); }
    if body.contains("datetime") { temporal.push("datetime"); }
    if body.contains("timedelta") { temporal.push("timedelta"); }
    if !temporal.is_empty() {
        let _ = writeln!(output, "from datetime import {}", temporal.join(", "));
    }
    if body.contains("Decimal") { output.push_str("from decimal import Decimal\n"); }
    let mut typing = Vec::new();
    if body.contains("Final[") { typing.push("Final"); }
    if body.contains("Never") { typing.push("Never"); }
    if !typing.is_empty() {
        let _ = writeln!(output, "from typing import {}", typing.join(", "));
    }
    let mut runtime = Vec::new();
    if body.contains("FunctionRef") { runtime.push("FunctionRef"); }
    if body.contains("(_Attribute)") { runtime.push("AttributeBase as _Attribute"); }
    if body.contains("(_Entity)") { runtime.push("EntityBase as _Entity"); }
    if body.contains("_FieldDescriptor[") { runtime.push("FieldDescriptor as _FieldDescriptor"); }
    if body.contains("(_Reference)") { runtime.push("ReferenceBase as _Reference"); }
    if body.contains("(_Relation)") { runtime.push("RelationBase as _Relation"); }
    if body.contains("_RoleDescriptor[") { runtime.push("RoleDescriptor as _RoleDescriptor"); }
    if body.contains("(_StructValue)") { runtime.push("StructValueBase as _StructValue"); }
    if body.contains("_freeze_struct(") { runtime.push("freeze_struct as _freeze_struct"); }
    if body.contains("_initialize(") { runtime.push("initialize_model as _initialize"); }
    if body.contains("_initialize_attribute(") { runtime.push("initialize_attribute as _initialize_attribute"); }
    if body.contains("_initialize_reference(") { runtime.push("initialize_reference as _initialize_reference"); }
    if body.contains("_install_model(") { runtime.push("install_model as _install_model"); }
    if body.contains("_install_runtime_projection(") { runtime.push("install_runtime_projection as _install_runtime_projection"); }
    if body.contains("_load_mapping(") { runtime.push("load_mapping as _load_mapping"); }
    if !runtime.is_empty() {
        let _ = writeln!(output, "from ._runtime import {}", runtime.join(", "));
    }
    if body.contains("_install_runtime_projection(") {
        output.push_str(
            "from ._schema import PROJECTION_FINGERPRINT_JSON as _PROJECTION_FINGERPRINT_JSON\n\
             from ._schema import RUNTIME_PROJECTION_JSON as _RUNTIME_PROJECTION_JSON\n\
             from ._schema import SEMANTIC_SCHEMA_FINGERPRINT_JSON as _SEMANTIC_SCHEMA_FINGERPRINT_JSON\n",
        );
    }
    output.push('\n');
    output
}

fn render_model(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    let name = model.target_name().as_str();
    let base = match model.declaration().parent() {
        Some(parent) => projection.models()[parent].target_name().as_str(),
        None => match model.id().kind() {
            TypeKind::Attribute => "_Attribute",
            TypeKind::Entity => "_Entity",
            TypeKind::Relation => "_Relation",
            TypeKind::Struct => {
                return Err(facet_error("struct type appeared in the model facet"));
            }
        },
    };
    let _ = writeln!(output, "class {name}({base}):");
    if !stub {
        let id = canonical_text!(model.id());
        let _ = writeln!(output, "    __type_id__ = {}", python_string(&id)?);
        output.push_str("    __model_form__ = \"complete\"\n    __slots__ = ()\n");
    }
    if stub {
        render_descriptors(output, projection, model)?;
    }
    if model.id().kind() == TypeKind::Attribute {
        render_attribute_constructor(output, model, stub)?;
    } else if model.create().enabled() {
        render_constructor(output, projection, model, stub)?;
    } else if !stub {
        output.push_str("    pass\n");
    }
    output.push('\n');

    if let Some(reference) = model.reference_read().target_name() {
        let reference = reference.as_str();
        let base = model
            .declaration()
            .parent()
            .and_then(|parent| projection.models()[parent].reference_read().target_name())
            .map_or("_Reference", |name| name.as_str());
        let _ = writeln!(output, "class {reference}({base}):");
        if !stub {
            let id = canonical_text!(model.id());
            let _ = writeln!(output, "    __type_id__ = {}", python_string(&id)?);
            output.push_str("    __model_form__ = \"reference\"\n    __slots__ = ()\n");
        }
        render_reference_constructor(output, projection, model, stub)?;
        output.push('\n');
    }
    Ok(())
}

fn render_attribute_constructor(
    output: &mut String,
    model: &ModelProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    let value_type = model
        .declaration()
        .value_type()
        .ok_or_else(|| facet_error("attribute projection has no scalar value type"))?;
    if stub {
        let scalar = scalar_type(value_type);
        let _ = writeln!(output, "    @property\n    def value(self) -> {scalar}: ...");
        let _ = writeln!(output, "    def __init__(self, value: {scalar}) -> None: ...");
    } else {
        let scalar = scalar_type(value_type);
        let _ = writeln!(
            output,
            "    def __init__(self, value: {scalar}) -> None:\n        _initialize_attribute(self, value, \"{}\")",
            scalar_tag(value_type)
        );
    }
    Ok(())
}

fn scalar_tag(tag: ValueTypeTag) -> &'static str {
    match tag {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "long",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime_tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    }
}

fn render_descriptors(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<(), Diagnostic> {
    let owner = model.target_name().as_str();
    for (id, token) in model.query_tokens().fields() {
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == id)
            .map(|field| {
                projected_type(projection, field.value())
                    .map(|base| apply_multiplicity(base, field.multiplicity(), Position::Read))
            })
            .transpose()?
            .unwrap_or_else(|| "Never".to_owned());
        let assign = model
            .create()
            .fields()
            .iter()
            .find(|field| field.token() == id)
            .map(|field| {
                projected_type(projection, field.value())
                    .map(|base| apply_multiplicity(base, field.multiplicity(), Position::Create))
            })
            .transpose()?
            .unwrap_or_else(|| "Never".to_owned());
        let _ = writeln!(
            output,
            "    {}: _FieldDescriptor[{owner}, {read}, {assign}]",
            token.target_name().as_str()
        );
    }
    for (id, token) in model.query_tokens().roles() {
        let read = model
            .complete_read()
            .roles()
            .get(id)
            .map(|role| {
                projected_union(projection, role.players())
                    .map(|base| apply_multiplicity(base, role.multiplicity(), Position::Read))
            })
            .transpose()?
            .unwrap_or_else(|| "Never".to_owned());
        let assign = model
            .create()
            .roles()
            .get(id)
            .map(|role| {
                projected_union(projection, role.players())
                    .map(|base| apply_multiplicity(base, role.multiplicity(), Position::Create))
            })
            .transpose()?
            .unwrap_or_else(|| "Never".to_owned());
        let logical = token
            .accepted_players()
            .iter()
            .map(|id| projection.models()[id].target_name().as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        let logical = if logical.is_empty() { "Never" } else { &logical };
        let _ = writeln!(
            output,
            "    {}: _RoleDescriptor[{owner}, {logical}, {read}, {assign}]",
            token.target_name().as_str()
        );
    }
    Ok(())
}

fn render_constructor(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    let mut parameters = Vec::new();
    let mut names = Vec::new();
    for field in model.create().fields() {
        let name = model.query_tokens().fields()[field.token()]
            .target_name()
            .as_str();
        let annotation = apply_multiplicity(
            projected_type(projection, field.value())?,
            field.multiplicity(),
            Position::Create,
        );
        parameters.push(format!(
            "{name}: {annotation}{}",
            default_value(field.multiplicity())
        ));
        names.push(name);
    }
    for (id, role) in model.create().roles() {
        let name = model.query_tokens().roles()[id].target_name().as_str();
        let annotation = apply_multiplicity(
            projected_union(projection, role.players())?,
            role.multiplicity(),
            Position::Create,
        );
        parameters.push(format!(
            "{name}: {annotation}{}",
            default_value(role.multiplicity())
        ));
        names.push(name);
    }
    let signature = if parameters.is_empty() {
        "self".to_owned()
    } else {
        format!("self, *, {}", parameters.join(", "))
    };
    let _ = writeln!(output, "    def __init__({signature}) -> None:");
    if stub {
        output.push_str("        ...\n");
    } else {
        output.push_str("        _initialize(self, {\n");
        for name in names {
            let _ = writeln!(output, "            \"{name}\": {name},");
        }
        output.push_str("        })\n");
    }
    Ok(())
}

fn render_reference_constructor(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    let mut parameters = Vec::new();
    let mut names = Vec::new();
    for id in model.reference_read().key_fields() {
        let token = &model.query_tokens().fields()[id];
        let read = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == id)
            .ok_or_else(|| facet_error("reference key has no complete-read field"))?;
        let name = token.target_name().as_str();
        let annotation = apply_multiplicity(
            projected_type(projection, read.value())?,
            read.multiplicity(),
            Position::Read,
        );
        parameters.push(format!("{name}: {annotation}"));
        names.push(name);
        if stub {
            let _ = writeln!(
                output,
                "    @property\n    def {name}(self) -> {annotation}: ..."
            );
        }
    }
    let signature = if parameters.is_empty() {
        "self, iid: str".to_owned()
    } else {
        format!("self, iid: str, *, {}", parameters.join(", "))
    };
    let _ = writeln!(output, "    def __init__({signature}) -> None:");
    if stub {
        output.push_str("        ...\n");
    } else {
        output.push_str("        _initialize_reference(self, iid, {\n");
        for name in names {
            let _ = writeln!(output, "            \"{name}\": {name},");
        }
        output.push_str("        })\n");
    }
    Ok(())
}

fn render_struct(
    output: &mut String,
    structure: &type_bridge_contract::projection::StructProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    let name = structure.target_name().as_str();
    let _ = writeln!(output, "class {name}(_StructValue):");
    if !stub {
        let id = canonical_text!(structure.id());
        let _ = writeln!(output, "    __struct_id__ = {}", python_string(&id)?);
        let slots = structure
            .fields()
            .iter()
            .map(|field| format!("\"{}\"", field.target_name().as_str()))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(output, "    __slots__ = ({slots},)");
    }
    if stub {
        for field in structure.fields() {
            let base = scalar_type(field.value_type());
            let annotation = if field.optional() {
                format!("{base} | None")
            } else {
                base.to_owned()
            };
            let _ = writeln!(
                output,
                "    @property\n    def {}(self) -> {annotation}: ...",
                field.target_name().as_str()
            );
        }
    }
    let parameters = structure
        .fields()
        .iter()
        .map(|field| {
            let name = field.target_name().as_str();
            let base = scalar_type(field.value_type());
            if field.optional() {
                format!("{name}: {base} | None = None")
            } else {
                format!("{name}: {base}")
            }
        })
        .collect::<Vec<_>>();
    let signature = if parameters.is_empty() {
        "self".to_owned()
    } else {
        format!("self, *, {}", parameters.join(", "))
    };
    let _ = writeln!(output, "    def __init__({signature}) -> None:");
    if stub {
        output.push_str("        ...\n\n");
    } else {
        output.push_str("        _freeze_struct(self, {\n");
        for field in structure.fields() {
            let name = field.target_name().as_str();
            let _ = writeln!(output, "            \"{name}\": {name},");
        }
        output.push_str("        })\n\n");
    }
    Ok(())
}

fn render_function(
    output: &mut String,
    projection: &RuntimeProjection,
    function: &type_bridge_contract::projection::FunctionProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    let name = function.target_name().as_str();
    let parameters = function
        .parameters()
        .iter()
        .map(|parameter| projected_type(projection, parameter.type_ref()))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let returns = function_return(projection, function.returns())?;
    if stub {
        let _ = writeln!(
            output,
            "{name}: Final[FunctionRef[[{parameters}], {returns}]]"
        );
    } else {
        let id = canonical_text!(function.id());
        let metadata = canonical_text!(function);
        let _ = writeln!(
            output,
            "{name}: FunctionRef[[{parameters}], {returns}] = FunctionRef({}, _load_mapping({}))",
            python_string(&id)?,
            python_string(&metadata)?
        );
    }
    Ok(())
}

fn function_return(
    projection: &RuntimeProjection,
    returns: &FunctionReturnProjection,
) -> Result<String, Diagnostic> {
    match returns {
        FunctionReturnProjection::Scalar(element) => return_element(projection, element),
        FunctionReturnProjection::Tuple(elements) => Ok(format!(
            "tuple[{}]",
            elements
                .iter()
                .map(|element| return_element(projection, element))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )),
        FunctionReturnProjection::Stream(elements) => {
            let elements = elements
                .iter()
                .map(|element| return_element(projection, element))
                .collect::<Result<Vec<_>, _>>()?;
            let row = if elements.len() == 1 {
                elements[0].clone()
            } else {
                format!("tuple[{}]", elements.join(", "))
            };
            Ok(format!("Iterator[{row}]"))
        }
    }
}

fn return_element(
    projection: &RuntimeProjection,
    element: &FunctionReturnElementProjection,
) -> Result<String, Diagnostic> {
    let value = projected_type(projection, element.type_ref())?;
    Ok(if element.optional() {
        format!("{value} | None")
    } else {
        value
    })
}

fn projected_type(
    projection: &RuntimeProjection,
    value: &ProjectedTypeRef,
) -> Result<String, Diagnostic> {
    match value {
        ProjectedTypeRef::Scalar(tag) => Ok(scalar_type(*tag).to_owned()),
        ProjectedTypeRef::Model(model) => projected_model_use(projection, model),
        ProjectedTypeRef::Struct(id) => projection
            .structs()
            .get(id)
            .map(|structure| structure.target_name().as_str().to_owned())
            .ok_or_else(|| facet_error("projected type references an absent struct")),
    }
}

fn projected_model_use(
    projection: &RuntimeProjection,
    value: &ProjectedModelUse,
) -> Result<String, Diagnostic> {
    let model = projection
        .models()
        .get(value.id())
        .ok_or_else(|| facet_error("projected type references an absent model"))?;
    match value.form() {
        ProjectedModelForm::Complete => Ok(model.target_name().as_str().to_owned()),
        ProjectedModelForm::Reference => model
            .reference_read()
            .target_name()
            .map(|name| name.as_str().to_owned())
            .ok_or_else(|| facet_error("reference use targets a model without a reference facet")),
    }
}

fn projected_union(
    projection: &RuntimeProjection,
    values: &BTreeSet<ProjectedModelUse>,
) -> Result<String, Diagnostic> {
    if values.is_empty() {
        return Ok("Never".to_owned());
    }
    Ok(values
        .iter()
        .map(|value| projected_model_use(projection, value))
        .collect::<Result<Vec<_>, _>>()?
        .join(" | "))
}

#[derive(Clone, Copy)]
enum Position {
    Create,
    Read,
}

fn apply_multiplicity(
    base: String,
    multiplicity: ProjectedMultiplicity,
    position: Position,
) -> String {
    match (multiplicity.container(), position, multiplicity.required()) {
        (ProjectedContainer::Scalar, _, true) => base,
        (ProjectedContainer::Scalar, _, false) => format!("{base} | None"),
        (ProjectedContainer::Sequence, Position::Create, _) => format!("Sequence[{base}]"),
        (ProjectedContainer::Sequence, Position::Read, _) => format!("tuple[{base}, ...]"),
    }
}

fn default_value(multiplicity: ProjectedMultiplicity) -> &'static str {
    if multiplicity.required() {
        ""
    } else if multiplicity.container() == ProjectedContainer::Scalar {
        " = None"
    } else {
        " = ()"
    }
}

fn scalar_type(tag: ValueTypeTag) -> &'static str {
    match tag {
        ValueTypeTag::String => "str",
        ValueTypeTag::Long => "int",
        ValueTypeTag::Double => "float",
        ValueTypeTag::Boolean => "bool",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime | ValueTypeTag::DateTimeTz => "datetime",
        ValueTypeTag::Decimal => "Decimal",
        ValueTypeTag::Duration => "timedelta",
    }
}

fn python_string(value: &str) -> Result<String, Diagnostic> {
    Ok(String::from_utf8(to_canonical_json(&value)?)
        .expect("canonical JSON output is UTF-8"))
}

fn validate_projection(projection: &RuntimeProjection) -> Result<(), Diagnostic> {
    let mut public = BTreeMap::<String, String>::new();
    for name in PUBLIC_RUNTIME_NAMES.iter().chain(PUBLIC_SCHEMA_NAMES) {
        public.insert((*name).to_owned(), "generated runtime".to_owned());
    }
    let mut emitted = BTreeSet::<TypeId>::new();
    for id in projection.emission().model_shells() {
        let model = projection
            .models()
            .get(id)
            .ok_or_else(|| facet_error("emission plan references an absent model"))?;
        if model
            .declaration()
            .parent()
            .is_some_and(|parent| !emitted.contains(parent))
        {
            return Err(facet_error(
                "model shell order does not place the nominal parent first",
            ));
        }
        emitted.insert(id.clone());
        register_name(&mut public, model.target_name().as_str(), "model")?;
        if let Some(reference) = model.reference_read().target_name() {
            register_name(&mut public, reference.as_str(), "reference model")?;
        }
        validate_model(projection, model)?;
    }
    for id in projection.emission().structs() {
        let structure = projection
            .structs()
            .get(id)
            .ok_or_else(|| facet_error("emission plan references an absent struct"))?;
        register_name(&mut public, structure.target_name().as_str(), "struct")?;
        let mut fields = BTreeSet::new();
        if structure
            .fields()
            .iter()
            .any(|field| !fields.insert(field.target_name().as_str()))
        {
            return Err(name_error("projected struct field names collide"));
        }
    }
    for id in projection.emission().functions() {
        let function = projection
            .functions()
            .get(id)
            .ok_or_else(|| facet_error("emission plan references an absent function"))?;
        register_name(&mut public, function.target_name().as_str(), "function")?;
        let mut parameters = BTreeSet::new();
        for parameter in function.parameters() {
            if !parameters.insert(parameter.target_name().as_str()) {
                return Err(name_error("projected function parameter names collide"));
            }
            projected_type(projection, parameter.type_ref())?;
        }
        function_return(projection, function.returns())?;
    }
    Ok(())
}

fn validate_model(
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<(), Diagnostic> {
    let mut members = MODEL_RESERVED_NAMES.iter().copied().collect::<BTreeSet<_>>();
    for (id, token) in model.query_tokens().fields() {
        if !members.insert(token.target_name().as_str()) {
            return Err(name_error("projected model member collides with a reserved name"));
        }
        if let Some(create) = model.create().fields().iter().find(|field| field.token() == id) {
            if create.multiplicity() != token.multiplicity() {
                return Err(facet_error("create field disagrees with its query token"));
            }
            projected_type(projection, create.value())?;
        }
        if let Some(read) = model
            .complete_read()
            .fields()
            .iter()
            .find(|field| field.token() == id)
        {
            if read.multiplicity() != token.multiplicity() {
                return Err(facet_error("read field disagrees with its query token"));
            }
            projected_type(projection, read.value())?;
        }
    }
    for (id, token) in model.query_tokens().roles() {
        if !members.insert(token.target_name().as_str()) {
            return Err(name_error("projected model member names collide"));
        }
        if let Some(create) = model.create().roles().get(id) {
            validate_role_use(projection, token, create.players(), create.multiplicity())?;
        }
        if let Some(read) = model.complete_read().roles().get(id) {
            validate_role_use(projection, token, read.players(), read.multiplicity())?;
        }
    }
    for key in model.reference_read().key_fields() {
        if !model.query_tokens().fields().contains_key(key)
            || !model
                .complete_read()
                .fields()
                .iter()
                .any(|field| field.token() == key)
        {
            return Err(facet_error(
                "reference key is absent from query or complete-read facets",
            ));
        }
    }
    for playing in model.declaration().direct_plays() {
        if !projection.playing_facts().contains_key(playing) {
            return Err(facet_error(
                "declared playing fact is absent from projection metadata",
            ));
        }
    }
    Ok(())
}

fn validate_role_use(
    projection: &RuntimeProjection,
    token: &type_bridge_contract::projection::RoleTokenProjection,
    players: &BTreeSet<ProjectedModelUse>,
    multiplicity: ProjectedMultiplicity,
) -> Result<(), Diagnostic> {
    if multiplicity != token.multiplicity()
        || players
            .iter()
            .any(|player| !token.accepted_players().contains(player.id()))
    {
        return Err(facet_error("role facet disagrees with its query token"));
    }
    for player in players {
        projected_model_use(projection, player)?;
    }
    Ok(())
}

fn register_name(
    names: &mut BTreeMap<String, String>,
    name: &str,
    kind: &str,
) -> Result<(), Diagnostic> {
    if let Some(previous) = names.insert(name.to_owned(), kind.to_owned()) {
        return Err(name_error(format!(
            "public name {name} collides between {previous} and {kind}"
        )));
    }
    Ok(())
}

fn facet_error(message: &'static str) -> Diagnostic {
    invalid("python_emitter_facet_mismatch", message)
}

fn name_error(message: impl Into<String>) -> Diagnostic {
    invalid("python_emitter_name_collision", message)
}
