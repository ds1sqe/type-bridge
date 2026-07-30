use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Result, parse2};

pub(crate) fn derive(input: TokenStream) -> Result<TokenStream> {
    let input = parse2::<DeriveInput>(input)?;
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            input.generics,
            "SelectedRow does not support generic row declarations",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input.ident,
            "SelectedRow supports named structs only",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "SelectedRow supports named structs only",
        ));
    };
    if fields.named.is_empty() || fields.named.len() > 16 {
        return Err(syn::Error::new_spanned(
            fields,
            "SelectedRow requires between 1 and 16 fields",
        ));
    }

    let row = &input.ident;
    let field_names = fields
        .named
        .iter()
        .map(|field| field.ident.as_ref().expect("named field"))
        .collect::<Vec<_>>();
    let field_name_strings = field_names
        .iter()
        .map(|field| field.to_string())
        .collect::<Vec<_>>();
    let field_types = fields
        .named
        .iter()
        .map(|field| &field.ty)
        .collect::<Vec<_>>();
    let selections = (0..field_names.len())
        .map(|index| format_ident!("__Selection{index}"))
        .collect::<Vec<_>>();

    Ok(quote! {
        impl #row {
            /// Build this declaration-ordered named selected shape.
            pub fn select<__Schema, #(#selections),*>(
                #(#field_names: #selections),*
            ) -> ::type_bridge::Result<
                ::type_bridge::NamedSelection<
                    __Schema,
                    Self,
                    (#(#selections,)*)
                >
            >
            where
                __Schema: ::type_bridge::Schema,
                #(
                    #selections: ::type_bridge::SelectedSlot<
                        __Schema,
                        Output = #field_types
                    >
                ),*
            {
                ::type_bridge::NamedSelection::__new(
                    (#(#field_names,)*),
                    &[#(#field_name_strings),*],
                )
            }
        }

        impl ::type_bridge::SelectedRowSpec<(#(#field_types,)*)> for #row {
            fn __from_selected_outputs(
                (#(#field_names,)*): (#(#field_types,)*)
            ) -> Self {
                Self { #(#field_names),* }
            }
        }
    })
}
