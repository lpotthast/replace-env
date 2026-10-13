#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]

use darling::{FromDeriveInput, FromField, ast};
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{DeriveInput, Type, parse_macro_input, spanned::Spanned};

const PRIMITIVE_TYPES: [&str; 14] = [
    "String", "u8", "u16", "u32", "u64", "u128", "i8", "i16", "i32", "i64", "i128", "f32", "f64",
    "bool",
];

/// Creates an error spanning `spanned`, rendering the `help` text below the main `message`.
fn error_with_help(spanned: &impl Spanned, message: &str, help: &str) -> darling::Error {
    darling::Error::custom(format!("{message}\n\n  = help: {help}")).with_span(spanned)
}

struct RawType {
    is_option: bool,
    ident: syn::Ident,
}

#[derive(Debug, FromField)]
#[darling(attributes(replace_env))]
struct MyFieldReceiver {
    ident: Option<syn::Ident>,

    ty: syn::Type,

    /// For secure fields will not print their value (or even their specified default value)!
    secret: Option<bool>,

    raw_type: Option<syn::Ident>,
}

struct TypeInfo {
    // Whether or not the ident is the type itself of was extracted from an Option<...>.
    is_option: bool,
    // The actual type. Might have come from inside an Option<...>.
    ident: Ident,
}

fn get_type_info(ty: &Type) -> darling::Result<TypeInfo> {
    let Type::Path(path) = ty else {
        return Err(darling::Error::custom("Only path types are supported!").with_span(ty));
    };
    let segment = &path.path.segments[0];
    if segment.ident != "Option" {
        return Ok(TypeInfo {
            is_option: false,
            ident: segment.ident.clone(),
        });
    }
    let syn::PathArguments::AngleBracketed(ab) = &segment.arguments else {
        return Err(darling::Error::custom("Expected angle brackets!").with_span(path));
    };
    let Some(syn::GenericArgument::Type(inner)) = ab.args.first() else {
        return Err(darling::Error::custom("Expected type in angle brackets!").with_span(path));
    };
    let Type::Path(inner) = inner else {
        return Err(darling::Error::custom("Only path types are supported!").with_span(path));
    };
    Ok(TypeInfo {
        is_option: true,
        ident: inner.path.segments[0].ident.clone(),
    })
}

impl MyFieldReceiver {
    fn raw_type(&self, type_info: &TypeInfo) -> darling::Result<RawType> {
        let TypeInfo { is_option, ident } = type_info;
        let is_option = *is_option;
        let is_primitive = PRIMITIVE_TYPES.contains(&ident.to_string().as_str());
        match self.raw_type.clone() {
            Some(raw_type) if raw_type == "String" && is_primitive => Err(error_with_help(
                &self.ty,
                "Do not specify `replace_env(raw_type = \"String\")` for primitive types for which 'String' will be the inferred type anyway.",
                "Remove attribute `replace_env(raw_type = \"String\")`",
            )),
            Some(raw_type) => Ok(RawType {
                is_option,
                ident: raw_type,
            }),
            None if is_primitive => Ok(RawType {
                is_option,
                ident: Ident::new("String", self.ty.span()),
            }),
            None => Err(error_with_help(
                &self.ty,
                &format!("Expected a primitive type like `String`, 'u32', ... But got: {ident}"),
                "Declare this field with: #[replace_env(raw_type = \"String\")] if it should be read as a string or #[replace_env(raw_type = \"YourOwnType\")] if its a type of your own that itself derived ReplaceEnv.",
            )),
        }
    }
}

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(replace_env), supports(struct_any))]
struct MyInputReceiver {
    ident: syn::Ident,

    data: ast::Data<(), MyFieldReceiver>,
}

impl MyInputReceiver {
    pub fn fields(&self) -> &ast::Fields<MyFieldReceiver> {
        match &self.data {
            ast::Data::Enum(_) => panic!("Only structs are supported"),
            ast::Data::Struct(fields) => fields,
        }
    }
}

