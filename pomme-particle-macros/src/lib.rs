//! Internal field-generating attributes for the client particle layer.
//!
//! Place these attributes before ordinary derives so derives see the appended
//! private fields. Generated paths target client physics/renderer types and
//! `::glam`; this is not a downstream-crate API.
//! Common state follows Vanilla 26.3 `Particle` and `SingleQuadParticle`
//! ownership, including fields without accessors.
//!
//! Both attributes accept one optional `exclude(field, ...)` list, with
//! optional trailing commas. Empty arguments or `exclude()` generate all common
//! fields. Exclusion skips a field's automatic declaration, inherent helpers
//! and type-specific legacy forwards. Manually declared excluded storage is
//! untouched and has no automatic methods or type requirements. Unknown family
//! fields, duplicate exclusions and explicit declarations of unexcluded common
//! fields are errors.
//!
//! Forwarding with no arguments always emits the full default method set.
//! Pass the original struct identifier to select its non-excluded methods, e.g.
//! `forward_particle_accessors!(FallingLeavesParticle)`. Use the typed form
//! after the struct in its defining module; aliases, qualified types and `Self`
//! inference are not supported. Particle Any casts are always forwarded.

use proc_macro::TokenStream;
use proc_macro2::{Ident, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, Field, Fields, Token, parenthesized};

struct FieldSpec {
    name: &'static str,
    field_type: TokenStream2,
    getter: Option<&'static str>,
    setter: Option<(&'static str, &'static str)>,
}

struct MethodSpec {
    name: Ident,
    parameters: TokenStream2,
    output: TokenStream2,
    arguments: TokenStream2,
    body: TokenStream2,
}

