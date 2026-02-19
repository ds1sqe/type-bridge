//! Implementation of `#[derive(TypeBridgeEntity)]`.

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
                    "TypeBridgeEntity requires a struct with named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "TypeBridgeEntity can only be derived for structs",
            ))
        }
    };

    // Parse #[entity(name = "...")]
    let type_name = parse_entity_name(&input.attrs)?;

    // Separate iid field from attribute fields
    let mut has_iid = false;
    let mut attr_fields: Vec<EntityField> = Vec::new();

    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "Expected named field"))?;

        if ident == "iid" {
            has_iid = true;
            continue;
        }

        let field_attrs = parse_field_attrs(&field.attrs)?;
        let (is_optional, inner_ty) = unwrap_option_type(&field.ty);

        attr_fields.push(EntityField {
            ident: ident.clone(),
            inner_ty: inner_ty.clone(),
            is_optional,
            is_key: field_attrs.is_key,
            _custom_name: field_attrs.custom_name,
        });
    }

    if !has_iid {
        return Err(syn::Error::new_spanned(
            &input,
            "TypeBridgeEntity requires a field `iid: Option<String>`",
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

    // Generate from_document()
    let from_doc_fields = attr_fields.iter().map(|f| {
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

    let field_idents: Vec<_> = attr_fields.iter().map(|f| &f.ident).collect();

    Ok(quote! {
        impl type_bridge_orm::TypeBridgeEntity for #name {
            const TYPE_NAME: &'static str = #type_name;

            fn owned_attributes() -> &'static [type_bridge_orm::OwnedAttributeInfo] {
                static ATTRS: [type_bridge_orm::OwnedAttributeInfo; #n_attrs] = [
                    #(#owned_attrs),*
                ];
                &ATTRS
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

            fn from_document(
                doc: &serde_json::Map<String, serde_json::Value>,
            ) -> type_bridge_orm::Result<Self> {
                #(#from_doc_fields)*
                Ok(Self {
                    iid: None,
                    #(#field_idents),*
                })
            }
        }
    })
}

struct EntityField {
    ident: syn::Ident,
    inner_ty: syn::Type,
    is_optional: bool,
    is_key: bool,
    _custom_name: Option<String>,
}

struct FieldAttrs {
    is_key: bool,
    custom_name: Option<String>,
}

/// Parse `#[entity(name = "...")]` from struct attributes.
fn parse_entity_name(attrs: &[syn::Attribute]) -> syn::Result<String> {
    for attr in attrs {
        if !attr.path().is_ident("entity") {
            continue;
        }
        let mut entity_name: Option<String> = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                entity_name = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `name`"))
            }
        })?;
        if let Some(name) = entity_name {
            return Ok(name);
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "Missing `#[entity(name = \"...\")]`",
    ))
}

/// Parse field-level attributes: `#[field(key)]`, `#[field(name = "...")]`
fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
    let mut is_key = false;
    let mut custom_name: Option<String> = None;

    for attr in attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("key") {
                is_key = true;
                Ok(())
            } else if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                custom_name = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `key` or `name`"))
            }
        })?;
    }

    Ok(FieldAttrs {
        is_key,
        custom_name,
    })
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
