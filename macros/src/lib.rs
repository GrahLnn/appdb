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

#[proc_macro_derive(Store, attributes(unique, secure, store, bindref))]
pub fn derive_store(input: TokenStream) -> TokenStream {
    match derive_store_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(Relation, attributes(relation))]
pub fn derive_relation(input: TokenStream) -> TokenStream {
    match derive_relation_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(Bridge)]
pub fn derive_bridge(input: TokenStream) -> TokenStream {
    match derive_bridge_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_store_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_ident = input.ident;
    let vis = input.vis.clone();

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

    let secure_fields = named_fields
        .iter()
        .filter(|field| has_secure_attr(&field.attrs))
        .map(|field| field.ident.clone().expect("named field"))
        .collect::<Vec<_>>();

    let unique_fields = named_fields
        .iter()
        .filter(|field| has_unique_attr(&field.attrs))
        .map(|field| field.ident.clone().expect("named field"))
        .collect::<Vec<_>>();

    if id_fields.len() > 1 {
        return Err(Error::new_spanned(
            struct_ident,
            "Store supports at most one `Id` field for automatic HasId generation",
        ));
    }

    if let Some(invalid_field) = named_fields
        .iter()
        .find(|field| has_secure_attr(&field.attrs) && has_unique_attr(&field.attrs))
    {
        let ident = invalid_field.ident.as_ref().expect("named field");
        return Err(Error::new_spanned(
            ident,
            "#[secure] fields cannot be used as #[unique] lookup keys",
        ));
    }

    let store_ref_fields = named_fields
        .iter()
        .filter_map(|field| match field_bindref_attr(field) {
            Ok(Some(attr)) => Some(parse_store_ref_field(field, attr)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect::<syn::Result<Vec<_>>>()?;

    if let Some(non_store_child) = store_ref_fields
        .iter()
        .find_map(|field| invalid_bindref_leaf_type(&field.kind.original_ty))
    {
        return Err(Error::new_spanned(
            non_store_child,
            BINDREF_BRIDGE_STORE_ONLY,
        ));
    }

    if let Some(invalid_field) = named_fields.iter().find(|field| {
        field_bindref_attr(field).ok().flatten().is_some() && has_unique_attr(&field.attrs)
    }) {
        let ident = invalid_field.ident.as_ref().expect("named field");
        return Err(Error::new_spanned(
            ident,
            "#[bindref] fields cannot be used as #[unique] lookup keys",
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

    let resolve_record_id_impl = if let Some(field) = id_fields.first() {
        quote! {
            #[::async_trait::async_trait]
            impl ::appdb::model::meta::ResolveRecordId for #struct_ident {
                async fn resolve_record_id(&self) -> ::anyhow::Result<::surrealdb::types::RecordId> {
                    Ok(::surrealdb::types::RecordId::new(
                        <Self as ::appdb::model::meta::ModelMeta>::table_name(),
                        self.#field.clone(),
                    ))
                }
            }
        }
    } else {
        quote! {
            #[::async_trait::async_trait]
            impl ::appdb::model::meta::ResolveRecordId for #struct_ident {
                async fn resolve_record_id(&self) -> ::anyhow::Result<::surrealdb::types::RecordId> {
                    ::appdb::repository::Repo::<Self>::find_unique_id_for(self).await
                }
            }
        }
    };

    let unique_schema_impls = unique_fields.iter().map(|field| {
        let field_name = field.to_string();
        let index_name = format!(
            "{}_{}_unique",
            to_snake_case(&struct_ident.to_string()),
            field_name
        );
        let ddl = format!(
            "DEFINE INDEX IF NOT EXISTS {index_name} ON {} FIELDS {field_name} UNIQUE;",
            to_snake_case(&struct_ident.to_string())
        );

        quote! {
            ::inventory::submit! {
                ::appdb::model::schema::SchemaItem {
                    ddl: #ddl,
                }
            }
        }
    });

    let lookup_fields = if unique_fields.is_empty() {
        named_fields
            .iter()
            .filter_map(|field| {
                let ident = field.ident.as_ref()?;
                if ident == "id"
                    || secure_fields.iter().any(|secure| secure == ident)
                    || store_ref_fields
                        .iter()
                        .any(|bindref| bindref.ident == *ident)
                {
                    None
                } else {
                    Some(ident.to_string())
                }
            })
            .collect::<Vec<_>>()
    } else {
        unique_fields
            .iter()
            .map(|field| field.to_string())
            .collect::<Vec<_>>()
    };

    let bindref_field_literals = store_ref_fields
        .iter()
        .map(|field| field.ident.to_string())
        .map(|field| quote! { #field });
    if id_fields.is_empty() && lookup_fields.is_empty() {
        return Err(Error::new_spanned(
            struct_ident,
            "Store requires an `Id` field or at least one non-secure lookup field for automatic record resolution",
        ));
    }
    let lookup_field_literals = lookup_fields.iter().map(|field| quote! { #field });

    let stored_model_impl = if !store_ref_fields.is_empty() {
        quote! {}
    } else if secure_field_count(&named_fields) > 0 {
        quote! {
            impl ::appdb::StoredModel for #struct_ident {
                type Stored = <Self as ::appdb::Sensitive>::Encrypted;

                fn into_stored(self) -> ::anyhow::Result<Self::Stored> {
                    <Self as ::appdb::Sensitive>::encrypt_with_runtime_resolver(&self)
                        .map_err(::anyhow::Error::from)
                }

                fn from_stored(stored: Self::Stored) -> ::anyhow::Result<Self> {
                    <Self as ::appdb::Sensitive>::decrypt_with_runtime_resolver(&stored)
                        .map_err(::anyhow::Error::from)
                }

                fn supports_create_return_id() -> bool {
                    false
                }
            }
        }
    } else {
        quote! {
            impl ::appdb::StoredModel for #struct_ident {
                type Stored = Self;

                fn into_stored(self) -> ::anyhow::Result<Self::Stored> {
                    ::std::result::Result::Ok(self)
                }

                fn from_stored(stored: Self::Stored) -> ::anyhow::Result<Self> {
                    ::std::result::Result::Ok(stored)
                }
            }
        }
    };

    let stored_fields = named_fields.iter().map(|field| {
        let ident = field.ident.clone().expect("named field");
        let ty = stored_field_type(field, &store_ref_fields);
        quote! { #ident: #ty }
    });

    let into_stored_assignments = named_fields.iter().map(|field| {
        let ident = field.ident.clone().expect("named field");
        match store_ref_field_kind(&ident, &store_ref_fields) {
            Some(StoreRefKind { original_ty, .. }) => quote! {
                #ident: <#original_ty as ::appdb::BindrefShape>::persist_bindref_shape(value.#ident).await?
            },
            None => quote! { #ident: value.#ident },
        }
    });

    let from_stored_assignments = named_fields.iter().map(|field| {
        let ident = field.ident.clone().expect("named field");
        match store_ref_field_kind(&ident, &store_ref_fields) {
            Some(StoreRefKind { original_ty, .. }) => quote! {
                #ident: <#original_ty as ::appdb::BindrefShape>::hydrate_bindref_shape(stored.#ident).await?
            },
            None => quote! { #ident: stored.#ident },
        }
    });

    let decode_store_ref_fields = store_ref_fields.iter().map(|field| {
        let ident = field.ident.to_string();
        quote! {
            if let ::std::option::Option::Some(value) = map.get_mut(#ident) {
                ::appdb::rewrite_store_ref_json_value(value);
            }
        }
    });

    let nested_store_impl = if store_ref_fields.is_empty() {
        quote! {
            impl ::appdb::NestedStoreRefs for #struct_ident {
                async fn persist_nested_refs(value: Self) -> ::anyhow::Result<Self::Stored> {
                    <Self as ::appdb::StoredModel>::into_stored(value)
                }

                async fn hydrate_nested_refs(stored: Self::Stored) -> ::anyhow::Result<Self> {
                    <Self as ::appdb::StoredModel>::from_stored(stored)
                }

                fn decode_stored_row(
                    row: ::surrealdb::types::Value,
                ) -> ::anyhow::Result<Self::Stored>
                where
                    Self::Stored: ::serde::de::DeserializeOwned,
                {
                    Ok(::serde_json::from_value(row.into_json_value())?)
                }
            }
        }
    } else {
        let stored_struct_ident = format_ident!("AppdbStored{}", struct_ident);
        quote! {
            #[derive(
                Debug,
                Clone,
                ::serde::Serialize,
                ::serde::Deserialize,
                ::surrealdb::types::SurrealValue,
            )]
            #vis struct #stored_struct_ident {
                #( #stored_fields, )*
            }

            impl ::appdb::StoredModel for #struct_ident {
                type Stored = #stored_struct_ident;

                fn into_stored(self) -> ::anyhow::Result<Self::Stored> {
                    unreachable!("nested store refs require async persist_nested_refs")
                }

                fn from_stored(_stored: Self::Stored) -> ::anyhow::Result<Self> {
                    unreachable!("nested store refs require async hydrate_nested_refs")
                }
            }

            impl ::appdb::NestedStoreRefs for #struct_ident {
                async fn persist_nested_refs(value: Self) -> ::anyhow::Result<Self::Stored> {
                    let value = value;
                    Ok(#stored_struct_ident {
                        #( #into_stored_assignments, )*
                    })
                }

                async fn hydrate_nested_refs(stored: Self::Stored) -> ::anyhow::Result<Self> {
                    Ok(Self {
                        #( #from_stored_assignments, )*
                    })
                }

                fn has_nested_store_refs() -> bool {
                    true
                }

                fn decode_stored_row(
                    row: ::surrealdb::types::Value,
                ) -> ::anyhow::Result<Self::Stored>
                where
                    Self::Stored: ::serde::de::DeserializeOwned,
                {
                    let mut row = row.into_json_value();
                    if let ::serde_json::Value::Object(map) = &mut row {
                        #( #decode_store_ref_fields )*
                    }
                    Ok(::serde_json::from_value(row)?)
                }
            }
        }
    };

    let store_marker_ident = format_ident!("AppdbStoreMarker{}", struct_ident);

    Ok(quote! {
        #[doc(hidden)]
        #vis struct #store_marker_ident;

        impl ::appdb::model::meta::ModelMeta for #struct_ident {
            fn table_name() -> &'static str {
                static TABLE_NAME: ::std::sync::OnceLock<&'static str> = ::std::sync::OnceLock::new();
                TABLE_NAME.get_or_init(|| {
                    let table = ::appdb::model::meta::default_table_name(stringify!(#struct_ident));
                    ::appdb::model::meta::register_table(stringify!(#struct_ident), table)
                })
            }
        }

        impl ::appdb::model::meta::StoreModelMarker for #struct_ident {}
        impl ::appdb::model::meta::StoreModelMarker for #store_marker_ident {}

        impl ::appdb::model::meta::UniqueLookupMeta for #struct_ident {
            fn lookup_fields() -> &'static [&'static str] {
                &[ #( #lookup_field_literals ),* ]
            }

            fn bindref_fields() -> &'static [&'static str] {
                &[ #( #bindref_field_literals ),* ]
            }
        }
        #stored_model_impl
        #nested_store_impl

        #auto_has_id_impl
        #resolve_record_id_impl

        #( #unique_schema_impls )*

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

fn derive_bridge_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let enum_ident = input.ident;

    let variants = match input.data {
        Data::Enum(data) => data.variants,
        _ => {
            return Err(Error::new_spanned(
                enum_ident,
                "Bridge can only be derived for enums",
            ))
        }
    };

    let payloads = variants
        .iter()
        .map(parse_bridge_variant)
        .collect::<syn::Result<Vec<_>>>()?;

    let from_impls = payloads.iter().map(|variant| {
        let variant_ident = &variant.variant_ident;
        let payload_ty = &variant.payload_ty;

        quote! {
            impl ::std::convert::From<#payload_ty> for #enum_ident {
                fn from(value: #payload_ty) -> Self {
                    Self::#variant_ident(value)
                }
            }
        }
    });

    let persist_match_arms = payloads.iter().map(|variant| {
        let variant_ident = &variant.variant_ident;

        quote! {
            Self::#variant_ident(value) => <_ as ::appdb::Bridge>::persist_bindref(value).await,
        }
    });

    let hydrate_match_arms = payloads.iter().map(|variant| {
        let variant_ident = &variant.variant_ident;
        let payload_ty = &variant.payload_ty;

        quote! {
            table if table == <#payload_ty as ::appdb::model::meta::ModelMeta>::table_name() => {
                ::std::result::Result::Ok(Self::#variant_ident(
                    <#payload_ty as ::appdb::Bridge>::hydrate_bindref(id).await?,
                ))
            }
        }
    });

    Ok(quote! {
        #( #from_impls )*

        #[::async_trait::async_trait]
        impl ::appdb::Bridge for #enum_ident {
            async fn persist_bindref(self) -> ::anyhow::Result<::surrealdb::types::RecordId> {
                match self {
                    #( #persist_match_arms )*
                }
            }

            async fn hydrate_bindref(
                id: ::surrealdb::types::RecordId,
            ) -> ::anyhow::Result<Self> {
                match id.table.to_string().as_str() {
                    #( #hydrate_match_arms, )*
                    table => ::anyhow::bail!(
                        "unsupported bindref table `{table}` for enum dispatcher `{}`",
                        ::std::stringify!(#enum_ident)
                    ),
                }
            }
        }
    })
}

#[derive(Clone)]
struct BridgeVariant {
    variant_ident: syn::Ident,
    payload_ty: Type,
}

fn parse_bridge_variant(variant: &syn::Variant) -> syn::Result<BridgeVariant> {
    let payload_ty = match &variant.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            fields.unnamed.first().expect("single field").ty.clone()
        }
        Fields::Unnamed(_) => {
            return Err(Error::new_spanned(
                &variant.ident,
                "Bridge variants must be single-field tuple variants",
            ))
        }
        Fields::Unit => {
            return Err(Error::new_spanned(
                &variant.ident,
                "Bridge does not support unit variants",
            ))
        }
        Fields::Named(_) => {
            return Err(Error::new_spanned(
                &variant.ident,
                "Bridge does not support struct variants",
            ))
        }
    };

    let payload_path = match &payload_ty {
        Type::Path(path) => path,
        _ => {
            return Err(Error::new_spanned(
                &payload_ty,
                "Bridge payload must implement appdb::Bridge",
            ))
        }
    };

    let segment = payload_path.path.segments.last().ok_or_else(|| {
        Error::new_spanned(&payload_ty, "Bridge payload must implement appdb::Bridge")
    })?;

    if !matches!(segment.arguments, PathArguments::None) {
        return Err(Error::new_spanned(
            &payload_ty,
            "Bridge payload must implement appdb::Bridge",
        ));
    }

    Ok(BridgeVariant {
        variant_ident: variant.ident.clone(),
        payload_ty,
    })
}

