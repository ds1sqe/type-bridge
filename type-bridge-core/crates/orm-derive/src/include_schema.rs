//! Implementation of `include_schema!` proc-macro.
//!
//! Reads a TypeQL `.tql` file at compile time and delegates inline Rust model
//! generation to the shared core bindgen renderer.

use std::path::PathBuf;

use proc_macro2::TokenStream;
use syn::LitStr;

use type_bridge_core_lib::bindgen::BindgenPlan;

pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let lit: LitStr = syn::parse2(input)?;
    let path = lit.value();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| syn::Error::new(lit.span(), "CARGO_MANIFEST_DIR not set"))?;
    let full_path = PathBuf::from(&manifest_dir).join(&path);
    let content = std::fs::read_to_string(&full_path).map_err(|e| {
        syn::Error::new(
            lit.span(),
            format!("cannot read {}: {}", full_path.display(), e),
        )
    })?;

    let plan = BindgenPlan::from_typeql(&content)
        .map_err(|e| syn::Error::new(lit.span(), format!("schema parse error: {e}")))?;
    let code = plan.render_rust_inline();
    let tokens: TokenStream = code.parse().map_err(|e: proc_macro2::LexError| {
        syn::Error::new(lit.span(), format!("generated code parse error: {e}"))
    })?;

    Ok(tokens)
}