/// Creates the initializer of the field in the `From<RawType>` conversion, converting the raw field value to its real type.
fn from_raw_field_initializer(
    field: &MyFieldReceiver,
    type_info: &TypeInfo,
    raw_type: &RawType,
) -> proc_macro2::TokenStream {
    let name = field.ident.as_ref().expect("Expected named field!");
    // Not every type can be transformed from its 'RawType' to its normal 'Type'.
    // Special case: String -> bool: some_string.parse::<bool>();

    // TODO: Take raw type into consideration as well.

    // If the user wants a boolean, we have to parse into a bool.
    let conv_actiono = if type_info.ident == "bool" {
        let expectation = format!(
            "Expected field '{name}' to be of type bool. But value read was: '{{}}', which was not parsable to a bool. Use 'false' or 'true'. Original error was: '{{}}'"
        );
        quote! {
            match orig.parse::<bool>() {
                Ok(val) => val,
                Err(err) => panic!(#expectation, orig, err) // TODO: Create error instead of panicking!
            }
        }
    }
    // If the user wants a u32, we have to parse into a u32.
    else if type_info.ident == "u32" {
        let expectation = format!(
            "Expected field '{name}' to be of type u32. But value read was: '{{}}', which was not parsable to a u32. Original error was: '{{}}'"
        );
        quote! {
            match orig.parse::<u32>() {
                Ok(val) => val,
                Err(err) => panic!(#expectation, orig, err) // TODO: Create error instead of panicking!
            }
        }
    // } else if path.path.segments[0].ident.to_string().starts_with("Raw") {
    //     quote! { raw.#name.into() }
    // } else if path.path.is_ident("String") {
    //     quote! { raw.#name.into() }
    } else {
        quote! { orig.into() }
    };

    let final_conv_action = match (raw_type.is_option, raw_type.ident == "String") {
        // TODO: Do we want empty check to be optional (based on user desire)? If excluded, parsing will typically fail...
        (true, true) => quote! {
            {
                let orig = raw.#name;
                match orig {
                    Some(orig) => {
                        if orig == "" {
                            None
                        } else {
                            Some(#conv_actiono)
                        }
                    },
                    None => None,
                }
            }
        },
        (true, false) => quote! {
            {
                let orig = raw.#name;
                orig.map(|orig| #conv_actiono)
            }
        },
        (false, _) => quote! {
            {
                let orig = raw.#name;
                #conv_actiono
            }
        },
    };

    quote! { #name: #final_conv_action }
}

#[proc_macro_derive(ReplaceEnv, attributes(replace_env))]
pub fn store(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    match expand(&ast) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.write_errors().into(),
    }
}

fn expand(ast: &DeriveInput) -> darling::Result<proc_macro2::TokenStream> {
    let input = MyInputReceiver::from_derive_input(ast)?;

    let ident = &input.ident;

    let fields = input.fields().iter().collect::<Vec<_>>();

    let replace_env_field_initializers = fields
        .iter()
        .map(|field| {
            let name = field.ident.as_ref().expect("Expected named field!");
            let secret = field.secret.unwrap_or(false);
            quote! { #name: self.#name.replace_env(replace_env::Metadata {
                secret: #secret,
            }) }
        })
        .collect::<Vec<_>>();

    /*
    All simple fields should be read as strings.
    All non-recognizable types should stay as they were defined.
    */

    let raw_type_ident = Ident::new(format!("Raw{ident}").as_str(), Span::call_site());

    let mut errors = darling::Error::accumulator();
    let fields_with_raw_type = fields
        .into_iter()
        .filter_map(|field| {
            errors.handle_in(|| {
                let type_info = get_type_info(&field.ty)?;
                let raw_type = field.raw_type(&type_info)?;
                Ok((field, type_info, raw_type))
            })
        })
        .collect::<Vec<_>>();
    errors.finish()?;

    let raw_field_type_declarations =
        fields_with_raw_type
            .iter()
            .map(|(field, _type_info, RawType { is_option, ident })| {
                let name = field.ident.as_ref().expect("Expected named field!");
                let raw_type = match is_option {
                    true => quote! { Option<#ident> },
                    false => quote! { #ident },
                };
                quote! {
                    #name: #raw_type
                }
            });

    let from_raw_field_initializers = fields_with_raw_type
        .iter()
        .map(|(field, type_info, raw_type)| from_raw_field_initializer(field, type_info, raw_type));

    // This is our derive implementation. We create:
    // 1. The RawType (in which all fields are Strings or special user-defined raw types) using the `raw_type_ident` and the `raw_field_type_declarations`.
    // 2. The From<'raw_type_ident'> for 'ident' conversion, with which a raw type instance can be converted in its real representation.
    //    This converts all string/raw fields to their real type, parsing booleans, integers, floats or converting strings to enum value using serde.
    // 3. The ReplaceEnv implementation for the 'raw_type_ident' which lets us replace environment variable names in the raw types fields before converting it to our real type.
    Ok(quote! {
        // 1.
        #[derive(Debug, serde::Deserialize)]
        struct #raw_type_ident {
            #(#raw_field_type_declarations),*
        }

        // 2.
        impl From<#raw_type_ident> for #ident {
            fn from(raw: #raw_type_ident) -> Self {
                Self {
                    #(#from_raw_field_initializers),*
                }
            }
        }

        // 3.
        impl replace_env::ReplaceEnv for #raw_type_ident {
            fn replace_env(self, _metadata: replace_env::Metadata) -> Self {
                Self {
                    #(#replace_env_field_initializers),*
                }
            }
        }
    })
}
