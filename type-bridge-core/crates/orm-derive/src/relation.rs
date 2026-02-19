//! Implementation of `#[derive(TypeBridgeRelation)]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Fields, LitStr};

pub fn derive(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;

    let fields = match &input.data {
        syn::Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "TypeBridgeRelation requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "TypeBridgeRelation can only be derived for structs",
            ))
        }
    };

    // Parse #[relation(name = "...")]
    let type_name = parse_relation_name(&input.attrs)?;

    // Separate fields into iid, roles, and attributes
    let mut has_iid = false;
    let mut role_fields: Vec<RoleField> = Vec::new();
    let mut attr_fields: Vec<AttrField> = Vec::new();

    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "Expected named field"))?;

        if ident == "iid" {
            has_iid = true;
            continue;
        }

        // Check for #[role(...)] annotation
        if let Some(role_attrs) = parse_role_attrs(&field.attrs)? {
            role_fields.push(RoleField {
                ident: ident.clone(),
                role_name: role_attrs.role_name,
                player_type: role_attrs.player_type,
            });
            continue;
        }

        // Otherwise treat as an attribute field
        let field_attrs = parse_field_attrs(&field.attrs)?;
        let (is_optional, inner_ty) = unwrap_option_type(&field.ty);

        attr_fields.push(AttrField {
            ident: ident.clone(),
            inner_ty: inner_ty.clone(),
            is_optional,
            is_key: field_attrs.is_key,
        });
    }

    if !has_iid {
        return Err(syn::Error::new_spanned(
            &input,
            "TypeBridgeRelation requires a field `iid: Option<String>`",
        ));
    }

    if role_fields.is_empty() {
        return Err(syn::Error::new_spanned(
            &input,
            "TypeBridgeRelation requires at least one #[role(...)] field",
        ));
    }

    // Generate owned_attributes()
    let owned_attrs = attr_fields.iter().map(|f| {
        let ty = &f.inner_ty;
        let is_key = f.is_key;
        quote! {
            type_bridge_orm::OwnedAttributeInfo {
                attr_name: <#ty as type_bridge_orm::TypeBridgeAttribute>::ATTR_NAME,
                value_type: <#ty as type_bridge_orm::TypeBridgeAttribute>::VALUE_TYPE,
                is_key: #is_key,
            }
        }
    });
    let n_attrs = attr_fields.len();

    // Generate role_info()
    let role_infos = role_fields.iter().map(|r| {
        let role_name = &r.role_name;
        let player_type = &r.player_type;
        quote! {
            type_bridge_orm::RoleInfo {
                role_name: #role_name,
                player_type_name: #player_type,
            }
        }
    });
    let n_roles = role_fields.len();

    // Generate to_attribute_values()
    let to_values = attr_fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.inner_ty;
        if f.is_optional {
            quote! {
                if let Some(ref val) = self.#ident {
                    values.push((
                        <#ty as type_bridge_orm::TypeBridgeAttribute>::ATTR_NAME,
                        <#ty as type_bridge_orm::TypeBridgeAttribute>::to_value(val),
                    ));
                }
            }
        } else {
            quote! {
                values.push((
                    <#ty as type_bridge_orm::TypeBridgeAttribute>::ATTR_NAME,
                    <#ty as type_bridge_orm::TypeBridgeAttribute>::to_value(&self.#ident),
                ));
            }
        }
    });

    // Generate to_role_player_refs()
    let role_refs = role_fields.iter().map(|r| {
        let ident = &r.ident;
        quote! {
            self.#ident.clone()
        }
    });

    // Generate from_document()
    let from_doc_attrs = attr_fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.inner_ty;
        let type_name_str = &type_name;

        if f.is_optional {
            quote! {
                let #ident = {
                    let attr_name = <#ty as type_bridge_orm::TypeBridgeAttribute>::ATTR_NAME;
                    let value_type = <#ty as type_bridge_orm::TypeBridgeAttribute>::VALUE_TYPE;
                    match doc.get(attr_name) {
                        Some(json_val) => {
                            let attr_val = type_bridge_orm::AttributeValue::from_json(json_val, value_type)
                                .ok_or_else(|| type_bridge_orm::OrmError::Hydration {
                                    type_name: #type_name_str.to_string(),
                                    message: format!("cannot parse attribute '{}' as {}", attr_name, value_type),
                                })?;
                            Some(<#ty as type_bridge_orm::TypeBridgeAttribute>::from_value(&attr_val)
                                .ok_or_else(|| type_bridge_orm::OrmError::Hydration {
                                    type_name: #type_name_str.to_string(),
                                    message: format!("type mismatch for attribute '{}'", attr_name),
                                })?)
                        }
                        None => None,
                    }
                };
            }
        } else {
            quote! {
                let #ident = {
                    let attr_name = <#ty as type_bridge_orm::TypeBridgeAttribute>::ATTR_NAME;
                    let value_type = <#ty as type_bridge_orm::TypeBridgeAttribute>::VALUE_TYPE;
                    let json_val = doc.get(attr_name).ok_or_else(|| {
                        type_bridge_orm::OrmError::Hydration {
                            type_name: #type_name_str.to_string(),
                            message: format!("missing required attribute '{}'", attr_name),
                        }
                    })?;
                    let attr_val = type_bridge_orm::AttributeValue::from_json(json_val, value_type)
                        .ok_or_else(|| type_bridge_orm::OrmError::Hydration {
                            type_name: #type_name_str.to_string(),
                            message: format!("cannot parse attribute '{}' as {}", attr_name, value_type),
                        })?;
                    <#ty as type_bridge_orm::TypeBridgeAttribute>::from_value(&attr_val)
                        .ok_or_else(|| type_bridge_orm::OrmError::Hydration {
                            type_name: #type_name_str.to_string(),
                            message: format!("type mismatch for attribute '{}'", attr_name),
                        })?
                };
            }
        }
    });

    // Generate default role player refs for from_document
    let default_role_fields = role_fields.iter().map(|r| {
        let ident = &r.ident;
        let role_name = &r.role_name;
        let player_type = &r.player_type;
        quote! {
            #ident: type_bridge_orm::RolePlayerRef {
                role: #role_name,
                entity_type_name: #player_type,
                iid: None,
                key: None,
            }
        }
    });

    let attr_idents: Vec<_> = attr_fields.iter().map(|f| &f.ident).collect();

    Ok(quote! {
        impl type_bridge_orm::TypeBridgeRelation for #name {
            const TYPE_NAME: &'static str = #type_name;

            fn owned_attributes() -> &'static [type_bridge_orm::OwnedAttributeInfo] {
                static ATTRS: [type_bridge_orm::OwnedAttributeInfo; #n_attrs] = [
                    #(#owned_attrs),*
                ];
                &ATTRS
            }

            fn role_info() -> &'static [type_bridge_orm::RoleInfo] {
                static ROLES: [type_bridge_orm::RoleInfo; #n_roles] = [
                    #(#role_infos),*
                ];
                &ROLES
            }

            fn iid(&self) -> Option<&str> {
                self.iid.as_deref()
            }

            fn set_iid(&mut self, iid: String) {
                self.iid = Some(iid);
            }

            fn to_attribute_values(&self) -> Vec<(&'static str, type_bridge_orm::AttributeValue)> {
                let mut values = Vec::new();
                #(#to_values)*
                values
            }

            fn to_role_player_refs(&self) -> Vec<type_bridge_orm::RolePlayerRef> {
                vec![#(#role_refs),*]
            }

            fn from_document(
                doc: &serde_json::Map<String, serde_json::Value>,
            ) -> type_bridge_orm::Result<Self> {
                #(#from_doc_attrs)*
                Ok(Self {
                    iid: None,
                    #(#default_role_fields,)*
                    #(#attr_idents),*
                })
            }
        }
    })
}

