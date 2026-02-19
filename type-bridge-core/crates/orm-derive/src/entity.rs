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

    // Parse #[entity(name = "...", abstract, extends = "...")]
    let entity_attrs = parse_entity_attrs(&input.attrs)?;
    let type_name = entity_attrs.name;
    let is_abstract = entity_attrs.is_abstract;
    let parent_type = entity_attrs.parent_type;

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
            is_unique: field_attrs.is_unique,
            card_min: field_attrs.card_min,
            card_max: field_attrs.card_max,
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
        let annots_tokens = build_annotations(f.is_key, f.is_unique, f.card_min, f.card_max);
        quote! {
            type_bridge_orm::OwnedAttributeInfo {
                attr_name: <#ty as type_bridge_orm::TypeBridgeAttribute>::ATTR_NAME,
                value_type: <#ty as type_bridge_orm::TypeBridgeAttribute>::VALUE_TYPE_ENUM,
                annotations: #annots_tokens,
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

    let is_abstract_tokens = if is_abstract {
        quote! { const IS_ABSTRACT: bool = true; }
    } else {
        quote! {}
    };

    let parent_type_tokens = match &parent_type {
        Some(p) => quote! { const PARENT_TYPE: Option<&'static str> = Some(#p); },
        None => quote! {},
    };

    Ok(quote! {
        impl type_bridge_orm::TypeBridgeEntity for #name {
            const TYPE_NAME: &'static str = #type_name;
            #is_abstract_tokens
            #parent_type_tokens

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
    is_unique: bool,
    card_min: Option<u32>,
    card_max: Option<Option<u32>>,
    _custom_name: Option<String>,
}

struct FieldAttrs {
    is_key: bool,
    is_unique: bool,
    card_min: Option<u32>,
    card_max: Option<Option<u32>>,
    custom_name: Option<String>,
}

struct EntityAttrs {
    name: String,
    is_abstract: bool,
    parent_type: Option<String>,
}

/// Parse `#[entity(name = "...", abstract, extends = "...")]` from struct attributes.
fn parse_entity_attrs(attrs: &[syn::Attribute]) -> syn::Result<EntityAttrs> {
    for attr in attrs {
        if !attr.path().is_ident("entity") {
            continue;
        }
        let mut entity_name: Option<String> = None;
        let mut is_abstract = false;
        let mut parent_type: Option<String> = None;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                entity_name = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("r#abstract") || meta.path.is_ident("abstract") {
                is_abstract = true;
                Ok(())
            } else if meta.path.is_ident("extends") {
                let value: LitStr = meta.value()?.parse()?;
                parent_type = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `name`, `abstract`, or `extends`"))
            }
        })?;

        if let Some(name) = entity_name {
            return Ok(EntityAttrs {
                name,
                is_abstract,
                parent_type,
            });
        }
    }
    Err(syn::Error::new(
        proc_macro2::Span::call_site(),
        "Missing `#[entity(name = \"...\")]`",
    ))
}

/// Parse field-level attributes: `#[field(key)]`, `#[field(unique)]`,
/// `#[field(name = "...")]`, `#[field(card_min = N)]`, `#[field(card_max = M)]`
fn parse_field_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldAttrs> {
    let mut is_key = false;
    let mut is_unique = false;
    let mut card_min: Option<u32> = None;
    let mut card_max: Option<Option<u32>> = None;
    let mut custom_name: Option<String> = None;

    for attr in attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("key") {
                is_key = true;
                Ok(())
            } else if meta.path.is_ident("unique") {
                is_unique = true;
                Ok(())
            } else if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                custom_name = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("card_min") {
                let value: syn::LitInt = meta.value()?.parse()?;
                card_min = Some(value.base10_parse()?);
                Ok(())
            } else if meta.path.is_ident("card_max") {
                let value: syn::LitInt = meta.value()?.parse()?;
                card_max = Some(Some(value.base10_parse()?));
                Ok(())
            } else {
                Err(meta.error("expected `key`, `unique`, `name`, `card_min`, or `card_max`"))
            }
        })?;
    }

    // If card_min is set but card_max is not, default to unbounded
    if card_min.is_some() && card_max.is_none() {
        card_max = Some(None);
    }

    Ok(FieldAttrs {
        is_key,
        is_unique,
        card_min,
        card_max,
        custom_name,
    })
}

/// Build annotation tokens from field flags.
fn build_annotations(
    is_key: bool,
    is_unique: bool,
    card_min: Option<u32>,
    card_max: Option<Option<u32>>,
) -> proc_macro2::TokenStream {
    let mut annots = Vec::new();
    if is_key {
        annots.push(quote! { type_bridge_orm::Annotation::Key });
    }
    if is_unique {
        annots.push(quote! { type_bridge_orm::Annotation::Unique });
    }
    if let Some(min) = card_min {
        let max_tokens = match card_max {
            Some(Some(m)) => quote! { Some(#m) },
            _ => quote! { None },
        };
        annots.push(quote! { type_bridge_orm::Annotation::Card(#min, #max_tokens) });
    }
    if annots.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#annots),*] }
    }
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
