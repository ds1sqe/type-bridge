//! Implementation of `include_schema!` proc-macro.
//!
//! Reads a TypeQL `.tql` file at compile time, parses it with
//! `type_bridge_core_lib`, and generates Rust model code inline.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::path::PathBuf;

use proc_macro2::TokenStream;
use syn::LitStr;

use type_bridge_core_lib::schema::{
    Cardinality, EntityType, OwnedAttribute, RelationType, TypeSchema,
};

pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let lit: LitStr = syn::parse2(input)?;
    let path = lit.value();

    // Resolve relative to CARGO_MANIFEST_DIR
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| syn::Error::new(lit.span(), "CARGO_MANIFEST_DIR not set"))?;
    let full_path = PathBuf::from(&manifest_dir).join(&path);
    let content = std::fs::read_to_string(&full_path).map_err(|e| {
        syn::Error::new(
            lit.span(),
            format!("cannot read {}: {}", full_path.display(), e),
        )
    })?;

    let schema = TypeSchema::from_typeql(&content)
        .map_err(|e| syn::Error::new(lit.span(), format!("schema parse error: {e}")))?;

    let code = generate_inline(&schema);
    let tokens: TokenStream = code.parse().map_err(|e: proc_macro2::LexError| {
        syn::Error::new(lit.span(), format!("generated code parse error: {e}"))
    })?;

    Ok(tokens)
}

/// Generate all model code as a single inline block (no separate modules).
fn generate_inline(schema: &TypeSchema) -> String {
    let mut out = String::new();

    // Attributes
    for attr in schema.attributes.values() {
        if attr.is_abstract {
            continue;
        }
        let struct_name = to_pascal_case(&attr.name);
        writeln!(
            out,
            "type_bridge_orm::define_attribute!({}, \"{}\", \"{}\");",
            struct_name, attr.name, attr.value_type
        )
        .unwrap();
    }
    writeln!(out).unwrap();

    // Entities (topologically sorted)
    let entity_order = topological_sort(&schema.entities, |e| e.parent.as_deref());
    for name in &entity_order {
        let entity = &schema.entities[name.as_str()];
        generate_entity(&mut out, entity, schema);
        writeln!(out).unwrap();
    }

    // Relations (topologically sorted)
    let relation_order = topological_sort(&schema.relations, |r| r.parent.as_deref());
    for name in &relation_order {
        let relation = &schema.relations[name.as_str()];
        generate_relation(&mut out, relation, schema);
        writeln!(out).unwrap();
    }

    out
}

fn generate_entity(out: &mut String, entity: &EntityType, _schema: &TypeSchema) {
    let mut entity_attr_parts = vec![format!("name = \"{}\"", entity.name)];
    if entity.is_abstract {
        entity_attr_parts.push("r#abstract".to_string());
    }
    if let Some(ref parent) = entity.parent {
        entity_attr_parts.push(format!("extends = \"{}\"", parent));
    }

    let struct_name = to_pascal_case(&entity.name);

    writeln!(out, "#[derive(type_bridge_orm::DeriveEntity, Debug)]").unwrap();
    writeln!(out, "#[entity({})]", entity_attr_parts.join(", ")).unwrap();
    writeln!(out, "pub struct {} {{", struct_name).unwrap();
    writeln!(out, "    pub iid: Option<String>,").unwrap();

    let attr_names = if entity.owns_order.is_empty() {
        entity
            .owns
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>()
    } else {
        entity.owns_order.iter().map(|s| s.as_str()).collect()
    };

    let owns_map: BTreeMap<&str, &OwnedAttribute> =
        entity.owns.iter().map(|o| (o.name.as_str(), o)).collect();

    for attr_name in &attr_names {
        if let Some(own) = owns_map.get(attr_name) {
            let field_name = to_snake_case(attr_name);
            let attr_type = to_pascal_case(attr_name);

            let mut field_attrs = Vec::new();
            if own.is_key {
                field_attrs.push("key".to_string());
            }
            if own.is_unique {
                field_attrs.push("unique".to_string());
            }
            if let Some(ref card) = own.cardinality {
                field_attrs.push(format!("card_min = {}", card.min));
                if let Some(max) = card.max {
                    field_attrs.push(format!("card_max = {}", max));
                }
            }

            let field_attr_line = if field_attrs.is_empty() {
                String::new()
            } else {
                format!("    #[field({})]\n", field_attrs.join(", "))
            };

            let field_type = determine_field_type(&attr_type, own);

            write!(out, "{}", field_attr_line).unwrap();
            writeln!(out, "    pub {}: {},", field_name, field_type).unwrap();
        }
    }

    writeln!(out, "}}").unwrap();
}