struct RoleField {
    ident: syn::Ident,
    role_name: String,
    player_type: String,
}

struct AttrField {
    ident: syn::Ident,
    inner_ty: syn::Type,
    is_optional: bool,
    is_key: bool,
}

struct RoleAttrs {
    role_name: String,
    player_type: String,
}

struct FieldAttrs {
    is_key: bool,
}

/// Parse `#[relation(name = "...")]` from struct attributes.
fn parse_relation_name(attrs: &[syn::Attribute]) -> syn::Result<String> {
    for attr in attrs {
        if !attr.path().is_ident("relation") {
            continue;
        }
        let mut relation_name: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                relation_name = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `name`"))
            }
        })?;
        if let Some(name) = relation_name {
            return Ok(name);
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "Missing `#[relation(name = \"...\")]`",
    ))
}

/// Parse `#[role(name = "...", player_type = "...")]` from field attributes.
/// Returns `None` if no `#[role(...)]` attribute is found.
fn parse_role_attrs(attrs: &[syn::Attribute]) -> syn::Result<Option<RoleAttrs>> {
    for attr in attrs {
        if !attr.path().is_ident("role") {
            continue;
        }
        let mut role_name: Option<String> = None;
        let mut player_type: Option<String> = None;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                role_name = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("player_type") {
                let value: LitStr = meta.value()?.parse()?;
                player_type = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `name` or `player_type`"))
            }
        })?;

        let role_name = role_name.ok_or_else(|| {
            syn::Error::new_spanned(attr, "Missing `name` in #[role(...)]")
        })?;
        let player_type = player_type.ok_or_else(|| {
            syn::Error::new_spanned(attr, "Missing `player_type` in #[role(...)]")
        })?;

        return Ok(Some(RoleAttrs {
            role_name,
            player_type,
        }));
    }
    Ok(None)
}

/// Parse field-level attributes: `#[field(key)]`
fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
    let mut is_key = false;

    for attr in attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("key") {
                is_key = true;
                Ok(())
            } else {
                Err(meta.error("expected `key`"))
            }
        })?;
    }

    Ok(FieldAttrs { is_key })
}

/// Check if a type is `Option<T>` and return the inner type.
fn unwrap_option_type(ty: &syn::Type) -> (bool, &syn::Type) {
    if let syn::Type::Path(type_path) = ty
        && let Some(last) = type_path.path.segments.last()
        && last.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &last.arguments
        && let Some(syn::GenericArgument::Type(inner)) = args.args.first()
    {
        return (true, inner);
    }
    (false, ty)
}