impl FieldSpec {
    fn methods(&self) -> syn::Result<Vec<MethodSpec>> {
        let field = syn::parse_str::<Ident>(self.name)?;
        let field_type = &self.field_type;
        let mut methods = Vec::new();
        if let Some(getter) = self.getter {
            methods.push(MethodSpec {
                name: syn::parse_str(getter)?,
                parameters: quote!(&self),
                output: quote!(-> #field_type),
                arguments: quote!(self),
                body: quote!(self.#field),
            });
        }
        if let Some((setter, parameter)) = self.setter {
            let parameter = syn::parse_str::<Ident>(parameter)?;
            methods.push(MethodSpec {
                name: syn::parse_str(setter)?,
                parameters: quote!(&mut self, #parameter: #field_type),
                output: TokenStream2::new(),
                arguments: quote!(self, #parameter),
                body: quote!(self.#field = #parameter;),
            });
        }
        if self.name == "removed" {
            methods.push(MethodSpec {
                name: syn::parse_str("remove")?,
                parameters: quote!(&mut self),
                output: TokenStream2::new(),
                arguments: quote!(self),
                body: quote!(self.#field = true;),
            });
            methods.push(MethodSpec {
                name: syn::parse_str("is_alive")?,
                parameters: quote!(&self),
                output: quote!(-> bool),
                arguments: quote!(self),
                body: quote!(!self.#field),
            });
        }
        Ok(methods)
    }
}

#[derive(Clone, Copy)]
enum Family {
    Particle,
    Quad,
}

impl Family {
    fn name(self) -> &'static str {
        match self {
            Self::Particle => "particle",
            Self::Quad => "quad",
        }
    }

    fn fields(self) -> Vec<FieldSpec> {
        match self {
            Self::Particle => particle_fields(),
            Self::Quad => quad_fields(),
        }
    }

    fn forwarding_macro(self, name: &Ident) -> Ident {
        format_ident!("__pomme_forward_{}_{}", self.name(), name.unraw())
    }
}

struct AccessorArgs {
    excluded: Punctuated<Ident, Token![,]>,
}

impl Parse for AccessorArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self {
            excluded: Punctuated::new(),
        };
        let mut seen_exclude = false;
        while !input.is_empty() {
            let option: Ident = input.parse()?;
            if option.unraw() != "exclude" {
                return Err(syn::Error::new(
                    option.span(),
                    format!("unknown option `{option}`; expected `exclude(...)`"),
                ));
            }
            if seen_exclude {
                return Err(syn::Error::new(
                    option.span(),
                    "repeated `exclude` option; use a single exclusion list",
                ));
            }
            seen_exclude = true;
            let content;
            parenthesized!(content in input);
            args.excluded = Punctuated::<Ident, Token![,]>::parse_terminated(&content)?;
            if !input.is_empty() {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

#[proc_macro_attribute]
pub fn particle_accessors(args: TokenStream, input: TokenStream) -> TokenStream {
    accessors(args, input, Family::Particle)
}

fn particle_fields() -> Vec<FieldSpec> {
    vec![
        FieldSpec {
            name: "position",
            field_type: quote!(::glam::DVec3),
            getter: Some("get_pos"),
            setter: None,
        },
        FieldSpec {
            name: "previous_position",
            field_type: quote!(::glam::DVec3),
            getter: Some("get_previous_pos"),
            setter: None,
        },
        FieldSpec {
            name: "velocity",
            field_type: quote!(::glam::DVec3),
            getter: Some("get_velocity"),
            setter: Some(("set_velocity", "velocity")),
        },
        FieldSpec {
            name: "bounds",
            field_type: quote!(crate::physics::aabb::Aabb),
            getter: Some("get_bounding_box"),
            setter: Some(("set_bounding_box", "bounds")),
        },
        FieldSpec {
            name: "width",
            field_type: quote!(f32),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "height",
            field_type: quote!(f32),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "removed",
            field_type: quote!(bool),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "on_ground",
            field_type: quote!(bool),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "stopped_by_collision",
            field_type: quote!(bool),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "has_physics",
            field_type: quote!(bool),
            getter: Some("has_physics"),
            setter: Some(("set_has_physics", "enabled")),
        },
        FieldSpec {
            name: "gravity",
            field_type: quote!(f32),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "friction",
            field_type: quote!(f32),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "speed_up_when_y_motion_is_blocked",
            field_type: quote!(bool),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "age",
            field_type: quote!(u32),
            getter: Some("get_age"),
            setter: None,
        },
        FieldSpec {
            name: "lifetime",
            field_type: quote!(u32),
            getter: Some("get_lifetime"),
            setter: Some(("set_lifetime", "lifetime")),
        },
    ]
}

#[proc_macro_attribute]
pub fn quad_accessors(args: TokenStream, input: TokenStream) -> TokenStream {
    accessors(args, input, Family::Quad)
}

fn quad_fields() -> Vec<FieldSpec> {
    vec![
        FieldSpec {
            name: "quad_size",
            field_type: quote!(f32),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "color",
            field_type: quote!([f32; 3]),
            getter: Some("get_color"),
            setter: Some(("set_color", "color")),
        },
        FieldSpec {
            name: "alpha",
            field_type: quote!(f32),
            getter: Some("get_alpha"),
            setter: Some(("set_alpha", "alpha")),
        },
        FieldSpec {
            name: "roll",
            field_type: quote!(f32),
            getter: Some("get_roll"),
            setter: Some(("set_roll", "roll")),
        },
        FieldSpec {
            name: "previous_roll",
            field_type: quote!(f32),
            getter: None,
            setter: None,
        },
        FieldSpec {
            name: "sprite",
            field_type: quote!(crate::renderer::chunk::atlas::AtlasRegion),
            getter: Some("get_sprite"),
            setter: Some(("set_sprite", "sprite")),
        },
    ]
}

fn accessors(args: TokenStream, input: TokenStream, family: Family) -> TokenStream {
    match expand_accessors(args.into(), input, family) {
        Ok(output) => output.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_accessors(
    args: TokenStream2,
    input: TokenStream,
    family: Family,
) -> syn::Result<TokenStream2> {
    let attribute = format!("{}_accessors", family.name());
    let specs = family.fields();
    let args = syn::parse2::<AccessorArgs>(args)
        .map_err(|error| syn::Error::new(error.span(), format!("{attribute}: {error}")))?;
    let mut excluded_names = Vec::new();
    for excluded in &args.excluded {
        let name = excluded.unraw().to_string();
        if !specs.iter().any(|spec| spec.name == name) {
            return Err(syn::Error::new(
                excluded.span(),
                format!("{attribute} has no field `{name}` to exclude"),
            ));
        }
        if excluded_names.contains(&name) {
            return Err(syn::Error::new(
                excluded.span(),
                format!("{attribute} excludes field `{name}` more than once"),
            ));
        }
        excluded_names.push(name);
    }
    let mut input = syn::parse::<DeriveInput>(input)?;
    let fields = match &mut input.data {
        Data::Struct(data) => match &mut data.fields {
            Fields::Named(fields) => &mut fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    format!("{attribute} requires a struct with named fields"),
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                format!("{attribute} requires a struct with named fields"),
            ));
        }
    };
    let mut methods = Vec::new();
    for spec in &specs {
        if excluded_names.iter().any(|name| name == spec.name) {
            continue;
        }
        let explicit_field = fields.iter().find(|field| {
            field
                .ident
                .as_ref()
                .is_some_and(|name| name.unraw() == spec.name)
        });
        if let Some(field) = explicit_field {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "{attribute} owns field `{}`; remove its explicit declaration or duplicate attribute",
                    spec.name
                ),
            ));
        }
        let name = syn::parse_str::<Ident>(spec.name)?;
        let field_type = &spec.field_type;
        fields.push(Field::parse_named.parse2(quote!(#name: #field_type))?);
        methods.extend(spec.methods()?);
    }
    let name = &input.ident;
    let forwarding_macro = family.forwarding_macro(name);
    let (helpers, forwards) = emit_methods(family, &methods);
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        #input

        impl #impl_generics #name #type_generics #where_clause {
            #helpers
        }

        #[allow(unused_macros)]
        macro_rules! #forwarding_macro {
            () => {
                #forwards
            };
        }
    })
}

fn emit_methods(family: Family, methods: &[MethodSpec]) -> (TokenStream2, TokenStream2) {
    let mut helpers = TokenStream2::new();
    let mut forwards = TokenStream2::new();
    if let Family::Particle = family {
        forwards.extend(quote! {
            fn as_any(&self) -> &dyn ::std::any::Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn ::std::any::Any {
                self
            }
        });
    }
    for method in methods {
        let MethodSpec {
            name,
            parameters,
            output,
            arguments,
            body,
        } = method;
        let helper = format_ident!("__pomme_{}_{}", family.name(), name);
        helpers.extend(quote! {
            fn #helper(#parameters) #output {
                #body
            }
        });
        forwards.extend(quote! {
            fn #name(#parameters) #output {
                Self::#helper(#arguments)
            }
        });
    }
    (helpers, forwards)
}

struct ForwardArgs {
    name: Option<Ident>,
}

impl Parse for ForwardArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let message =
            "expected no arguments or one original struct identifier (not a qualified type)";
        let name = if input.is_empty() {
            None
        } else {
            let name: Ident = input.parse().map_err(|_| input.error(message))?;
            if name == "Self" {
                return Err(syn::Error::new(name.span(), message));
            }
            Some(name)
        };
        if !input.is_empty() {
            return Err(input.error(message));
        }
        Ok(Self { name })
    }
}

#[proc_macro]
pub fn forward_particle_accessors(input: TokenStream) -> TokenStream {
    forward_accessors(input, Family::Particle)
}

#[proc_macro]
pub fn forward_quad_accessors(input: TokenStream) -> TokenStream {
    forward_accessors(input, Family::Quad)
}

fn forward_accessors(input: TokenStream, family: Family) -> TokenStream {
    let result = (|| {
        let args = syn::parse::<ForwardArgs>(input)?;
        if let Some(name) = args.name {
            let forwarding_macro = family.forwarding_macro(&name);
            return Ok(quote!(#forwarding_macro!();));
        }
        let mut methods = Vec::new();
        for spec in family.fields() {
            methods.extend(spec.methods()?);
        }
        Ok::<_, syn::Error>(emit_methods(family, &methods).1)
    })();
    match result {
        Ok(output) => output.into(),
        Err(error) => error.to_compile_error().into(),
    }
}
