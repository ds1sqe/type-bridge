//! Nominal role inputs for unordered player edits.
use super::*;

fn roles(model: &ModelProjection) -> impl Iterator<Item = (usize, &RoleTokenProjection)> {
    model
        .query_tokens()
        .roles()
        .values()
        .enumerate()
        .filter(|(_, role)| {
            role.multiplicity().collection_mode().is_unordered()
                && !role.accepted_players().is_empty()
        })
}
pub(super) fn escaped_member(
    prefix: &str,
    target: &TargetIdentifier,
) -> Result<String, Diagnostic> {
    let raw = projected_member_suffix(prefix, target)?;
    Ok(if AUTHORING_KEYWORDS.contains(&raw) {
        format!("{raw}zk")
    } else {
        raw.to_owned()
    })
}
fn base(
    model: &ModelProjection,
    role: &RoleTokenProjection,
    prefix: &str,
) -> Result<String, Diagnostic> {
    name(
        model,
        &format!(
            "plan_{}_player",
            escaped_member(prefix, role.target_name())?
        ),
    )
}
fn carrier(
    projection: &RuntimeProjection,
    model: &ModelProjection,
    role: &RoleTokenProjection,
    prefix: &str,
) -> Result<String, Diagnostic> {
    if role.accepted_players().len() == 1 {
        name(
            &projection.models()[role.accepted_players().first().expect("one player")],
            "handle_t",
        )
    } else {
        generated_symbol(&base(model, role, prefix)?, "t")
    }
}
pub(super) fn symbols(
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<Vec<String>, Diagnostic> {
    let mut symbols = Vec::new();
    for model in models(projection).filter(|model| roles(model).next().is_some()) {
        for suffix in ["link_t", "unlink_t", "link", "unlink"] {
            symbols.push(name(model, suffix)?);
        }
        for (_, role) in roles(model).filter(|(_, role)| role.accepted_players().len() > 1) {
            let base = base(model, role, prefix)?;
            for suffix in ["t", "kind_t", "none"] {
                symbols.push(generated_symbol(&base, suffix)?);
            }
            for player in role.accepted_players() {
                symbols.push(generated_symbol(
                    &base,
                    &escaped_member(prefix, projection.models()[player].target_name())?,
                )?);
            }
        }
    }
    Ok(symbols)
}
pub(super) fn render_types(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    for model in models(projection).filter(|model| roles(model).next().is_some()) {
        for (_, role) in roles(model).filter(|(_, role)| role.accepted_players().len() > 1) {
            let base = base(model, role, prefix)?;
            let kind = generated_symbol(&base, "kind_t")?;
            let none = generated_symbol(&base, "none")?;
            let ty = carrier(projection, model, role, prefix)?;
            let _ = writeln!(output, "typedef enum {kind} {{ {none} = 0,");
            for (index, player) in role.accepted_players().iter().enumerate() {
                let tag = generated_symbol(
                    &base,
                    &escaped_member(prefix, projection.models()[player].target_name())?,
                )?;
                let _ = writeln!(output, "{tag} = {},", index + 1);
            }
            let _ = writeln!(
                output,
                "}} {kind};\n/* Integer storage permits checked refusal of unknown tags in C++. */\ntypedef struct {ty} {{ uint32_t kind; union {{"
            );
            for player in role.accepted_players() {
                let player = &projection.models()[player];
                let _ = writeln!(
                    output,
                    "{} {};",
                    name(player, "handle_t")?,
                    escaped_member(prefix, player.target_name())?
                );
            }
            let _ = writeln!(output, "}} value; }} {ty};");
        }
        let link = name(model, "link_t")?;
        let unlink = name(model, "unlink_t")?;
        let _ = writeln!(
            output,
            "/* Zero means no change. Arrays are borrowed only for this call; the recording retains their player IDs. */\ntypedef struct {link} {{"
        );
        for (_, role) in roles(model) {
            let ty = carrier(projection, model, role, prefix)?;
            let member = escaped_member(prefix, role.target_name())?;
            if role.multiplicity().container() == ProjectedContainer::Scalar {
                let _ = writeln!(output, "{ty} {member};");
            } else {
                let _ = writeln!(output, "const {ty} *{member}; size_t {member}_count;");
            }
        }
        let _ = writeln!(output, "}} {link};\ntypedef {link} {unlink};");
    }
    Ok(())
}
pub(super) fn render_create_members(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
    members: &mut BTreeSet<String>,
) -> Result<(), Diagnostic> {
    for (_, role) in
        roles(model).filter(|(_, role)| model.create().roles().contains_key(role.role()))
    {
        let ty = carrier(projection, model, role, prefix)?;
        let member = escaped_member(prefix, role.target_name())?;
        if !members.insert(member.clone()) {
            return Err(invalid(
                "c_emitter_generated_symbol_collision",
                "generated C struct members collide",
            ));
        }
        if role.multiplicity().container() == ProjectedContainer::Scalar {
            let _ = writeln!(output, "{ty} {member};");
        } else {
            if !members.insert(format!("{member}_count")) {
                return Err(invalid(
                    "c_emitter_generated_symbol_collision",
                    "generated C struct members collide",
                ));
            }
            let _ = writeln!(output, "const {ty} *{member}; size_t {member}_count;");
        }
    }
    Ok(())
}
fn player_call(
    output: &mut String,
    player: &str,
    function: Option<&str>,
    role_index: usize,
    empty_is_absent: bool,
    prefix: &str,
) -> Result<(), Diagnostic> {
    if empty_is_absent {
        let _ = writeln!(
            output,
            "if ({player}.recording != NULL || {player}.binding != 0u || {player}.model_ordinal != 0u)"
        );
    }
    if let Some(function) = function {
        let _ = writeln!(output, "{{
            status = type_bridge_plan_builder_check_handle(p, {player}.recording, {player}.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;
            status = {function}(p, h.binding, type_bridge_plan_builder_model_role(p, h.model_ordinal, {role_index}u), {player}.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status; }}");
    } else {
        // Reuse the create helper's alias inputs: a per-player array would grow the
        // frame with every accepted player under MSVC stack analysis.
        let [add_input, preflight] = generated_alias_helper_symbols(prefix)?;
        let _ = writeln!(output, "{{ count = 0u;
            {add_input}(inputs, &count, TYPE_BRIDGE_GENERATED_INPUT_PLAN_BUILDER, {player}.recording, 1u);
            status = {preflight}(inputs, count, out, sizeof(*out), marker_output, marker_output_size); if (status != TYPE_BRIDGE_STATUS_OK) return status; }}");
    }
    Ok(())
}
pub(super) fn render_functions(
    output: &mut String,
    projection: &RuntimeProjection,
    prefix: &str,
) -> Result<(), Diagnostic> {
    for model in models(projection).filter(|model| roles(model).next().is_some()) {
        for (suffix, native) in [
            ("link", "type_bridge_plan_builder_link"),
            ("unlink", "type_bridge_plan_builder_delete_player"),
        ] {
            let function = name(model, suffix)?;
            let values = name(model, &format!("{suffix}_t"))?;
            let handle = name(model, "handle_t")?;
            let _ = writeln!(output, "static inline type_bridge_status_t {function}(type_bridge_plan_builder_t *p, {handle} h, const {values} *values) {{
                type_bridge_status_t status = type_bridge_plan_builder_require_open(p); if (status != TYPE_BRIDGE_STATUS_OK) return status; status = type_bridge_plan_builder_check_handle(p, h.recording, h.binding); if (status != TYPE_BRIDGE_STATUS_OK) return status;
                if (values == NULL) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;");
            render_assignments(output, projection, model, prefix, Some(native), false)?;
            output.push_str("return status;\n}\n");
        }
    }
    Ok(())
}

pub(super) fn render_assignments(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    prefix: &str,
    native: Option<&str>,
    create_only: bool,
) -> Result<(), Diagnostic> {
    for (_, role) in roles(model)
        .filter(|(_, role)| !create_only || model.create().roles().contains_key(role.role()))
        .filter(|(_, role)| role.multiplicity().container() == ProjectedContainer::Sequence)
    {
        let member = escaped_member(prefix, role.target_name())?;
        let _ = writeln!(
            output,
            "if (values->{member}_count > SIZE_MAX / sizeof(*values->{member}) || (values->{member}_count != 0u && values->{member} == NULL)) return TYPE_BRIDGE_STATUS_INVALID_ARGUMENT;"
        );
    }
    for (index, role) in roles(model)
        .filter(|(_, role)| !create_only || model.create().roles().contains_key(role.role()))
    {
        let member = escaped_member(prefix, role.target_name())?;
        let sequence = role.multiplicity().container() == ProjectedContainer::Sequence;
        let value = if sequence {
            let _ = writeln!(
                output,
                "{{ size_t index; for (index = 0u; index < values->{member}_count; ++index) {{"
            );
            format!("values->{member}[index]")
        } else {
            format!("values->{member}")
        };
        if role.accepted_players().len() == 1 {
            player_call(output, &value, native, index, true, prefix)?;
        } else {
            let base = base(model, role, prefix)?;
            let none = generated_symbol(&base, "none")?;
            let _ = writeln!(output, "switch ({value}.kind) {{ case {none}: break;");
            for player in role.accepted_players() {
                let member = escaped_member(prefix, projection.models()[player].target_name())?;
                let tag = generated_symbol(&base, &member)?;
                let _ = writeln!(output, "case {tag}: {{");
                player_call(
                    output,
                    &format!("{value}.value.{member}"),
                    native,
                    index,
                    false,
                    prefix,
                )?;
                output.push_str("break; }\n");
            }
            if native.is_some() {
                output.push_str(
                    "default: return type_bridge_plan_builder_check_handle(p, p, UINT32_MAX); }\n",
                );
            } else {
                output.push_str("default: break; }\n");
            }
        }
        if sequence {
            output.push_str("} }\n");
        }
    }
    Ok(())
}
