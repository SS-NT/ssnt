// TODO: Document macro syntax

extern crate proc_macro;

use std::{collections::hash_map::DefaultHasher, hash::Hasher};

use darling::{ast, FromDeriveInput, FromField, ToTokens};
use proc_macro::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{
    parenthesized, parse::Parse, parse_macro_input, punctuated::Punctuated, spanned::Spanned,
    DeriveInput, Ident, Lit, Path, Token, Type,
};

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(networked), supports(struct_named))]
struct NetworkedInput {
    ident: Ident,
    data: ast::Data<darling::util::Ignored, NetworkedFieldInput>,
    #[darling(default)]
    client: Option<Type>,
    #[darling(default)]
    server: Option<Type>,
    #[darling(default)]
    priority: i16,
    #[darling(default = "default_param")]
    param: Type,
}

fn default_param() -> Type {
    syn::parse_quote!(())
}

#[derive(Debug, FromField)]
#[darling(attributes(networked))]
struct NetworkedFieldInput {
    ident: Option<Ident>,
    ty: Type,
    #[darling(default)]
    with: Option<FieldMethod>,
    #[darling(default)]
    updated: Option<Path>,
}

#[derive(Debug)]
struct FieldMethod {
    path: Path,
    params: Vec<Type>,
    networked_ty: Option<Type>,
}

impl Parse for FieldMethod {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut networked_ty = match input.peek2(Token![->]) {
            true => {
                let ty = input.parse::<Type>()?;
                input.parse::<Token![->]>()?;
                Some(ty)
            }
            false => None,
        };

        let path = input.parse()?;
        let content;
        parenthesized!(content in input);
        let params: Punctuated<_, Token![,]> = content.parse_terminated(Type::parse)?;

        if let Ok(t) = input.parse::<Token![->]>() {
            if networked_ty.is_some() {
                return Err(syn::Error::new_spanned(
                    t,
                    "Network type can only be specified once",
                ));
            }
            networked_ty = Some(input.parse::<Type>()?);
        }

        Ok(FieldMethod {
            path,
            params: params.into_iter().collect(),
            networked_ty,
        })
    }
}

impl darling::FromMeta for FieldMethod {
    fn from_value(value: &Lit) -> darling::Result<Self> {
        let value = match value {
            Lit::Str(v) => v,
            _ => return Err(darling::Error::unexpected_lit_type(value)),
        };
        Ok(value.parse()?)
    }
}

struct NetworkedField {
    ident: Ident,
    networked_type: Type,
    with: Option<FieldMethod>,
    updated: Option<Path>,
}

#[derive(Clone, Copy)]
enum NetworkedSide {
    Server,
    Client,
}

impl NetworkedSide {
    fn variable_name(&self) -> &'static str {
        match self {
            NetworkedSide::Server => "NetworkVar",
            NetworkedSide::Client => "ServerVar",
        }
    }
}

fn parse_networked_field_input(
    input: NetworkedFieldInput,
    side: NetworkedSide,
) -> darling::Result<Option<NetworkedField>> {
    let ident = input.ident.unwrap();

    // Parse the field type to extract the actual type that will be networked
    let update_type = &input.ty;
    let type_path = match update_type {
        Type::Path(p) => p,
        _ => {
            return Err(darling::Error::custom("Invalid variable type").with_span(update_type));
        }
    };

    let mut iterator = type_path.path.segments.iter();
    // Find the segment that has our relevant networked variable type
    // TODO: Isn't this just the last one?
    let segment = match iterator.find(|s| s.ident == side.variable_name()) {
        Some(s) => s,
        None => {
            // TODO: Differentiate between fields without annotation and with #[networked] annotation (see https://github.com/TedDriggs/darling/issues/167#issuecomment-1285517559)
            return Ok(None);
        }
    };

    // Try to find the generic type
    let networked_type = match &segment.arguments {
        syn::PathArguments::AngleBracketed(args) => match args.args.first() {
            Some(syn::GenericArgument::Type(t)) => Some(t),
            _ => None,
        },
        _ => None,
    }
    .ok_or_else(|| {
        darling::Error::custom(format!(
            "{} must have a generic argument",
            side.variable_name()
        ))
        .with_span(&segment.arguments)
    })?;

    Ok(Some(NetworkedField {
        ident,
        networked_type: networked_type.to_owned(),
        with: input.with,
        updated: input.updated,
    }))
}

