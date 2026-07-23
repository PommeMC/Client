use proc_macro2::TokenStream;
use quote::quote;
use syn::ItemStruct;

pub fn plugin(_attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let input = syn::parse2::<ItemStruct>(item)?;

    Ok(quote! {
        #input

        #[::stabby::export]
        pub extern "C" fn load_plugin() -> ::plugin_api::PluginModule {
            ::plugin_api::PluginModule {
                name: env!("CARGO_PKG_NAME").into(),
                version: ::plugin_api::meta::Version {
                    major: ::plugin_api::meta::parse_u32(env!("CARGO_PKG_VERSION_MAJOR")),
                    minor: ::plugin_api::meta::parse_u32(env!("CARGO_PKG_VERSION_MINOR")),
                    patch: ::plugin_api::meta::parse_u32(env!("CARGO_PKG_VERSION_PATCH")),
                },
                plugin: ::stabby::boxed::Box::new(<ExamplePlugin as Plugin>::new()).into(),
            }
        }

        #[unsafe(no_mangle)]
        pub static PLUGIN_MARKER: ::plugin_api::meta::PluginMarker = ::plugin_api::meta::PLUGIN_MARKER_VALUE;
        #[unsafe(no_mangle)]
        pub static PLUGIN_API_VERSION: ::plugin_api::meta::PluginApiVersion = ::plugin_api::meta::PLUGIN_API_VERSION_VALUE;

        pub use ::plugin_api::meta::setup_shared_logger_ref;
    })
}
