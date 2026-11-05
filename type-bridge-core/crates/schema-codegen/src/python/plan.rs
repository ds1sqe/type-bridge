//! Generated model-specific signatures over the shared plan recorder.
use super::*;

fn fields(
    projection: &RuntimeProjection,
    model: &ModelProjection,
) -> Result<Vec<(String, String)>, Diagnostic> {
    model
        .query_tokens()
        .fields()
        .iter()
        .map(|(id, token)| {
            let attribute = TypeId::new(TypeKind::Attribute, id.attribute().label().as_str())?;
            let tag = projection.models()[&attribute]
                .declaration()
                .value_type()
                .ok_or_else(|| facet_error("plan field has no scalar domain"))?;
            Ok((
                token.target_name().as_str().to_owned(),
                scalar_type(tag).to_owned(),
            ))
        })
        .collect()
}

fn role_type(
    projection: &RuntimeProjection,
    role: &type_bridge_contract::projection::RoleTokenProjection,
) -> String {
    let players = role
        .accepted_players()
        .iter()
        .map(|id| format!("{}Handle", projection.models()[id].target_name().as_str()))
        .collect::<Vec<_>>()
        .join(" | ");
    if players.is_empty() {
        "Never".to_owned()
    } else {
        players
    }
}

pub(super) fn model(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    if !matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation) {
        return Ok(());
    }
    let name = model.target_name().as_str();
    // Schema subclasses may add required factory arguments.
    let override_note = if model.declaration().parent().is_some() {
        "  # pyright: ignore[reportIncompatibleMethodOverride]"
    } else {
        ""
    };
    let fields = fields(projection, model)?;
    let key = model
        .query_tokens()
        .fields()
        .values()
        .filter(|field| field.is_key())
        .collect::<Vec<_>>();
    let key = if key.len() == 1 {
        Some(key[0].target_name().as_str())
    } else {
        None
    };
    let key_type = key
        .and_then(|key| fields.iter().find(|(name, _)| name == key))
        .map(|(_, ty)| ty.as_str());
    let named = fields
        .iter()
        .map(|(field, ty)| format!("{field}: {ty} | _PlanValue[{ty}] | _PlanInput[{ty}] = ..."))
        .collect::<Vec<_>>()
        .join(", ");
    for (method, subtypes) in [("match", false), ("subtypes", true)] {
        if stub {
            if let Some(ty) = key_type {
                let _ = writeln!(
                    output,
                    "    @classmethod\n    @overload\n    def {method}(cls, key: {ty} | _PlanInput[{ty}], /) -> {name}Handle: ...{override_note}\n    @classmethod\n    @overload"
                );
            } else {
                output.push_str("    @classmethod\n");
            }
            let signature = if named.is_empty() {
                "cls".to_owned()
            } else {
                format!("cls, *, {named}")
            };
            let _ = writeln!(
                output,
                "    def {method}({signature}) -> {name}Handle: ...{override_note}"
            );
        } else {
            let _ = writeln!(
                output,
                "    @classmethod\n    def {method}(cls, key: object = _PLAN_MISSING, /, **fields: object) -> {name}Handle:{override_note}\n        return _plan_match(cls, {name}Handle, _plan_authority(), {}, key, fields, subtypes={})",
                key.map_or_else(|| "None".to_owned(), |key| format!("{key:?}")),
                if subtypes { "True" } else { "False" }
            );
        }
    }
    output.push_str("    @classmethod\n");
    if stub {
        let _ = writeln!(
            output,
            "    def by_iid(cls, iid: _Iid, /) -> {name}Handle: ...{override_note}"
        );
    } else {
        let _ = writeln!(
            output,
            "    def by_iid(cls, iid: _Iid, /) -> {name}Handle:{override_note}\n        return _plan_match(cls, {name}Handle, _plan_authority(), None, _PLAN_MISSING, {{}}, iid=iid)"
        );
    }
    if !crate::supports_scalar_plan_writes(model) {
        if model.declaration().parent().is_some() {
            for method in ["insert", "put", "insert_many", "put_many"] {
                let _ = writeln!(output, "    {method}: None = None{override_note}");
            }
        }
        return Ok(());
    }
    let values_note = if model.declaration().parent().is_some() {
        "  # pyright: ignore[reportIncompatibleVariableOverride]"
    } else {
        ""
    };
    let _ = writeln!(
        output,
        "    class Values(TypedDict, total=False):{values_note}"
    );
    let mut insert = Vec::new();
    let mut field_names = Vec::new();
    for field in model.create().fields() {
        let field_name = model.query_tokens().fields()[field.token()]
            .target_name()
            .as_str();
        let ty = &fields
            .iter()
            .find(|(name, _)| name == field_name)
            .expect("create field has a query token")
            .1;
        let ty = if field.multiplicity().container() == ProjectedContainer::Sequence {
            format!("Sequence[{ty}]")
        } else {
            ty.to_owned()
        };
        let value_type = if field.multiplicity().required() {
            format!("Required[{ty}]")
        } else {
            ty.to_owned()
        };
        let _ = writeln!(output, "        {field_name}: {value_type}");
        insert.push(format!(
            "{field_name}: {ty}{}",
            if field.multiplicity().required() {
                ""
            } else if stub {
                " = ..."
            } else {
                " = _PLAN_MISSING"
            }
        ));
        field_names.push(field_name);
    }
    if field_names.is_empty() {
        output.push_str("        pass\n");
    }
    let mut role_names = Vec::new();
    for (id, role) in model.create().roles() {
        let token = &model.query_tokens().roles()[id];
        let role_name = token.target_name().as_str();
        let ty = role_type(projection, token);
        let ty = if role.multiplicity().container() == ProjectedContainer::Sequence {
            format!("Sequence[{ty}]")
        } else {
            ty
        };
        insert.push(format!(
            "{role_name}: {ty}{}",
            if role.multiplicity().required() {
                ""
            } else if stub {
                " = ..."
            } else {
                " = _PLAN_MISSING"
            }
        ));
        role_names.push(role_name);
    }
    let signature = if insert.is_empty() {
        "cls".to_owned()
    } else {
        format!("cls, *, {}", insert.join(", "))
    };
    let field_values = field_names
        .iter()
        .map(|name| format!("{name:?}: {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    let role_values = role_names
        .iter()
        .map(|name| format!("{name:?}: {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    for (method, put) in [("insert", false), ("put", true)] {
        let _ = writeln!(
            output,
            "    @classmethod\n    def {method}({signature}) -> _Plan[list[{name}]]:{override_note}"
        );
        if stub {
            output.push_str("        ...\n");
        } else {
            let _ = writeln!(
                output,
                "        return _plan_insert(cls, _plan_authority(), {{{field_values}}}, {{{role_values}}}, put={})",
                if put { "True" } else { "False" }
            );
        }
    }
    if model.id().kind() == TypeKind::Entity {
        for (method, put) in [("insert_many", false), ("put_many", true)] {
            let _ = writeln!(
                output,
                "    @classmethod\n    def {method}(cls, rows: Iterable[{name}.Values], /) -> _Plan[list[{name}]]:{override_note}"
            );
            if stub {
                output.push_str("        ...\n");
            } else {
                let _ = writeln!(
                    output,
                    "        return _plan_bulk_insert(cls, _plan_authority(), rows, put={})",
                    if put { "True" } else { "False" }
                );
            }
        }
    }
    Ok(())
}

pub(super) fn handle(
    output: &mut String,
    projection: &RuntimeProjection,
    model: &ModelProjection,
    stub: bool,
) -> Result<(), Diagnostic> {
    if !matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation) {
        return Ok(());
    }
    let name = model.target_name().as_str();
    let _ = writeln!(output, "class {name}Handle(_PlanHandle[{name}]):");
    let _ = writeln!(
        output,
        "    \"\"\"A bound {name} inside an authoring context.\"\"\""
    );
    let fields = fields(projection, model)?;
    let scalar_fields = model
        .query_tokens()
        .fields()
        .values()
        .filter(|field| {
            field.multiplicity().container() == ProjectedContainer::Scalar && !field.is_key()
        })
        .map(|field| field.target_name().as_str())
        .collect::<BTreeSet<_>>();
    let mut parameters = Vec::new();
    for (field, ty) in &fields {
        let _ = writeln!(
            output,
            "    @property\n    def {field}(self) -> _PlanValue[{ty}]:"
        );
        if stub {
            output.push_str("        ...\n");
        } else {
            let _ = writeln!(
                output,
                "        return cast(_PlanValue[{ty}], self._value(getattr({name}, {field:?})))"
            );
        }
        if scalar_fields.contains(field.as_str()) {
            parameters.push(format!(
                "{field}: {ty} | _PlanValue[{ty}] | _PlanExpr[{ty}] | _PlanInput[{ty}] = {}",
                if stub { "..." } else { "_PLAN_MISSING" }
            ));
        }
    }
    let signature = if parameters.is_empty() {
        "self".to_owned()
    } else {
        format!("self, *, {}", parameters.join(", "))
    };
    if !parameters.is_empty() {
        let _ = writeln!(output, "    def set({signature}) -> _Plan[list[{name}]]:");
        if stub {
            output.push_str("        ...\n");
        } else {
            let values = fields
                .iter()
                .filter(|(field, _)| scalar_fields.contains(field.as_str()))
                .map(|(field, _)| format!("{field:?}: {field}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(output, "        return self._set({{{values}}})");
        }
    }
    let roles = model
        .query_tokens()
        .roles()
        .values()
        .filter(|role| role.multiplicity().collection_mode().is_unordered())
        .collect::<Vec<_>>();
    if !roles.is_empty() {
        let parameters = roles
            .iter()
            .map(|role| {
                format!(
                    "{}: {} = {}",
                    role.target_name().as_str(),
                    if role.multiplicity().container() == ProjectedContainer::Sequence {
                        let ty = role_type(projection, role);
                        format!("{ty} | Sequence[{ty}]")
                    } else {
                        role_type(projection, role)
                    },
                    if stub { "..." } else { "_PLAN_MISSING" }
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let values = roles
            .iter()
            .map(|role| {
                let name = role.target_name().as_str();
                format!("{name:?}: {name}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        for (method, remove) in [("link", false), ("unlink", true)] {
            let _ = writeln!(
                output,
                "    def {method}(self, *, {parameters}) -> _Plan[list[{name}]]:"
            );
            if stub {
                output.push_str("        ...\n");
            } else {
                let _ = writeln!(
                    output,
                    "        return self._link({{{values}}}, remove={})",
                    if remove { "True" } else { "False" }
                );
            }
        }
    }
    output.push('\n');
    Ok(())
}

pub(super) fn roles(output: &mut String, model: &ModelProjection) {
    if !matches!(model.id().kind(), TypeKind::Entity | TypeKind::Relation) {
        return;
    }
    let roles = model
        .complete_read()
        .roles()
        .keys()
        .map(|id| {
            format!(
                "(getattr({}, {:?}), {})",
                model.target_name().as_str(),
                model.query_tokens().roles()[id].target_name().as_str(),
                "False"
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        output,
        "{}.__plan_roles__ = [{roles}]",
        model.target_name().as_str()
    );
}
