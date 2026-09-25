//! Proc macros re-exported by `seaquel-runtime`. Depend on `seaquel-runtime`
//! and write `#[seaquel_runtime::async_trait]`; don't use this crate directly.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

/// `async_trait` that drops the `Send` bound on wasm32.
///
/// Futures backed by JavaScript aren't `Send`, so the browser build of Core
/// needs `#[async_trait(?Send)]` while native builds need the default. Writing
/// both `cfg_attr` lines by hand on every trait and impl is easy to get wrong,
/// so every Seaquel async trait uses this attribute instead.
#[proc_macro_attribute]
pub fn async_trait(args: TokenStream, item: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return quote! {
            compile_error!("#[seaquel_runtime::async_trait] takes no arguments");
        }
        .into();
    }
    let item = TokenStream2::from(item);
    quote! {
        #[cfg_attr(
            not(target_arch = "wasm32"),
            ::seaquel_runtime::__private::async_trait::async_trait
        )]
        #[cfg_attr(
            target_arch = "wasm32",
            ::seaquel_runtime::__private::async_trait::async_trait(?Send)
        )]
        #item
    }
    .into()
}
