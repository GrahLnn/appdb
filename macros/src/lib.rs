use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    parse_macro_input, Attribute, Data, DeriveInput, Error, Field, Fields, GenericArgument,
    PathArguments, Type, TypePath,
};

#[proc_macro_derive(Sensitive, attributes(secure))]
pub fn derive_sensitive(input: TokenStream) -> TokenStream {
    match derive_sensitive_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(Store)]
pub fn derive_store(input: TokenStream) -> TokenStream {
    match derive_store_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_store_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_ident = input.ident;

    let named_fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            _ => {
                return Err(Error::new_spanned(
                    struct_ident,
                    "Store can only be derived for structs with named fields",
                ))
            }
        },
        _ => {
            return Err(Error::new_spanned(
                struct_ident,
                "Store can only be derived for structs",
            ))
        }
    };

    let id_fields = named_fields
        .iter()
        .filter(|field| is_id_type(&field.ty))
        .map(|field| field.ident.clone().expect("named field"))
        .collect::<Vec<_>>();

    if id_fields.len() > 1 {
        return Err(Error::new_spanned(
            struct_ident,
            "Store supports at most one `Id` field for automatic HasId generation",
        ));
    }

    let auto_has_id_impl = id_fields.first().map(|field| {
        quote! {
            impl ::appdb::model::meta::HasId for #struct_ident {
                fn id(&self) -> ::surrealdb::types::RecordId {
                    ::surrealdb::types::RecordId::new(
                        <Self as ::appdb::model::meta::ModelMeta>::table_name(),
                        self.#field.clone(),
                    )
                }
            }
        }
    });

    Ok(quote! {
        impl ::appdb::model::meta::ModelMeta for #struct_ident {
            fn table_name() -> &'static str {
                static TABLE_NAME: ::std::sync::OnceLock<&'static str> = ::std::sync::OnceLock::new();
                TABLE_NAME.get_or_init(|| {
                    let table = ::appdb::model::meta::default_table_name(stringify!(#struct_ident));
                    ::appdb::model::meta::register_table(stringify!(#struct_ident), table)
                })
            }
        }

        #auto_has_id_impl

        impl ::appdb::repository::Crud for #struct_ident {}

        impl #struct_ident {
            pub async fn get<T>(id: T) -> ::anyhow::Result<Self>
            where
                ::surrealdb::types::RecordIdKey: From<T>,
                T: Send,
            {
                ::appdb::repository::Repo::<Self>::get(id).await
            }

            pub async fn list() -> ::anyhow::Result<::std::vec::Vec<Self>> {
                ::appdb::repository::Repo::<Self>::list().await
            }

            pub async fn list_limit(count: i64) -> ::anyhow::Result<::std::vec::Vec<Self>> {
                ::appdb::repository::Repo::<Self>::list_limit(count).await
            }

            pub async fn delete_all() -> ::anyhow::Result<()> {
                ::appdb::repository::Repo::<Self>::delete_all().await
            }

            pub async fn find_one_id(
                k: &str,
                v: &str,
            ) -> ::anyhow::Result<::surrealdb::types::RecordId> {
                ::appdb::repository::Repo::<Self>::find_one_id(k, v).await
            }

            pub async fn list_record_ids() -> ::anyhow::Result<::std::vec::Vec<::surrealdb::types::RecordId>> {
                ::appdb::repository::Repo::<Self>::list_record_ids().await
            }

            pub async fn create_at(
                id: ::surrealdb::types::RecordId,
                data: Self,
            ) -> ::anyhow::Result<Self> {
                ::appdb::repository::Repo::<Self>::create_at(id, data).await
            }

            pub async fn upsert_at(
                id: ::surrealdb::types::RecordId,
                data: Self,
            ) -> ::anyhow::Result<Self> {
                ::appdb::repository::Repo::<Self>::upsert_at(id, data).await
            }

            pub async fn update_at(
                self,
                id: ::surrealdb::types::RecordId,
            ) -> ::anyhow::Result<Self> {
                ::appdb::repository::Repo::<Self>::update_at(id, self).await
            }

            pub async fn delete<T>(id: T) -> ::anyhow::Result<()>
            where
                ::surrealdb::types::RecordIdKey: From<T>,
                T: Send,
            {
                ::appdb::repository::Repo::<Self>::delete(id).await
            }
        }
    })
}

