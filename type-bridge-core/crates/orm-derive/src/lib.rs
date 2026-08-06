//! Generated-query derives for the TypeBridge Rust client.

use proc_macro::TokenStream;

mod selected_row;

/// Derive a declaration-ordered named selection constructor for the public
/// generated Rust query facade.
#[proc_macro_derive(SelectedRow)]
pub fn derive_selected_row(input: TokenStream) -> TokenStream {
    selected_row::derive(input.into())
        .unwrap_or_else(|error| error.to_compile_error())
        .into()
}
