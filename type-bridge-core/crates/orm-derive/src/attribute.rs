//! Implementation of `#[derive(TypeBridgeAttribute)]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Fields, LitStr};

pub fn derive(input: TokenStream) -> syn::Result<TokenStream> {
    let input: DeriveInput = syn::parse2(input)?;
    let name = &input.ident;

    // Must be a newtype struct: struct Name(pub T);
    let data = match &input.data {
        syn::Data::Struct(s) => s,
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "TypeBridgeAttribute can only be derived for structs",
            ))
        }
    };

    let fields = match &data.fields {
        Fields::Unnamed(f) => f,
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "TypeBridgeAttribute requires a newtype struct (e.g., `struct Name(pub String)`)",
            ))
        }
    };

    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            fields,
            "TypeBridgeAttribute requires exactly one unnamed field",
        ));
    }

    // Parse #[attribute(name = "...", value_type = "...")]
    let (attr_name, value_type) = parse_attribute_attrs(&input.attrs)?;

    // Map value_type string to ValueType enum variant
    let value_type_enum = match value_type.as_str() {
        "string" => quote! { type_bridge_orm::ValueType::String },
        "long" => quote! { type_bridge_orm::ValueType::Long },
        "double" => quote! { type_bridge_orm::ValueType::Double },
        "boolean" => quote! { type_bridge_orm::ValueType::Boolean },
        "date" => quote! { type_bridge_orm::ValueType::Date },
        "datetime" => quote! { type_bridge_orm::ValueType::DateTime },
        "datetime-tz" => quote! { type_bridge_orm::ValueType::DateTimeTz },
        "decimal" => quote! { type_bridge_orm::ValueType::Decimal },
        "duration" => quote! { type_bridge_orm::ValueType::Duration },
        _ => quote! { type_bridge_orm::ValueType::String }, // unreachable, caught below
    };

    // Generate to_value/from_value based on value_type
    let (to_value_body, from_value_body) = match value_type.as_str() {
        "string" => (
            quote! { type_bridge_orm::AttributeValue::String(self.0.clone()) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::String(s) => Some(Self(s.clone())),
                    _ => None,
                }
            },
        ),
        "long" => (
            quote! { type_bridge_orm::AttributeValue::Long(self.0) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::Long(n) => Some(Self(*n)),
                    _ => None,
                }
            },
        ),
        "double" => (
            quote! { type_bridge_orm::AttributeValue::Double(self.0) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::Double(n) => Some(Self(*n)),
                    _ => None,
                }
            },
        ),
        "boolean" => (
            quote! { type_bridge_orm::AttributeValue::Boolean(self.0) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::Boolean(b) => Some(Self(*b)),
                    _ => None,
                }
            },
        ),
        "date" => (
            quote! { type_bridge_orm::AttributeValue::Date(self.0.clone()) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::Date(s) => Some(Self(s.clone())),
                    _ => None,
                }
            },
        ),
        "datetime" => (
            quote! { type_bridge_orm::AttributeValue::DateTime(self.0.clone()) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::DateTime(s) => Some(Self(s.clone())),
                    _ => None,
                }
            },
        ),
        "datetime-tz" => (
            quote! { type_bridge_orm::AttributeValue::DateTimeTZ(self.0.clone()) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::DateTimeTZ(s) => Some(Self(s.clone())),
                    _ => None,
                }
            },
        ),
        "decimal" => (
            quote! { type_bridge_orm::AttributeValue::Decimal(self.0.clone()) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::Decimal(s) => Some(Self(s.clone())),
                    _ => None,
                }
            },
        ),
        "duration" => (
            quote! { type_bridge_orm::AttributeValue::Duration(self.0.clone()) },
            quote! {
                match value {
                    type_bridge_orm::AttributeValue::Duration(s) => Some(Self(s.clone())),
                    _ => None,
                }
            },
        ),
        other => {
            return Err(syn::Error::new_spanned(
                &input,
                format!("Unsupported value_type: \"{other}\". Expected one of: string, long, double, boolean, date, datetime, datetime-tz, decimal, duration"),
            ));
        }
    };

    Ok(quote! {
        impl type_bridge_orm::TypeBridgeAttribute for #name {
            const ATTR_NAME: &'static str = #attr_name;
            const VALUE_TYPE: &'static str = #value_type;
            const VALUE_TYPE_ENUM: type_bridge_orm::ValueType = #value_type_enum;

            fn to_value(&self) -> type_bridge_orm::AttributeValue {
                #to_value_body
            }

            fn from_value(value: &type_bridge_orm::AttributeValue) -> Option<Self> {
                #from_value_body
            }
        }
    })
}

/// Parse `#[attribute(name = "...", value_type = "...")]` from struct attributes.
fn parse_attribute_attrs(attrs: &[syn::Attribute]) -> syn::Result<(String, String)> {
    let mut attr_name: Option<String> = None;
    let mut value_type: Option<String> = None;

    for attr in attrs {
        if !attr.path().is_ident("attribute") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value: LitStr = meta.value()?.parse()?;
                attr_name = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("value_type") {
                let value: LitStr = meta.value()?.parse()?;
                value_type = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("expected `name` or `value_type`"))
            }
        })?;
    }

    let attr_name = attr_name.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "Missing `#[attribute(name = \"...\")]`",
        )
    })?;
    let value_type = value_type.ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            "Missing `#[attribute(value_type = \"...\")]`",
        )
    })?;

    Ok((attr_name, value_type))
}
