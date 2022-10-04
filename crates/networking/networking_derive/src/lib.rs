extern crate proc_macro;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, spanned::Spanned, ItemStruct};

fn parse_networked_field(field: &syn::Field) -> syn::Result<(&syn::Field, &syn::Type)> {
    let update_type = &field.ty;
    let type_path = match update_type {
        syn::Type::Path(p) => p,
        _ => {
            return Err(syn::Error::new(update_type.span(), "Invalid variable type"));
        }
    };
    let mut iterator = type_path.path.segments.iter();
    let segment = match iterator.find(|s| s.ident == "NetworkVar") {
        Some(s) => s,
        None => {
            return Err(syn::Error::new(
                update_type.span(),
                "Synced variable must be of type NetworkVar",
            ));
        }
    };
    let actual_type = match &segment.arguments {
        syn::PathArguments::AngleBracketed(args) => {
            match args.args.first().expect("NetworkVar generic must exist") {
                syn::GenericArgument::Type(t) => t,
                _ => panic!("Expected generic type"),
            }
        }
        _ => panic!("Expected generic type"),
    };
    Ok((field, actual_type))
}

#[proc_macro_derive(Networked, attributes(client, synced, priority))]
pub fn networked_derive(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemStruct);
    if input.generics.lt_token.is_some() {
        return syn::Error::new(
            input.generics.span(),
            "Networking derive is not supported on structs with generics",
        )
        .to_compile_error()
        .into();
    }

    let name = &input.ident;
    let client_attribute = match input.attrs.iter().find(|a| a.path.is_ident("client")) {
        Some(a) => a,
        None => {
            return syn::Error::new(name.span(), "Missing #[client = XXX] attribute")
                .to_compile_error()
                .into()
        }
    };

    let client_struct = match client_attribute.parse_args::<syn::Path>() {
        Ok(p) => p,
        _ => {
            return syn::Error::new(client_attribute.span(), "Invalid client attribute syntax")
                .to_compile_error()
                .into()
        }
    };

    let priority = match input.attrs.iter().find(|a| a.path.is_ident("priority")) {
        Some(a) => match a.parse_args::<syn::LitInt>() {
            Ok(l) => match l.base10_parse::<i16>() {
                Ok(i) => i,
                Err(err) => {
                    return err.to_compile_error().into();
                }
            },
            Err(err) => {
                return err.to_compile_error().into();
            }
        },
        None => 0i16,
    };

    let fields = match input.fields {
        syn::Fields::Named(f) => f,
        _ => {
            return syn::Error::new(input.span(), "Only structs can be networked")
                .to_compile_error()
                .into()
        }
    };

    let synced_fields = match fields
        .named
        .iter()
        .filter(|f| f.attrs.iter().any(|a| a.path.is_ident("synced")))
        .map(parse_networked_field)
        .collect::<syn::Result<Vec<_>>>()
        .map_err(syn::Error::into_compile_error)
    {
        Ok(f) => f,
        Err(e) => return e.into(),
    };

    let writes = synced_fields.iter().map(|(field, actual_type)| {
        let var_name = field.ident.as_ref().unwrap();
        let changed_name = format_ident!("{}_changed", var_name);
        quote! {
            let #changed_name = since_tick
                .map(|t| self.#var_name.has_changed_since(t.into()))
                .unwrap_or(true);
            serde::Serialize::serialize(
                &#changed_name.then(|| {
                    networking::variable::ValueUpdate::<#actual_type>::from(&*(self.#var_name))
                }),
                &mut serializer,
            )
            .unwrap();
        }
    });

    let updates = synced_fields.iter().map(|(field, _)| {
        let var_name = field.ident.as_ref().unwrap();

        quote! {
            self.#var_name.update_state(tick)
        }
    });

    let reads = synced_fields.iter().map(|(field, actual_type)| {
        let var_name = field.ident.as_ref().unwrap();
        let update_name = format_ident!("{}_update", var_name);

        quote! {
            let #update_name =
                Option::<networking::variable::ValueUpdate<#actual_type>>::deserialize(&mut deserializer)
                    .expect("Error deserializing networked variable");
            if let Some(#update_name) = #update_name {
                self.#var_name.set(#update_name.0.into_owned());
            }
        }
    });

    quote! {
        impl networking::variable::NetworkedToClient for #name {
            type Param = ();

            fn receiver_matters() -> bool {
                false
            }

            fn serialize(
                &self,
                _: &(),
                _: Option<networking::ConnectionId>,
                since_tick: Option<std::num::NonZeroU32>,
            ) -> Option<networking::variable::Bytes> {
                let mut writer =
                    networking::variable::BufMut::writer(networking::variable::BytesMut::new());
                let mut serializer = networking::variable::StandardSerializer::new(
                    &mut writer,
                    networking::variable::serializer_options(),
                );

                #(#writes)*

                Some(writer.into_inner().into())
            }

            fn update_state(&mut self, tick: u32) -> bool {
                #(#updates)|*
            }

            fn priority(&self) -> i16 {
                #priority
            }
        }

        impl networking::variable::NetworkedFromServer for #client_struct {
            type Param = ();

            fn deserialize<'w, 's>(
                &mut self,
                _: &<<Self::Param as bevy::ecs::system::SystemParam>::Fetch as bevy::ecs::system::SystemParamFetch<'w, 's>>::Item,
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
        }
    }.into()
}
