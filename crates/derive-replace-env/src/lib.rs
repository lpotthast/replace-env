#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]

use darling::{ast, FromDeriveInput, FromField};
use proc_macro::TokenStream;
use proc_macro2::{Ident, Span};
use proc_macro_error::{abort, proc_macro_error};
use quote::quote;
use syn::{parse_macro_input, spanned::Spanned, DeriveInput, Error, Type};

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

fn get_type_info(ty: &Type) -> TypeInfo {
    let span = ty.span();
    match ty {
        syn::Type::Path(path) => {
            let span = path.span();
            match path.path.segments[0].ident.to_string().as_str() {
                "Option" => match &path.path.segments[0].arguments {
                    syn::PathArguments::AngleBracketed(ab) => {
                        match ab.args.first().expect("present") {
                            syn::GenericArgument::Type(t) => match t {
                                Type::Path(p) => TypeInfo {
                                    is_option: true,
                                    ident: p.path.segments[0].ident.clone(),
                                },
                                _ => abort!(span, "Only path types are supported!"),
                            },
                            _ => abort!(span, "Expected type in angle brackets!"),
                        }
                    }
                    _ => abort!(span, "Expected angle brackets!"),
                },
                _ => TypeInfo {
                    is_option: false,
                    ident: path.path.segments[0].ident.clone(),
                },
            }
        }
        _ => abort!(span, "Only path types are supported!"),
    }
}

impl MyFieldReceiver {
    pub fn raw_type(&self) -> Result<RawType, Error> {
        let TypeInfo { is_option, ident } = get_type_info(&self.ty);
        self.raw_type.clone().map(|raw_type| {
            if raw_type == "String" {
                match ident.to_string().as_str() {
                    "String" | "u8" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i32" | "i64" | "i128" | "f32" | "f64" | "bool"
                    =>  abort! {self.ty.span(), "Do not specify `replace_env(raw_type = \"String\")` for primitive types for which 'String' will be the inferred type anyway."; help = "Remove attribute `replace_env(raw_type = \"String\")`"},
                    _non_primitive_type => Ok(RawType { is_option, ident: raw_type })
                }
            } else {
                Ok(RawType { is_option, ident: raw_type })
            }
        }).unwrap_or_else(|| {
            match ident.to_string().as_str() {
                "String" | "u8" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i32" | "i64" | "i128" | "f32" | "f64" | "bool"
                    => Ok(RawType { is_option, ident: Ident::new("String", self.ty.span()) }),
                other => {
                    let message = format!("Expected a primitive type like `String`, 'u32', ... But got: {other}");
                    abort!(
                        self.ty.span(), message;
                        help = "Declare this field with: #[replace_env(raw_type = \"String\")] if it should be read as a string or #[replace_env(raw_type = \"YourOwnType\")] if its a type of your own that itself derived ReplaceEnv.";
                    );
                }
            }
        })
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

#[proc_macro_derive(ReplaceEnv, attributes(replace_env))]
#[proc_macro_error]
pub fn store(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);

    let input: MyInputReceiver = match FromDeriveInput::from_derive_input(&ast) {
        Ok(args) => args,
        Err(err) => return darling::Error::write_errors(err).into(),
    };

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

    let raw_type_ident = Ident::new(format!("Raw{}", ident).as_str(), Span::call_site());

    let fields_with_raw_type = fields
        .into_iter()
        .map(|field| {
            let type_info = get_type_info(&field.ty);
            let raw = field.raw_type();
            if let Err(err) = raw {
                abort!(err);
            }
            (
                field,
                type_info,
                raw.expect("Must be present. This is a bug."),
            )
        })
        .collect::<Vec<_>>();

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

    let from_raw_field_initializers = fields_with_raw_type.iter().map(|(field, type_info, raw_type)| {
        let name = field.ident.as_ref().expect("Expected named field!");
        // Not every type can be transformed from its 'RawType' to its normal 'Type'.
        // Special case: String -> bool: some_string.parse::<bool>();

        // TODO: Take raw type into consideration as well.

        // If the user wants a boolean, we have to parse into a bool.
        let conv_actiono = if type_info.ident == "bool" {
            let expectation = format!("Expected field '{name}' to be of type bool. But value read was: '{{}}', which was not parsable to a bool. Use 'false' or 'true'. Original error was: '{{}}'");
            quote! {
                match orig.parse::<bool>() {
                    Ok(val) => val,
                    Err(err) => panic!(#expectation, orig, err) // TODO: Create error instead of panicking!
                }
            }
        }
        // If the user wants a u32, we have to parse into a u32.
        else if type_info.ident == "u32" {
            let expectation = format!("Expected field '{name}' to be of type u32. But value read was: '{{}}', which was not parsable to a u32. Original error was: '{{}}'");
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

        let final_conv_action = match (raw_type.is_option, raw_type.ident == "String")  {
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
    });

    // This is our derive implementation. We create:
    // 1. The RawType (in which all fields are Strings or special user-defined raw types) using the `raw_type_ident` and the `raw_field_type_declarations`.
    // 2. The From<'raw_type_ident'> for 'ident' conversion, with which a raw type instance can be converted in its real representation.
    //    This converts all string/raw fields to their real type, parsing booleans, integers, floats or converting strings to enum value using serde.
    // 3. The ReplaceEnv implementation for the 'raw_type_ident' which lets us replace environment variable names in the raw types fields before converting it to our real type.
    quote! {
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
    }
    .into()
}
