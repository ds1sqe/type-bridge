//! Compiler-checked C declarations from the shared native view-layout authority.
use super::*;
use type_bridge_contract::projection::c_view::{
    CViewKey, CViewLayout, CViewLayouts, CViewMemberKind,
};

fn view_name(
    projection: &RuntimeProjection,
    prefix: &str,
    key: &CViewKey,
) -> Result<String, Diagnostic> {
    match key {
        CViewKey::Exact { model, player } => name(
            &projection.models()[model],
            if *player {
                "plan_player_exact_view_t"
            } else {
                "plan_exact_view_t"
            },
        ),
        CViewKey::Model { model, player } => name(
            &projection.models()[model],
            if *player {
                "plan_player_view_t"
            } else {
                "plan_view_t"
            },
        ),
        CViewKey::Role { owner, role } => {
            let model = &projection.models()[owner];
            let role = &model.query_tokens().roles()[role];
            name(
                model,
                &format!(
                    "plan_{}_player_view_t",
                    roles::escaped_member(prefix, role.target_name())?
                ),
            )
        }
    }
}
pub(super) fn symbols(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<Vec<String>, Diagnostic> {
    let layouts = CViewLayouts::for_projection(projection)?;
    let mut symbols = layouts
        .iter()
        .map(|(key, _)| view_name(projection, prefix, key))
        .collect::<Result<Vec<_>, _>>()?;
    symbols.extend(
        models(projection)
            .map(|model| name(model, "result_at"))
            .collect::<Result<Vec<_>, _>>()?,
    );
    symbols.extend(
        models(projection)
            .map(|model| name(model, "rows_model_at"))
            .collect::<Result<Vec<_>, _>>()?,
    );
    symbols.push(generated_symbol(prefix, "plan_view_layout_digest")?);
    Ok(symbols)
}
fn dependencies(layout: &CViewLayout) -> Vec<CViewKey> {
    if let Some(alias) = &layout.alias {
        return vec![alias.clone()];
    }
    let mut deps = layout
        .variants
        .iter()
        .map(|variant| variant.layout.clone())
        .collect::<Vec<_>>();
    if let CViewKey::Exact { model, .. } = &layout.key {
        deps.extend(
            layout
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    CViewMemberKind::Role { role } => Some(CViewKey::Role {
                        owner: model.clone(),
                        role: role.clone(),
                    }),
                    _ => None,
                }),
        );
    }
    deps
}
fn concrete_layout<'a>(layouts: &'a CViewLayouts, mut layout: &'a CViewLayout) -> &'a CViewLayout {
    while let Some(alias) = &layout.alias {
        layout = layouts
            .get(alias)
            .expect("shared layout aliases are closed");
    }
    layout
}
fn member_name(
    projection: &RuntimeProjection,
    prefix: &str,
    layout: &CViewLayout,
    kind: &CViewMemberKind,
) -> Result<String, Diagnostic> {
    let model = match &layout.key {
        CViewKey::Exact { model, .. } => Some(&projection.models()[model]),
        _ => None,
    };
    Ok(match kind {
        CViewMemberKind::Iid => "iid".to_owned(),
        CViewMemberKind::Kind => "kind".to_owned(),
        CViewMemberKind::Deleted => "deleted".to_owned(),
        CViewMemberKind::Field { field, .. } => member(
            prefix,
            &model.expect("exact field layout").query_tokens().fields()[field],
        )?,
        CViewMemberKind::FieldPresent { field } => format!(
            "has_{}",
            member(
                prefix,
                &model.expect("exact field layout").query_tokens().fields()[field]
            )?
        ),
        CViewMemberKind::FieldCount { field } => format!(
            "{}_count",
            member(
                prefix,
                &model.expect("exact field layout").query_tokens().fields()[field]
            )?
        ),
        CViewMemberKind::Role { role } => roles::escaped_member(
            prefix,
            model.expect("exact role layout").query_tokens().roles()[role].target_name(),
        )?,
        CViewMemberKind::RoleCount { role } => format!(
            "{}_count",
            roles::escaped_member(
                prefix,
                model.expect("exact role layout").query_tokens().roles()[role].target_name()
            )?
        ),
    })
}
fn scalar_type(domain: ValueTypeTag) -> &'static str {
    match domain {
        ValueTypeTag::Long => "int64_t",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "bool",
        _ => "const char *",
    }
}
fn render_one(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
    layouts: &CViewLayouts,
    key: &CViewKey,
    emitted: &mut BTreeSet<CViewKey>,
) -> Result<(), Diagnostic> {
    if emitted.contains(key) {
        return Ok(());
    }
    let layout = layouts.get(key).expect("shared view inventory is closed");
    for dependency in dependencies(layout) {
        render_one(output, projection, prefix, layouts, &dependency, emitted)?;
    }
    let ty = view_name(projection, prefix, key)?;
    if let Some(alias) = &layout.alias {
        let _ = writeln!(
            output,
            "typedef {} {ty};",
            view_name(projection, prefix, alias)?
        );
    } else {
        let _ = writeln!(output, "typedef struct {ty} {{");
        let mut names = BTreeSet::new();
        for item in &layout.members {
            let member = member_name(projection, prefix, layout, &item.kind)?;
            if !names.insert(member.clone()) {
                return Err(invalid(
                    "c_emitter_generated_symbol_collision",
                    "generated C view members collide",
                ));
            }
            let field_type = match &item.kind {
                CViewMemberKind::Iid => "const char *".to_owned(),
                CViewMemberKind::Kind => "uint32_t".to_owned(),
                CViewMemberKind::Deleted | CViewMemberKind::FieldPresent { .. } => {
                    "bool".to_owned()
                }
                CViewMemberKind::FieldCount { .. } | CViewMemberKind::RoleCount { .. } => {
                    "size_t".to_owned()
                }
                CViewMemberKind::Field { domain, list, .. } => {
                    let scalar = scalar_type(*domain);
                    if *list {
                        if scalar == "const char *" {
                            "const char *const *".to_owned()
                        } else {
                            format!("const {scalar} *")
                        }
                    } else {
                        scalar.to_owned()
                    }
                }
                CViewMemberKind::Role { role } => {
                    let CViewKey::Exact { model, .. } = &layout.key else {
                        unreachable!("role fields belong to exact roots")
                    };
                    format!(
                        "const {} *",
                        view_name(
                            projection,
                            prefix,
                            &CViewKey::Role {
                                owner: model.clone(),
                                role: role.clone()
                            }
                        )?
                    )
                }
            };
            let _ = writeln!(output, "{field_type} {member};");
        }
        if layout.union_offset.is_some() {
            output.push_str("union {\n");
            if layout.variants.is_empty() {
                output.push_str("unsigned char _empty;\n");
            }
            for variant in &layout.variants {
                let field = roles::escaped_member(
                    prefix,
                    projection.models()[&variant.model].target_name(),
                )?;
                let variant_type = view_name(projection, prefix, &variant.layout)?;
                let _ = writeln!(output, "{variant_type} {field};");
            }
            output.push_str("} value;\n");
        }
        let _ = writeln!(output, "}} {ty};");
    }
    for (guard, assertion, align) in [
        ("#ifdef __cplusplus", "static_assert", "alignof"),
        ("#else", "_Static_assert", "_Alignof"),
    ] {
        let _ = writeln!(
            output,
            "{guard}\n{assertion}(sizeof({ty}) == {}u, \"C view size\");\n{assertion}({align}({ty}) == {}u, \"C view alignment\");",
            layout.size, layout.alignment
        );
        let concrete = concrete_layout(layouts, layout);
        for item in &layout.members {
            let field = member_name(projection, prefix, concrete, &item.kind)?;
            let _ = writeln!(
                output,
                "{assertion}(offsetof({ty}, {field}) == {}u, \"C view member offset\");",
                item.offset
            );
        }
        if let Some(offset) = layout.union_offset {
            let _ = writeln!(
                output,
                "{assertion}(offsetof({ty}, value) == {offset}u, \"C view union offset\");"
            );
            for variant in &layout.variants {
                let field = roles::escaped_member(
                    prefix,
                    projection.models()[&variant.model].target_name(),
                )?;
                let _ = writeln!(
                    output,
                    "{assertion}(offsetof({ty}, value.{field}) == {offset}u, \"C view variant offset\");"
                );
            }
        }
    }
    output.push_str("#endif\n");
    emitted.insert(key.clone());
    Ok(())
}
pub(super) fn render_types(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    let layouts = CViewLayouts::for_projection(projection)?;
    output.push_str("/* Every view and nested role span borrows its result owner. Freeing that\n * owner invalidates them all. Player views are deliberately nonrecursive. */\n");
    let mut emitted = BTreeSet::new();
    for (key, _) in layouts.iter() {
        render_one(output, projection, prefix, &layouts, key, &mut emitted)?;
    }
    Ok(())
}
pub(super) fn render_functions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    for model in models(projection) {
        let function = name(model, "result_at")?;
        let result = name(model, "result_t")?;
        let view = name(model, "plan_view_t")?;
        let token = model_token_symbol(projection, prefix, model.id())?;
        let _ = writeln!(
            output,
            "static inline type_bridge_status_t {function}(const {result} *result, size_t index, {view} *out) {{ return type_bridge_plan_result_view((const type_bridge_plan_result_t *)result, index, &{token}, out, sizeof(*out)); }}"
        );
        let tuple_model = name(model, "rows_model_at")?;
        let _ = writeln!(
            output,
            "/* The view pointer and nested spans borrow the rows owner. */\nstatic inline type_bridge_status_t {tuple_model}(const type_bridge_rows_result_t *result, size_t row, size_t column, const {view} **out, type_bridge_execution_diagnostics_t **diagnostics) {{ return type_bridge_rows_result_model_view(result, row, column, &{token}, (const void **)out, diagnostics); }}"
        );
    }
    Ok(())
}