fn derive_sensitive_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_ident = input.ident;
    let encrypted_ident = format_ident!("Encrypted{}", struct_ident);
    let vis = input.vis;

    let named_fields = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Named(fields) => fields.named,
            _ => {
                return Err(Error::new_spanned(
                    struct_ident,
                    "Sensitive can only be derived for structs with named fields",
                ))
            }
        },
        _ => {
            return Err(Error::new_spanned(
                struct_ident,
                "Sensitive can only be derived for structs",
            ))
        }
    };

    let mut secure_field_count = 0usize;
    let mut encrypted_fields = Vec::new();
    let mut encrypt_assignments = Vec::new();
    let mut decrypt_assignments = Vec::new();

    for field in named_fields.iter() {
        let ident = field.ident.clone().expect("named field");
        let field_vis = field.vis.clone();
        let secure = has_secure_attr(&field.attrs);

        if secure {
            secure_field_count += 1;
            let secure_kind = secure_kind(field)?;
            let encrypted_ty = secure_kind.encrypted_type();
            let encrypt_expr = secure_kind.encrypt_expr(&ident);
            let decrypt_expr = secure_kind.decrypt_expr(&ident);
            encrypted_fields.push(quote! { #field_vis #ident: #encrypted_ty });
            encrypt_assignments.push(quote! { #ident: #encrypt_expr });
            decrypt_assignments.push(quote! { #ident: #decrypt_expr });
        } else {
            let ty = field.ty.clone();
            encrypted_fields.push(quote! { #field_vis #ident: #ty });
            encrypt_assignments.push(quote! { #ident: self.#ident.clone() });
            decrypt_assignments.push(quote! { #ident: encrypted.#ident.clone() });
        }
    }

    if secure_field_count == 0 {
        return Err(Error::new_spanned(
            struct_ident,
            "Sensitive requires at least one #[secure] field",
        ));
    }

    Ok(quote! {
        #[derive(
            Debug,
            Clone,
            ::serde::Serialize,
            ::serde::Deserialize,
            ::surrealdb::types::SurrealValue,
        )]
        #vis struct #encrypted_ident {
            #( #encrypted_fields, )*
        }

        impl ::appdb::Sensitive for #struct_ident {
            type Encrypted = #encrypted_ident;

            fn encrypt(
                &self,
                context: &::appdb::crypto::CryptoContext,
            ) -> ::std::result::Result<Self::Encrypted, ::appdb::crypto::CryptoError> {
                ::std::result::Result::Ok(#encrypted_ident {
                    #( #encrypt_assignments, )*
                })
            }

            fn decrypt(
                encrypted: &Self::Encrypted,
                context: &::appdb::crypto::CryptoContext,
            ) -> ::std::result::Result<Self, ::appdb::crypto::CryptoError> {
                ::std::result::Result::Ok(Self {
                    #( #decrypt_assignments, )*
                })
            }
        }

        impl #struct_ident {
            pub fn encrypt(
                &self,
                context: &::appdb::crypto::CryptoContext,
            ) -> ::std::result::Result<#encrypted_ident, ::appdb::crypto::CryptoError> {
                <Self as ::appdb::Sensitive>::encrypt(self, context)
            }
        }

        impl #encrypted_ident {
            pub fn decrypt(
                &self,
                context: &::appdb::crypto::CryptoContext,
            ) -> ::std::result::Result<#struct_ident, ::appdb::crypto::CryptoError> {
                <#struct_ident as ::appdb::Sensitive>::decrypt(self, context)
            }
        }
    })
}

fn has_secure_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("secure"))
}

enum SecureKind {
    String,
    OptionString,
}

impl SecureKind {
    fn encrypted_type(&self) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => quote! { ::std::vec::Vec<u8> },
            SecureKind::OptionString => quote! { ::std::option::Option<::std::vec::Vec<u8>> },
        }
    }

    fn encrypt_expr(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => {
                quote! { ::appdb::crypto::encrypt_string(&self.#ident, context)? }
            }
            SecureKind::OptionString => {
                quote! { ::appdb::crypto::encrypt_optional_string(&self.#ident, context)? }
            }
        }
    }

    fn decrypt_expr(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => {
                quote! { ::appdb::crypto::decrypt_string(&encrypted.#ident, context)? }
            }
            SecureKind::OptionString => {
                quote! { ::appdb::crypto::decrypt_optional_string(&encrypted.#ident, context)? }
            }
        }
    }
}

fn secure_kind(field: &Field) -> syn::Result<SecureKind> {
    if is_string_type(&field.ty) {
        return Ok(SecureKind::String);
    }

    if let Some(inner) = option_inner_type(&field.ty) {
        if is_string_type(inner) {
            return Ok(SecureKind::OptionString);
        }
    }

    Err(Error::new_spanned(
        &field.ty,
        "#[secure] currently supports only String and Option<String>",
    ))
}

fn is_string_type(ty: &Type) -> bool {
    match ty {
        Type::Path(TypePath { path, .. }) => path.is_ident("String"),
        _ => false,
    }
}

fn is_id_type(ty: &Type) -> bool {
    match ty {
        Type::Path(TypePath { path, .. }) => path.segments.last().is_some_and(|segment| {
            let ident = segment.ident.to_string();
            ident == "Id"
        }),
        _ => false,
    }
}

fn option_inner_type(ty: &Type) -> Option<&Type> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return None;
    };
    let segment = path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    let GenericArgument::Type(inner) = args.args.first()? else {
        return None;
    };
    Some(inner)
}