fn transform_field_value(
    variable_access: proc_macro2::TokenStream,
    param_indices: &[usize],
    i: usize,
    with: &FieldMethod,
) -> proc_macro2::TokenStream {
    let method_path = &with.path;
    let paramset_method = format_ident!(
        "p{}",
        param_indices.iter().position(|&index| i == index).unwrap()
    );
    quote_spanned! { method_path.span() =>
        #method_path(#variable_access, param.#paramset_method())
    }
}

#[proc_macro_derive(Networked, attributes(networked))]
pub fn networked_derive(input: TokenStream) -> TokenStream {
    let derive_input = parse_macro_input!(input as DeriveInput);

    let input: NetworkedInput = match NetworkedInput::from_derive_input(&derive_input) {
        Ok(i) => i,
        Err(err) => {
            return err.write_errors().into();
        }
    };

    // Get client or server attribute on the struct
    let (side, matching_type) = match match [input.client, input.server] {
        [Some(c), None] => Ok((NetworkedSide::Server, c)),
        [None, Some(s)] => Ok((NetworkedSide::Client, s)),
        [Some(_), Some(s)] => {
            Err(darling::Error::custom("Only one of 'server' and 'client' may exist").with_span(&s))
        }
        [None, None] => Err(darling::Error::custom(
            "One of 'server' and 'client' must exist",
        )),
    } {
        Ok(o) => o,
        Err(err) => return err.write_errors().into(),
    };

    let networked_fields: Vec<_> = match input
        .data
        .take_struct()
        .expect("Should never be enum")
        .fields
        .into_iter()
        .map(|i| parse_networked_field_input(i, side))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(o) => o.into_iter().flatten().collect(),
        Err(err) => return err.write_errors().into(),
    };

    let (param_indices, params): (Vec<_>, Vec<_>) = networked_fields
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.with.as_ref().map(|with| (i, with)))
        .map(|(i, method)| (i, &method.params))
        .unzip();

    let mut hasher = DefaultHasher::new();
    for field in networked_fields.iter() {
        let ty = field
            .with
            .as_ref()
            .and_then(|w| w.networked_ty.as_ref())
            .unwrap_or(&field.networked_type);
        hasher.write(ty.to_token_stream().to_string().as_bytes());
    }
    let signature = hasher.finish();

    let paramset_param = match params.is_empty() {
        true => quote!(((),)),
        false => quote!(#( (#(#params),*,) ),*),
    };

    let paramset = quote! {
        bevy::prelude::ParamSet<'static, 'static, (#paramset_param)>
    };

    let name = input.ident;
    let priority = input.priority;
    let param = input.param;
    let method_param = quote_spanned!(param.span()=> param: &mut <<Self::Param as bevy::ecs::system::SystemParam>::Fetch as bevy::ecs::system::SystemParamFetch<'w, 's>>::Item);
    match side {
        NetworkedSide::Server => {
            // Build writes for the serialize method
            let writes = networked_fields
                .iter()
                .enumerate()
                .map(|(i, networked_field)| {
                    let var_name = &networked_field.ident;
                    let changed_name = format_ident!("{}_changed", var_name);
                    let networked_type = networked_field.with.as_ref().and_then(|w| w.networked_ty.as_ref()).unwrap_or(&networked_field.networked_type);

                    // Optionally transform the value before serializing
                    let value_expression = match networked_field.with.as_ref() {
                        Some(with) => {
                            let variable_access = quote_spanned! { var_name.span() =>
                                &self.#var_name
                            };
                            let transformation = transform_field_value(variable_access, &param_indices, i, with);
                            quote!(owned(#transformation))
                        }
                        None => {
                            let variable_access = quote_spanned! { var_name.span() =>
                                &*(self.#var_name)
                            };
                            quote!(from(#variable_access))
                        },
                    };
                    quote_spanned! { var_name.span() =>
                        let #changed_name = since_tick
                            .map(|t| self.#var_name.has_changed_since(t.into()))
                            .unwrap_or(true);
                        serde::Serialize::serialize(
                            &#changed_name.then(|| {
                                networking::variable::ValueUpdate::<#networked_type>::#value_expression
                            }),
                            &mut serializer,
                        )
                        .unwrap();
                    }
                }).collect::<Vec<_>>();

            let serialize_body =
                match writes.is_empty() {
                    true => {
                        // Optimization for networked marker structs
                        quote! {
                            if since_tick.is_some() {
                                None
                            } else {
                                Some(networking::variable::Bytes::new())
                            }
                        }
                    },
                    false => {
                        quote! {
                            let mut writer =
                                networking::variable::BufMut::writer(networking::variable::BytesMut::new());
                            let mut serializer = networking::variable::StandardSerializer::new(
                                &mut writer,
                                networking::variable::serializer_options(),
                            );

                            #(#writes)*

                            Some(writer.into_inner().into())
                        }
                    },
                };

            // Build trait update method
            let field_updates = networked_fields.iter().map(|networked_field| {
                let var_name = &networked_field.ident;

                quote! {
                    self.#var_name.update_state(tick)
                }
            }).collect::<Vec<_>>();
            let update_body = match field_updates.is_empty() {
                            true => quote!(false),
                            false => quote!(#(#field_updates)|*),
                        };

            // Build server trait implementation
            quote! {
                impl networking::variable::NetworkedToClient for #name {
                    type Param = #paramset;

                    fn receiver_matters() -> bool {
                        false
                    }

                    fn serialize<'w, 's>(
                        &self,
                        #method_param,
                        _: Option<networking::ConnectionId>,
                        since_tick: Option<u32>,
                    ) -> Option<networking::variable::Bytes> {
                        #serialize_body
                    }

                    fn update_state(&mut self, tick: u32) -> bool {
                        #update_body
                    }

                    fn priority(&self) -> i16 {
                        #priority
                    }

                    fn client_type_id() -> std::any::TypeId {
                        std::any::TypeId::of::<#matching_type>()
                    }

                    fn data_signature() -> u64 {
                        #signature
                    }
                }
            }
        }
        NetworkedSide::Client => {
            let reads = networked_fields.iter().enumerate().map(|(i, networked_field)| {
                let var_name = &networked_field.ident;
                let networked_type = networked_field.with.as_ref().and_then(|w| w.networked_ty.as_ref()).unwrap_or(&networked_field.networked_type);
                let update_name = format_ident!("{}_update", var_name);
                let new_name = format_ident!("{}_new_value", var_name);

                let var_read = quote! {
                    #update_name.0.into_owned()
                };

                let var_expression = match networked_field.with.as_ref() {
                    Some(with) => {
                        transform_field_value(var_read, &param_indices, i, with)
                    },
                    None => var_read,
                };

                let update_hook = match &networked_field.updated {
                    Some(updated) => {
                        quote_spanned! { updated.span() =>
                            #updated(self, &#new_name);
                        }
                    },
                    None => proc_macro2::TokenStream::new(),
                };

                quote_spanned! { networked_field.ident.span() =>
                    let #update_name: Option::<networking::variable::ValueUpdate<#networked_type>> =
                        serde::Deserialize::deserialize(&mut deserializer)
                            .expect("Error deserializing networked variable");
                    if let Some(#update_name) = #update_name {
                        let #new_name = #var_expression;
                        #update_hook
                        self.#var_name.set(#new_name);
                    }
                }
            });

            // Build client trait implementation
            quote! {
                impl networking::variable::NetworkedFromServer for #name {
                    type Param = #paramset;

                    fn deserialize<'w, 's>(
                        &mut self,
                        #method_param,
                        data: &[u8],
                    ) {
                        let mut deserializer = networking::variable::StandardDeserializer::with_reader(
                            networking::variable::Buf::reader(data),
                            networking::variable::serializer_options(),
                        );

                        #(#reads)*
                    }

                    fn default_if_missing() -> Option<Self> {
                        Some(Default::default())
                    }

                    fn server_type_id() -> std::any::TypeId {
                        std::any::TypeId::of::<#matching_type>()
                    }

                    fn data_signature() -> u64 {
                        #signature
                    }
                }
            }
        }
    }.into()
}
