mod plugin;

#[proc_macro_attribute]
pub fn plugin(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    plugin::plugin(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