fn derive_relation_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_ident = input.ident;
    let relation_name = relation_name_override(&input.attrs)?
        .unwrap_or_else(|| to_snake_case(&struct_ident.to_string()));

    match input.data {
        Data::Struct(data) => {
            match data.fields {
                Fields::Unit | Fields::Named(_) => {}
                _ => return Err(Error::new_spanned(
                    struct_ident,
                    "Relation can only be derived for unit structs or structs with named fields",
                )),
            }
        }
        _ => {
            return Err(Error::new_spanned(
                struct_ident,
                "Relation can only be derived for structs",
            ))
        }
    }

    Ok(quote! {
        impl ::appdb::model::relation::RelationMeta for #struct_ident {
            fn relation_name() -> &'static str {
                static REL_NAME: ::std::sync::OnceLock<&'static str> = ::std::sync::OnceLock::new();
                REL_NAME.get_or_init(|| ::appdb::model::relation::register_relation(#relation_name))
            }
        }

        impl #struct_ident {
            pub async fn relate<A, B>(a: &A, b: &B) -> ::anyhow::Result<()>
            where
                A: ::appdb::model::meta::ResolveRecordId + Send + Sync,
                B: ::appdb::model::meta::ResolveRecordId + Send + Sync,
            {
                ::appdb::graph::relate_at(a.resolve_record_id().await?, b.resolve_record_id().await?, <Self as ::appdb::model::relation::RelationMeta>::relation_name()).await
            }

            pub async fn unrelate<A, B>(a: &A, b: &B) -> ::anyhow::Result<()>
            where
                A: ::appdb::model::meta::ResolveRecordId + Send + Sync,
                B: ::appdb::model::meta::ResolveRecordId + Send + Sync,
            {
                ::appdb::graph::unrelate_at(a.resolve_record_id().await?, b.resolve_record_id().await?, <Self as ::appdb::model::relation::RelationMeta>::relation_name()).await
            }

            pub async fn out_ids<A>(a: &A, out_table: &str) -> ::anyhow::Result<::std::vec::Vec<::surrealdb::types::RecordId>>
            where
                A: ::appdb::model::meta::ResolveRecordId + Send + Sync,
            {
                ::appdb::graph::out_ids(a.resolve_record_id().await?, <Self as ::appdb::model::relation::RelationMeta>::relation_name(), out_table).await
            }

            pub async fn in_ids<B>(b: &B, in_table: &str) -> ::anyhow::Result<::std::vec::Vec<::surrealdb::types::RecordId>>
            where
                B: ::appdb::model::meta::ResolveRecordId + Send + Sync,
            {
                ::appdb::graph::in_ids(b.resolve_record_id().await?, <Self as ::appdb::model::relation::RelationMeta>::relation_name(), in_table).await
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
    let mut runtime_encrypt_assignments = Vec::new();
    let mut runtime_decrypt_assignments = Vec::new();
    let mut field_tag_structs = Vec::new();

    for field in named_fields.iter() {
        let ident = field.ident.clone().expect("named field");
        let field_vis = field.vis.clone();
        let secure = has_secure_attr(&field.attrs);

        if secure {
            secure_field_count += 1;
            let secure_kind = secure_kind(field)?;
            let encrypted_ty = secure_kind.encrypted_type();
            let field_tag_ident = format_ident!(
                "AppdbSensitiveFieldTag{}{}",
                struct_ident,
                to_pascal_case(&ident.to_string())
            );
            let field_tag_literal = ident.to_string();
            let encrypt_expr = secure_kind.encrypt_with_context_expr(&ident);
            let decrypt_expr = secure_kind.decrypt_with_context_expr(&ident);
            let runtime_encrypt_expr =
                secure_kind.encrypt_with_runtime_expr(&ident, &field_tag_ident);
            let runtime_decrypt_expr =
                secure_kind.decrypt_with_runtime_expr(&ident, &field_tag_ident);
            encrypted_fields.push(quote! { #field_vis #ident: #encrypted_ty });
            encrypt_assignments.push(quote! { #ident: #encrypt_expr });
            decrypt_assignments.push(quote! { #ident: #decrypt_expr });
            runtime_encrypt_assignments.push(quote! { #ident: #runtime_encrypt_expr });
            runtime_decrypt_assignments.push(quote! { #ident: #runtime_decrypt_expr });
            field_tag_structs.push(quote! {
                #[doc(hidden)]
                #vis struct #field_tag_ident;

                impl ::appdb::crypto::SensitiveFieldTag for #field_tag_ident {
                    fn model_tag() -> &'static str {
                        <#struct_ident as ::appdb::crypto::SensitiveModelTag>::model_tag()
                    }

                    fn field_tag() -> &'static str {
                        #field_tag_literal
                    }
                }
            });
        } else {
            let ty = field.ty.clone();
            encrypted_fields.push(quote! { #field_vis #ident: #ty });
            encrypt_assignments.push(quote! { #ident: self.#ident.clone() });
            decrypt_assignments.push(quote! { #ident: encrypted.#ident.clone() });
            runtime_encrypt_assignments.push(quote! { #ident: self.#ident.clone() });
            runtime_decrypt_assignments.push(quote! { #ident: encrypted.#ident.clone() });
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

        impl ::appdb::crypto::SensitiveModelTag for #struct_ident {
            fn model_tag() -> &'static str {
                ::std::concat!(::std::module_path!(), "::", ::std::stringify!(#struct_ident))
            }
        }

        #( #field_tag_structs )*

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

            fn encrypt_with_runtime_resolver(
                &self,
            ) -> ::std::result::Result<Self::Encrypted, ::appdb::crypto::CryptoError> {
                ::std::result::Result::Ok(#encrypted_ident {
                    #( #runtime_encrypt_assignments, )*
                })
            }

            fn decrypt_with_runtime_resolver(
                encrypted: &Self::Encrypted,
            ) -> ::std::result::Result<Self, ::appdb::crypto::CryptoError> {
                ::std::result::Result::Ok(Self {
                    #( #runtime_decrypt_assignments, )*
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

fn has_unique_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("unique"))
}

fn field_bindref_attr(field: &Field) -> syn::Result<Option<&Attribute>> {
    let mut bindref_attr = None;

    for attr in &field.attrs {
        if !(attr.path().is_ident("bindref") || attr.path().is_ident("store")) {
            continue;
        }

        if bindref_attr.is_some() {
            return Err(Error::new_spanned(
                attr,
                "duplicate nested-ref attribute is not supported",
            ));
        }

        bindref_attr = Some(attr);
    }

    Ok(bindref_attr)
}

fn validate_store_ref_field(field: &Field, attr: &Attribute) -> syn::Result<Type> {
    if attr.path().is_ident("bindref") {
        return bindref_leaf_type(&field.ty)
            .ok_or_else(|| Error::new_spanned(&field.ty, BINDREF_ACCEPTED_SHAPES));
    }

    if attr.path().is_ident("store") {
        let mut saw_ref = false;

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("ref") {
                if saw_ref {
                    return Err(meta.error("duplicate `ref` store option is not supported"));
                }
                saw_ref = true;
                Ok(())
            } else {
                Err(meta.error(STORE_REF_LEGACY_REJECTED))
            }
        })?;

        if saw_ref {
            return Err(Error::new_spanned(attr, STORE_REF_LEGACY_REJECTED));
        }

        return Err(Error::new_spanned(attr, STORE_REF_UNSUPPORTED_ATTRIBUTE));
    }

    Err(Error::new_spanned(attr, STORE_REF_UNSUPPORTED_ATTRIBUTE))
}

const BINDREF_ACCEPTED_SHAPES: &str =
    "#[bindref] supports recursive Option<_> / Vec<_> shapes whose leaf type implements appdb::Bridge";

const BINDREF_BRIDGE_STORE_ONLY: &str =
    "#[bindref] leaf types must derive Store or #[derive(Bridge)] dispatcher enums";

const STORE_REF_LEGACY_REJECTED: &str =
    "#[store(ref)] is no longer supported; use #[bindref] for explicit nested references";

const STORE_REF_UNSUPPORTED_ATTRIBUTE: &str = "unsupported nested-ref attribute; use #[bindref]";

#[derive(Clone)]
struct StoreRefField {
    ident: syn::Ident,
    kind: StoreRefKind,
}

#[derive(Clone)]
struct StoreRefKind {
    original_ty: Type,
    stored_ty: Type,
}

fn parse_store_ref_field(field: &Field, attr: &Attribute) -> syn::Result<StoreRefField> {
    validate_store_ref_field(field, attr)?;
    let ident = field.ident.clone().expect("named field");

    let kind = StoreRefKind {
        original_ty: field.ty.clone(),
        stored_ty: bindref_stored_type(&field.ty)
            .ok_or_else(|| Error::new_spanned(&field.ty, BINDREF_ACCEPTED_SHAPES))?,
    };

    Ok(StoreRefField { ident, kind })
}

fn store_ref_field_kind<'a>(
    ident: &syn::Ident,
    fields: &'a [StoreRefField],
) -> Option<&'a StoreRefKind> {
    fields
        .iter()
        .find(|field| field.ident == *ident)
        .map(|field| &field.kind)
}

fn stored_field_type(field: &Field, store_ref_fields: &[StoreRefField]) -> Type {
    let ident = field.ident.as_ref().expect("named field");
    match store_ref_field_kind(ident, store_ref_fields) {
        Some(StoreRefKind { stored_ty, .. }) => stored_ty.clone(),
        None => field.ty.clone(),
    }
}

fn bindref_stored_type(ty: &Type) -> Option<Type> {
    if let Some(inner) = option_inner_type(ty) {
        let inner = bindref_stored_type(inner)?;
        return Some(syn::parse_quote!(::std::option::Option<#inner>));
    }

    if let Some(inner) = vec_inner_type(ty) {
        let inner = bindref_stored_type(inner)?;
        return Some(syn::parse_quote!(::std::vec::Vec<#inner>));
    }

    direct_store_child_type(ty)
        .cloned()
        .map(|_| syn::parse_quote!(::surrealdb::types::RecordId))
}

fn bindref_leaf_type(ty: &Type) -> Option<Type> {
    if let Some(inner) = option_inner_type(ty) {
        return bindref_leaf_type(inner);
    }

    if let Some(inner) = vec_inner_type(ty) {
        return bindref_leaf_type(inner);
    }

    direct_store_child_type(ty).cloned().map(Type::Path)
}

fn invalid_bindref_leaf_type(ty: &Type) -> Option<Type> {
    let leaf = bindref_leaf_type(ty)?;
    match &leaf {
        Type::Path(type_path) => {
            let segment = type_path.path.segments.last()?;
            if matches!(segment.arguments, PathArguments::None) {
                None
            } else {
                Some(leaf)
            }
        }
        _ => Some(leaf),
    }
}

fn direct_store_child_type(ty: &Type) -> Option<&TypePath> {
    let Type::Path(type_path) = ty else {
        return None;
    };

    let segment = type_path.path.segments.last()?;
    if !matches!(segment.arguments, PathArguments::None) {
        return None;
    }

    if is_id_type(ty) || is_string_type(ty) || is_common_non_store_leaf_type(ty) {
        return None;
    }

    Some(type_path)
}

fn is_common_non_store_leaf_type(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Path(TypePath { path, .. })
            if path.is_ident("bool")
                || path.is_ident("u8")
                || path.is_ident("u16")
                || path.is_ident("u32")
                || path.is_ident("u64")
                || path.is_ident("u128")
                || path.is_ident("usize")
                || path.is_ident("i8")
                || path.is_ident("i16")
                || path.is_ident("i32")
                || path.is_ident("i64")
                || path.is_ident("i128")
                || path.is_ident("isize")
                || path.is_ident("f32")
                || path.is_ident("f64")
                || path.is_ident("char")
    )
}

fn secure_field_count(fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>) -> usize {
    fields
        .iter()
        .filter(|field| has_secure_attr(&field.attrs))
        .count()
}

fn relation_name_override(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if !attr.path().is_ident("relation") {
            continue;
        }

        let mut name = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let value = meta.value()?;
                let literal: syn::LitStr = value.parse()?;
                name = Some(literal.value());
                Ok(())
            } else {
                Err(meta.error("unsupported relation attribute"))
            }
        })?;
        return Ok(name);
    }

    Ok(None)
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

    fn encrypt_with_context_expr(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => {
                quote! { ::appdb::crypto::encrypt_string(&self.#ident, context)? }
            }
            SecureKind::OptionString => {
                quote! { ::appdb::crypto::encrypt_optional_string(&self.#ident, context)? }
            }
        }
    }

    fn decrypt_with_context_expr(&self, ident: &syn::Ident) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => {
                quote! { ::appdb::crypto::decrypt_string(&encrypted.#ident, context)? }
            }
            SecureKind::OptionString => {
                quote! { ::appdb::crypto::decrypt_optional_string(&encrypted.#ident, context)? }
            }
        }
    }

    fn encrypt_with_runtime_expr(
        &self,
        ident: &syn::Ident,
        field_tag_ident: &syn::Ident,
    ) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => {
                quote! {{
                    let context = ::appdb::crypto::resolve_crypto_context_for::<#field_tag_ident>()?;
                    ::appdb::crypto::encrypt_string(&self.#ident, context.as_ref())?
                }}
            }
            SecureKind::OptionString => {
                quote! {{
                    let context = ::appdb::crypto::resolve_crypto_context_for::<#field_tag_ident>()?;
                    ::appdb::crypto::encrypt_optional_string(&self.#ident, context.as_ref())?
                }}
            }
        }
    }

    fn decrypt_with_runtime_expr(
        &self,
        ident: &syn::Ident,
        field_tag_ident: &syn::Ident,
    ) -> proc_macro2::TokenStream {
        match self {
            SecureKind::String => {
                quote! {{
                    let context = ::appdb::crypto::resolve_crypto_context_for::<#field_tag_ident>()?;
                    ::appdb::crypto::decrypt_string(&encrypted.#ident, context.as_ref())?
                }}
            }
            SecureKind::OptionString => {
                quote! {{
                    let context = ::appdb::crypto::resolve_crypto_context_for::<#field_tag_ident>()?;
                    ::appdb::crypto::decrypt_optional_string(&encrypted.#ident, context.as_ref())?
                }}
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

fn vec_inner_type(ty: &Type) -> Option<&Type> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return None;
    };
    let segment = path.segments.last()?;
    if segment.ident != "Vec" {
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

fn to_snake_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    let mut prev_is_lower_or_digit = false;

    for ch in input.chars() {
        if ch.is_ascii_uppercase() {
            if prev_is_lower_or_digit {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_is_lower_or_digit = false;
        } else {
            out.push(ch);
            prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }

    out
}

fn to_pascal_case(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut uppercase_next = true;

    for ch in input.chars() {
        if ch == '_' || ch == '-' {
            uppercase_next = true;
            continue;
        }

        if uppercase_next {
            out.push(ch.to_ascii_uppercase());
            uppercase_next = false;
        } else {
            out.push(ch);
        }
    }

    out
}
