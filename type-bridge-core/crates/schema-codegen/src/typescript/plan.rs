//! Model-specific types over the shared plan recorder.
use super::*;

fn tag(projection: &RuntimeProjection, field: &OwnsFactId) -> Result<ValueTypeTag, Diagnostic> {
    let id = TypeId::new(TypeKind::Attribute, field.attribute().label().as_str())?;
    projection.models()[&id]
        .declaration()
        .value_type()
        .ok_or_else(|| facet_error("plan field has no scalar domain"))
}

pub(super) fn scalar(
    projection: &RuntimeProjection,
    field: &OwnsFactId,
) -> Result<&'static str, Diagnostic> {
    Ok(scalar_type(tag(projection, field)?))
}

fn player_type(
    projection: &RuntimeProjection,
    role: &type_bridge_contract::projection::RoleTokenProjection,
) -> String {
    let players = role
        .accepted_players()
        .iter()
        .map(|id| format!("{}Handle", projection.models()[id].target_name().as_str()))
        .collect::<Vec<_>>();
    if players.is_empty() {
        "never".to_owned()
    } else {
        players.join(" | ")
    }
}

pub(super) fn shell(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<(), Diagnostic> {
    if !matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation) {
        return Ok(());
    }
    let name = model.target_name().as_str();
    let _ = writeln!(
        output,
        "export namespace {name} {{\n  export interface Values {{"
    );
    for field in model.create().fields() {
        let token = &model.query_tokens().fields()[field.token()];
        let value = scalar(projection, field.token())?;
        let value = if field.multiplicity().container() == ProjectedContainer::Scalar {
            value.to_owned()
        } else {
            format!("readonly {value}[]")
        };
        let optional = if field.multiplicity().required() {
            ""
        } else {
            "?"
        };
        let _ = writeln!(
            output,
            "    readonly {}{optional}: {value};",
            token.target_name().as_str()
        );
    }
    output.push_str("  }\n  export interface Match {\n");
    for (id, token) in model.query_tokens().fields() {
        let ty = scalar(projection, id)?;
        let _ = writeln!(
            output,
            "    readonly {}?: {ty} | PlanValue<{ty}> | PlanInput<{ty}>;",
            token.target_name().as_str()
        );
    }
    output.push_str("  }\n  export interface Insert {\n");
    for field in model.create().fields() {
        let token = &model.query_tokens().fields()[field.token()];
        let ty = scalar(projection, field.token())?;
        let optional = if field.multiplicity().required() {
            ""
        } else {
            "?"
        };
        let _ = writeln!(
            output,
            "    readonly {}{optional}: {ty} | PlanValue<{ty}> | PlanExpr<{ty}> | PlanInput<{ty}>;",
            token.target_name().as_str()
        );
    }
    for (id, create) in model.create().roles() {
        let role = &model.query_tokens().roles()[id];
        let ty = player_type(projection, role);
        let ty = if create.multiplicity().container() == ProjectedContainer::Scalar {
            ty
        } else {
            format!("readonly ({ty})[]")
        };
        let optional = if create.multiplicity().required() {
            ""
        } else {
            "?"
        };
        let _ = writeln!(
            output,
            "    readonly {}{optional}: {ty};",
            role.target_name().as_str()
        );
    }
    let _ = writeln!(output, "  }}\n}}\n\nexport interface {name}PlanMethods {{");
    let keys = model
        .query_tokens()
        .fields()
        .iter()
        .filter(|(_, token)| token.is_key())
        .collect::<Vec<_>>();
    for method in ["match", "subtypes"] {
        if let [(field, _)] = &keys[..] {
            let ty = scalar(projection, field)?;
            let _ = writeln!(
                output,
                "  {method}(key: {ty} | PlanInput<{ty}>): {name}Handle;"
            );
        }
        let _ = writeln!(output, "  {method}(fields?: {name}.Match): {name}Handle;");
    }
    let _ = writeln!(output, "  byIid(iid: string): {name}Handle;");
    if crate::supports_scalar_plan_writes(model) {
        let _ = writeln!(
            output,
            "  insert(values: {name}.Insert): Plan<{name}[]>;\n  put(values: {name}.Insert): Plan<{name}[]>;"
        );
        if model.id().kind() == TypeKind::Entity {
            let _ = writeln!(
                output,
                "  insertMany(rows: Iterable<{name}.Values>): Plan<{name}[]>;\n  putMany(rows: Iterable<{name}.Values>): Plan<{name}[]>;"
            );
        }
    }
    let _ = writeln!(
        output,
        "}}\n\nexport class {name}Handle extends PlanHandle<{name}> {{"
    );
    for (id, token) in model.query_tokens().fields() {
        let ty = scalar(projection, id)?;
        let field = token.target_name().as_str();
        let _ = writeln!(
            output,
            "  get {field}(): PlanValue<{ty}> {{ return this.field<{ty}>({}); }}",
            js_string(field)?
        );
    }
    let sets = model
        .query_tokens()
        .fields()
        .iter()
        .filter(|(_, field)| {
            !field.is_key() && field.multiplicity().container() == ProjectedContainer::Scalar
        })
        .map(|(id, field)| {
            let ty = scalar(projection, id)?;
            Ok(format!(
                "readonly {}?: {ty} | PlanValue<{ty}> | PlanExpr<{ty}> | PlanInput<{ty}>",
                field.target_name().as_str()
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    if !sets.is_empty() {
        let _ = writeln!(
            output,
            "  set(values: {{ {} }}): Plan<{name}[]> {{ return this.setFields(values); }}",
            sets.join("; ")
        );
    }
    let roles = model
        .query_tokens()
        .roles()
        .values()
        .filter(|role| role.multiplicity().collection_mode().is_unordered())
        .map(|role| {
            let ty = player_type(projection, role);
            let ty = if role.multiplicity().container() == ProjectedContainer::Scalar {
                ty
            } else {
                format!("{ty} | readonly ({ty})[]")
            };
            format!("readonly {}?: {ty}", role.target_name().as_str())
        })
        .collect::<Vec<_>>();
    if !roles.is_empty() {
        let _ = writeln!(
            output,
            "  link(players: {{ {} }}): Plan<{name}[]> {{ return this.linkPlayers(players); }}\n  unlink(players: {{ {} }}): Plan<{name}[]> {{ return this.linkPlayers(players, true); }}",
            roles.join("; "),
            roles.join("; ")
        );
    }
    output.push_str("}\n\n");
    Ok(())
}

pub(super) fn link_tail(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<(), Diagnostic> {
    if !matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation) {
        output.push_str("});\n");
        return Ok(());
    }
    let name = model.target_name().as_str();
    let keys = model
        .query_tokens()
        .fields()
        .iter()
        .filter(|(_, token)| token.is_key())
        .collect::<Vec<_>>();
    let (key_name, input_type) = if let [(id, token)] = &keys[..] {
        (
            js_string(token.target_name().as_str())?,
            format!(
                "{name}.Match | {ty} | PlanInput<{ty}>",
                ty = scalar(projection, id)?
            ),
        )
    } else {
        ("null".to_owned(), format!("{name}.Match"))
    };
    let _ = writeln!(
        output,
        "}}, (token) => {{\n  const definition = () => planDefinition<{name}>(token);\n  return {{\n    match: (input?: {input_type}) => planModelMatch(definition(), {name}Handle, {key_name}, input, false),\n    subtypes: (input?: {input_type}) => planModelMatch(definition(), {name}Handle, {key_name}, input, true),\n    byIid: (iid: string) => planMatch(definition(), {name}Handle, {{}}, {{ iid }}),"
    );
    if crate::supports_scalar_plan_writes(model) {
        let _ = writeln!(
            output,
            "    insert: (values: {name}.Insert) => planModelInsert(definition(), values, false),\n    put: (values: {name}.Insert) => planModelInsert(definition(), values, true),"
        );
        if model.id().kind() == TypeKind::Entity {
            let _ = writeln!(
                output,
                "    insertMany: (rows: Iterable<{name}.Values>) => planInsertMany(definition(), rows),\n    putMany: (rows: Iterable<{name}.Values>) => planInsertMany(definition(), rows, true),"
            );
        }
    }
    output.push_str("  };\n});\n");
    Ok(())
}