fn generate_relation(out: &mut String, relation: &RelationType, schema: &TypeSchema) {
    let mut rel_attr_parts = vec![format!("name = \"{}\"", relation.name)];
    if relation.is_abstract {
        rel_attr_parts.push("r#abstract".to_string());
    }
    if let Some(ref parent) = relation.parent {
        rel_attr_parts.push(format!("extends = \"{}\"", parent));
    }

    let struct_name = to_pascal_case(&relation.name);

    writeln!(out, "#[derive(type_bridge_orm::DeriveRelation, Debug)]").unwrap();
    writeln!(out, "#[relation({})]", rel_attr_parts.join(", ")).unwrap();
    writeln!(out, "pub struct {} {{", struct_name).unwrap();
    writeln!(out, "    pub iid: Option<String>,").unwrap();

    let mut seen_roles: HashSet<String> = HashSet::new();
    for role in &relation.roles {
        let player_type = resolve_role_player(schema, &relation.name, &role.name);
        let field_name = make_unique_role_field(&role.name, &mut seen_roles);
        writeln!(
            out,
            "    #[role(name = \"{}\", player_type = \"{}\")]",
            role.name, player_type
        )
        .unwrap();
        writeln!(
            out,
            "    pub {}: type_bridge_orm::RolePlayerRef,",
            field_name
        )
        .unwrap();
    }

    let attr_names = if relation.owns_order.is_empty() {
        relation
            .owns
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>()
    } else {
        relation.owns_order.iter().map(|s| s.as_str()).collect()
    };

    let owns_map: BTreeMap<&str, &OwnedAttribute> =
        relation.owns.iter().map(|o| (o.name.as_str(), o)).collect();

    for attr_name in &attr_names {
        if let Some(own) = owns_map.get(attr_name) {
            let field_name = to_snake_case(attr_name);
            let attr_type = to_pascal_case(attr_name);

            let mut field_attrs = Vec::new();
            if own.is_key {
                field_attrs.push("key".to_string());
            }
            if own.is_unique {
                field_attrs.push("unique".to_string());
            }
            if let Some(ref card) = own.cardinality {
                field_attrs.push(format!("card_min = {}", card.min));
                if let Some(max) = card.max {
                    field_attrs.push(format!("card_max = {}", max));
                }
            }

            let field_attr_line = if field_attrs.is_empty() {
                String::new()
            } else {
                format!("    #[field({})]\n", field_attrs.join(", "))
            };

            let field_type = determine_field_type(&attr_type, own);

            write!(out, "{}", field_attr_line).unwrap();
            writeln!(out, "    pub {}: Option<{}>,", field_name, field_type).unwrap();
        }
    }

    writeln!(out, "}}").unwrap();
}

fn determine_field_type(attr_type: &str, own: &OwnedAttribute) -> String {
    match &own.cardinality {
        Some(Cardinality {
            min: 0,
            max: Some(1),
        }) => format!("Option<{}>", attr_type),
        Some(Cardinality { max: None, .. }) => format!("Vec<{}>", attr_type),
        Some(Cardinality { max: Some(max), .. }) if *max > 1 => format!("Vec<{}>", attr_type),
        _ => attr_type.to_string(),
    }
}

fn resolve_role_player(schema: &TypeSchema, relation_name: &str, role_name: &str) -> String {
    let role_ref = format!("{}:{}", relation_name, role_name);
    for entity in schema.entities.values() {
        for play in &entity.plays {
            if play.role_ref == role_ref {
                return entity.name.clone();
            }
        }
    }
    for rel in schema.relations.values() {
        for play in &rel.plays {
            if play.role_ref == role_ref {
                return rel.name.clone();
            }
        }
    }
    "unknown".to_string()
}

fn make_unique_role_field(role_name: &str, seen: &mut HashSet<String>) -> String {
    let base = to_snake_case(role_name);
    if seen.insert(base.clone()) {
        return base;
    }
    let mut i = 1;
    loop {
        let candidate = format!("{}_{}", base, i);
        if seen.insert(candidate.clone()) {
            return candidate;
        }
        i += 1;
    }
}

fn topological_sort<T, F>(map: &BTreeMap<String, T>, get_parent: F) -> Vec<String>
where
    F: Fn(&T) -> Option<&str>,
{
    let mut result = Vec::new();
    let mut visited = HashSet::new();

    fn visit<T2, F2>(
        name: &str,
        map: &BTreeMap<String, T2>,
        get_parent: &F2,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) where
        F2: Fn(&T2) -> Option<&str>,
    {
        if visited.contains(name) {
            return;
        }
        visited.insert(name.to_string());
        if let Some(entry) = map.get(name)
            && let Some(parent) = get_parent(entry)
        {
            visit(parent, map, get_parent, visited, result);
        }
        result.push(name.to_string());
    }

    for name in map.keys() {
        visit(name, map, &get_parent, &mut visited, &mut result);
    }
    result
}

fn to_pascal_case(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + chars.as_str()
                }
                None => String::new(),
            }
        })
        .collect()
}

fn to_snake_case(name: &str) -> String {
    name.replace('-', "_")
}
