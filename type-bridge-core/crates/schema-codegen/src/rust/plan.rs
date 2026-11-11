//! Generated nominal authoring helpers over the shared SDK recorder.
use super::render::{
    concrete_descendants, facet_error, header, model, role_owner_and_union, scalar_type,
};
use std::fmt::Write as _;
use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::projection::{ProjectedContainer, RuntimeProjection};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::value::ValueTypeTag;

fn scalar(projection: &RuntimeProjection, field: &OwnsFactId) -> Result<&'static str, Diagnostic> {
    let id = TypeId::new(TypeKind::Attribute, field.attribute().label().as_str())?;
    let tag = projection.models()[&id]
        .declaration()
        .value_type()
        .ok_or_else(|| facet_error("plan field has no scalar domain"))?;
    Ok(if tag == ValueTypeTag::Double {
        "f64"
    } else {
        scalar_type(tag)
    })
}

pub(super) fn render(projection: &RuntimeProjection) -> Result<String, Diagnostic> {
    let mut output = String::from(header());
    if !projection
        .models()
        .keys()
        .any(|id| matches!(id.kind(), TypeKind::Entity | TypeKind::Relation))
    {
        return Ok(output);
    }
    output.push_str("use crate::*;\nuse type_bridge::plan::recording;\nuse type_bridge::{Exact, SelectionMode, Subtypes};\n\n");
    for id in projection.emission().model_shells() {
        let model = model(projection, id)?;
        if !matches!(id.kind(), TypeKind::Entity | TypeKind::Relation) {
            continue;
        }
        let name = model.target_name().as_str();
        let token_type = model
            .query_tokens()
            .target_name()
            .ok_or_else(|| facet_error("query token name is absent"))?
            .as_str();
        let roles = model
            .complete_read()
            .roles()
            .keys()
            .map(|id| {
                Ok(format!(
                    "({:?}, {})",
                    String::from_utf8(to_canonical_json(id)?)
                        .map_err(|_| facet_error("non UTF-8 role identity"))?,
                    false
                ))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?
            .join(", ");
        let roles = format!("&[{roles}]");
        let _ = writeln!(
            output,
            "pub struct {name}Handle<Mode: SelectionMode = Exact> {{ __tb_handle: recording::Handle<{name}, Mode> }}\nimpl<Mode: SelectionMode> Clone for {name}Handle<Mode> {{ fn clone(&self) -> Self {{ Self {{ __tb_handle: self.__tb_handle.clone() }} }} }}\nimpl<Mode: SelectionMode> recording::BoundModel for {name}Handle<Mode> {{ type Model = {name}; type Mode = Mode; fn player_reference(&self) -> type_bridge::Result<recording::PlayerReference> {{ recording::BoundModel::player_reference(&self.__tb_handle) }} }}\nimpl<Mode: SelectionMode> {name}Handle<Mode> {{"
        );
        for (id, field) in model.query_tokens().fields() {
            let ty = scalar(projection, id)?;
            let field = field.target_name().as_str();
            let _ = writeln!(
                output,
                "pub fn {field}(&self) -> recording::Value<{ty}> {{ self.__tb_handle.value({token_type}::{field}.owns_id_json()) }}"
            );
        }
        let _ = writeln!(
            output,
            "pub fn where_<C: recording::FilterFor<{name}, Mode>>(self, condition: C) -> Self {{ Self {{ __tb_handle: self.__tb_handle.where_(condition) }} }}\npub fn models(&self, p: &mut recording::Recording<AppSchema>) -> Plan<Vec<<Mode as recording::ModelSelection<{name}>>::Output>> where Mode: recording::ModelSelection<{name}> {{ self.__tb_handle.models(p, {roles}) }}\npub fn delete(&self, p: &mut recording::Recording<AppSchema>) -> Plan<type_bridge::plan::Matched> {{ self.__tb_handle.delete(p); p.matched() }}\npub fn set<'a>(&'a self, p: &'a mut recording::Recording<AppSchema>) -> {name}Set<'a, Mode> {{ {name}Set {{ __tb_handle: self, __tb_recording: p }} }}\n}}"
        );
        let _ = writeln!(
            output,
            "pub struct {name}Set<'a, Mode: SelectionMode = Exact> {{ __tb_handle: &'a {name}Handle<Mode>, __tb_recording: &'a mut recording::Recording<AppSchema> }}\nimpl<Mode: SelectionMode> {name}Set<'_, Mode> {{"
        );
        for (id, field) in model.query_tokens().fields() {
            if field.is_key() || field.multiplicity().container() != ProjectedContainer::Scalar {
                continue;
            }
            let ty = scalar(projection, id)?;
            let field = field.target_name().as_str();
            let _ = writeln!(
                output,
                "pub fn {field}(self, value: impl Into<recording::Argument<{ty}>>) -> Self {{ self.__tb_handle.__tb_handle.set(self.__tb_recording, {token_type}::{field}.owns_id_json(), value); self }}"
            );
        }
        let _ = writeln!(
            output,
            "pub fn models(self) -> Plan<Vec<<Mode as recording::ModelSelection<{name}>>::Output>> where Mode: recording::ModelSelection<{name}> {{ self.__tb_handle.models(self.__tb_recording) }}\n}}"
        );
        let _ = writeln!(
            output,
            "impl<Mode: SelectionMode + recording::ModelSelection<{name}>> From<{name}Set<'_, Mode>> for Plan<Vec<Mode::Output>> {{ fn from(value: {name}Set<'_, Mode>) -> Self {{ value.models() }} }}"
        );
        let _ = writeln!(
            output,
            "impl<Mode: SelectionMode + recording::ModelSelection<{name}>> {name}Handle<Mode> {{\npub fn unset<Owner, V>(&self, p: &mut recording::Recording<AppSchema>, field: FieldToken<Owner, V>) -> Plan<Vec<Mode::Output>> where {name}: NominalUpcast<Owner>, Owner: Model<Schema = AppSchema> {{ self.__tb_handle.unset(p, field.owns_id_json()); self.models(p) }}\npub fn delete_returning(&self, p: &mut recording::Recording<AppSchema>) -> Plan<Vec<Mode::Output>> {{ let result = self.models(p); self.__tb_handle.delete(p); result }}\n}}"
        );
        let link_roles = model
            .query_tokens()
            .roles()
            .values()
            .filter(|role| role.multiplicity().collection_mode().is_unordered())
            .collect::<Vec<_>>();
        if !link_roles.is_empty() {
            let _ = writeln!(
                output,
                "impl<Mode: SelectionMode> {name}Handle<Mode> {{\npub fn link<'a>(&'a self, p: &'a mut recording::Recording<AppSchema>) -> {name}Link<'a, Mode> {{ {name}Link {{ __tb_handle: self, __tb_recording: p, __tb_unlink: false }} }}\npub fn unlink<'a>(&'a self, p: &'a mut recording::Recording<AppSchema>) -> {name}Link<'a, Mode> {{ {name}Link {{ __tb_handle: self, __tb_recording: p, __tb_unlink: true }} }}\n}}\npub struct {name}Link<'a, Mode: SelectionMode = Exact> {{ __tb_handle: &'a {name}Handle<Mode>, __tb_recording: &'a mut recording::Recording<AppSchema>, __tb_unlink: bool }}\nimpl<Mode: SelectionMode> {name}Link<'_, Mode> {{"
            );
            for role in link_roles {
                let field = role.target_name().as_str();
                let (_, players) = role_owner_and_union(projection, role.role())?;
                let _ = writeln!(
                    output,
                    "pub fn {field}(self, value: &dyn recording::Player<{players}>) -> Self {{ self.__tb_handle.__tb_handle.link(self.__tb_recording, {token_type}::{field}, value, self.__tb_unlink); self }}"
                );
            }
            let _ = writeln!(
                output,
                "pub fn models(self) -> Plan<Vec<Mode::Output>> where Mode: recording::ModelSelection<{name}> {{ self.__tb_handle.models(self.__tb_recording) }}\n}}"
            );
        }
        let _ = writeln!(output, "impl {name} {{");
        let field_count = model.query_tokens().fields().len();
        let _ = writeln!(
            output,
            "/// Begin a named match; call `.handle()` after supplying its fields.\npub fn matching_by(p: &mut recording::Recording<AppSchema>) -> {name}Match {{ {name}Match {{ __tb_handle: {name}Handle {{ __tb_handle: p.matching::<Self>(false, None) }}, __tb_seen: [false; {field_count}] }} }}"
        );
        let keys = model
            .query_tokens()
            .fields()
            .iter()
            .filter(|(_, field)| field.is_key())
            .collect::<Vec<_>>();
        if let [(id, key)] = keys.as_slice() {
            let ty = scalar(projection, id)?;
            let field = key.target_name().as_str();
            let _ = writeln!(
                output,
                "pub fn matching(p: &mut recording::Recording<AppSchema>, key: impl Into<{ty}>) -> {name}Handle {{ {name}Handle {{ __tb_handle: p.matching::<Self>(false, None).key({token_type}::{field}.owns_id_json(), key.into()) }} }}\npub fn subtypes(p: &mut recording::Recording<AppSchema>, key: impl Into<{ty}>) -> {name}Handle<Subtypes> {{ {name}Handle {{ __tb_handle: p.subtypes::<Self>(None).key({token_type}::{field}.owns_id_json(), key.into()) }} }}"
            );
        } else {
            let _ = writeln!(
                output,
                "pub fn matching(p: &mut recording::Recording<AppSchema>) -> {name}Handle {{ {name}Handle {{ __tb_handle: p.matching::<Self>(false, None) }} }}\npub fn subtypes(p: &mut recording::Recording<AppSchema>) -> {name}Handle<Subtypes> {{ {name}Handle {{ __tb_handle: p.subtypes::<Self>(None) }} }}"
            );
        }
        let _ = writeln!(
            output,
            "pub fn by_iid(p: &mut recording::Recording<AppSchema>, iid: &type_bridge::Iid) -> {name}Handle<Subtypes> {{ {name}Handle {{ __tb_handle: p.subtypes::<Self>(Some(iid)) }} }}\n}}"
        );
        let _ = writeln!(
            output,
            "pub struct {name}Match {{ __tb_handle: {name}Handle, __tb_seen: [bool; {field_count}] }}\nimpl {name}Match {{\npub fn handle(self) -> {name}Handle {{ self.__tb_handle }}"
        );
        for (index, (id, field)) in model.query_tokens().fields().iter().enumerate() {
            let ty = scalar(projection, id)?;
            let field = field.target_name().as_str();
            let _ = writeln!(
                output,
                "pub fn {field}(mut self, value: impl Into<recording::Comparable<{ty}>>) -> Self {{ if self.__tb_seen[{index}] {{ self.__tb_handle.__tb_handle.duplicate_match_field(); return self; }} self.__tb_seen[{index}] = true; self.__tb_handle = self.__tb_handle.where_({token_type}::{field}.eq(value)); self }}"
            );
        }
        output.push_str("}\n");
        creates(&mut output, projection, model, token_type, &roles)?;
        if !model.declaration().is_abstract() {
            let _ = writeln!(
                output,
                "impl recording::ModelResult for {name} {{ type Model = Self; type Mode = Exact; }}"
            );
        }
        let descendants = concrete_descendants(projection, id)?;
        if descendants.len() >= 2 || (model.declaration().is_abstract() && !descendants.is_empty())
        {
            let _ = writeln!(
                output,
                "impl recording::ModelResult for {name}Family {{ type Model = {name}; type Mode = Subtypes; }}"
            );
        }
    }
    Ok(output)
}

fn creates(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &type_bridge_contract::projection::ModelProjection,
    token_type: &str,
    roles: &str,
) -> Result<(), Diagnostic> {
    if !crate::supports_scalar_plan_writes(model) {
        return Ok(());
    }
    let name = model.target_name().as_str();
    if model.id().kind() == TypeKind::Entity {
        let _ = writeln!(output, "pub struct {name}Values {{");
        for field in model.create().fields() {
            let ty = scalar(projection, field.token())?;
            let ty = if field.multiplicity().required() {
                ty.to_owned()
            } else {
                format!("Option<{ty}>")
            };
            let field = model.query_tokens().fields()[field.token()]
                .target_name()
                .as_str();
            let _ = writeln!(output, "pub {field}: {ty},");
        }
        let _ = writeln!(output, "}}\nimpl {name} {{");
        for (method, put) in [("insert_many", false), ("put_many", true)] {
            let _ = writeln!(
                output,
                "pub fn {method}(p: &mut recording::Recording<AppSchema>, rows: impl IntoIterator<Item = {name}Values>) -> Plan<Vec<Self>> {{\nlet rows = rows.into_iter().map(|values| {{"
            );
            if model.create().fields().is_empty() {
                output.push_str("let _ = values; Vec::new()\n");
            } else {
                output.push_str("vec![\n");
                for field in model.create().fields() {
                    let name = model.query_tokens().fields()[field.token()]
                        .target_name()
                        .as_str();
                    if field.multiplicity().required() {
                        let _ = writeln!(
                            output,
                            "Some(recording::Scalar::into_plan_scalar(values.{name})),"
                        );
                    } else {
                        let _ = writeln!(
                            output,
                            "values.{name}.map(recording::Scalar::into_plan_scalar),"
                        );
                    }
                }
                output.push_str("]\n");
            }
            let columns = model
                .create()
                .fields()
                .iter()
                .map(|field| {
                    let name = model.query_tokens().fields()[field.token()]
                        .target_name()
                        .as_str();
                    format!("{token_type}::{name}.owns_id_json()")
                })
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                output,
                "}}).collect();\np.bulk::<Self>({put}, &[{columns}], rows).models(p, {roles})\n}}"
            );
        }
        output.push_str("}\n");
    }
    let lifetime = if model.create().roles().is_empty() {
        ""
    } else {
        "<'a>"
    };
    let _ = writeln!(output, "pub struct {name}Insert{lifetime} {{");
    for field in model.create().fields() {
        let ty = scalar(projection, field.token())?;
        let ty = format!("recording::Argument<{ty}>");
        let ty = if field.multiplicity().required() {
            ty
        } else {
            format!("Option<{ty}>")
        };
        let field = model.query_tokens().fields()[field.token()]
            .target_name()
            .as_str();
        let _ = writeln!(output, "pub {field}: {ty},");
    }
    for (id, create) in model.create().roles() {
        let token = &model.query_tokens().roles()[id];
        let (_, players) = role_owner_and_union(projection, token.role())?;
        let ty = format!("&'a dyn recording::Player<{players}>");
        let ty = if create.multiplicity().container() != ProjectedContainer::Scalar {
            format!("Vec<{ty}>")
        } else if !create.multiplicity().required() {
            format!("Option<{ty}>")
        } else {
            ty
        };
        let _ = writeln!(output, "pub {}: {ty},", token.target_name().as_str());
    }
    output.push_str("}\n");
    let _ = writeln!(output, "impl {name} {{");
    for (method, put) in [("insert", false), ("put", true)] {
        let values_lifetime = if lifetime.is_empty() { "" } else { "<'_>" };
        let _ = writeln!(
            output,
            "pub fn {method}(p: &mut recording::Recording<AppSchema>, values: {name}Insert{values_lifetime}) -> Plan<Vec<{name}>> {{\nlet created = p.create::<Self>({put});"
        );
        if model.create().fields().is_empty() && model.create().roles().is_empty() {
            output.push_str("let _ = values;\n");
        }
        for field in model.create().fields() {
            let ty = scalar(projection, field.token())?;
            let field_name = model.query_tokens().fields()[field.token()]
                .target_name()
                .as_str();
            if field.multiplicity().required() {
                let _ = writeln!(
                    output,
                    "created.field::<{ty}>(p, {token_type}::{field_name}.owns_id_json(), values.{field_name});"
                );
            } else {
                let _ = writeln!(
                    output,
                    "if let Some(value) = values.{field_name} {{ created.field::<{ty}>(p, {token_type}::{field_name}.owns_id_json(), value); }}"
                );
            }
        }
        for (id, create) in model.create().roles() {
            let field = model.query_tokens().roles()[id].target_name().as_str();
            let statement = format!("created.player(p, {token_type}::{field}, value);");
            if create.multiplicity().container() != ProjectedContainer::Scalar {
                let _ = writeln!(output, "for value in values.{field} {{ {statement} }}");
            } else if !create.multiplicity().required() {
                let _ = writeln!(
                    output,
                    "if let Some(value) = values.{field} {{ {statement} }}"
                );
            } else {
                let _ = writeln!(
                    output,
                    "created.player(p, {token_type}::{field}, values.{field});"
                );
            }
        }
        let _ = writeln!(output, "created.models(p, {roles})\n}}");
    }
    output.push_str("}\n");
    Ok(())
}
