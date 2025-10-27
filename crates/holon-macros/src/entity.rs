use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Meta};

use crate::attr_parser;

pub fn derive_entity_impl(input: DeriveInput) -> TokenStream {
    let name = &input.ident;
    let entity_attr = attr_parser::extract_entity_attribute(&input.attrs);
    let entity_name = &entity_attr.name;
    let short_name_expr = match &entity_attr.short_name {
        Some(sn) => quote! { Some(#sn) },
        None => quote! { None },
    };

    // Entity types come from holon_api, or from 'crate' when used within holon-api itself
    let api_path = match &entity_attr.api_crate {
        Some(crate_name) if crate_name == "crate" => quote! { crate },
        Some(crate_name) => {
            let ident = syn::Ident::new(crate_name, proc_macro2::Span::call_site());
            quote! { #ident }
        }
        None => quote! { holon_api },
    };

    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => &fields.named,
            _ => panic!("Entity can only be derived for structs with named fields"),
        },
        _ => panic!("Entity can only be derived for structs"),
    };

    let mut primary_key_field = None;
    let mut field_schemas = Vec::new();
    let lens_definitions: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut to_entity_fields = Vec::new();
    let mut from_entity_fields = Vec::new();
    let mut schema_fields = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_name_str = field_name.to_string();
        let field_type = &field.ty;

        let is_primary_key = field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("primary_key"));

        let is_indexed = field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("indexed"));

        let is_jsonb = field.attrs.iter().any(|attr| attr.path().is_ident("jsonb"));

        let default_value: Option<String> = field.attrs.iter().find_map(|attr| {
            if attr.path().is_ident("default_value") {
                if let syn::Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &nv.value
                    {
                        return Some(s.value());
                    }
                }
            }
            None
        });

        let _skip_lens = field.attrs.iter().any(|attr| {
            if attr.path().is_ident("lens")
                && let Meta::List(meta_list) = &attr.meta
            {
                let tokens_str = meta_list.tokens.to_string();
                return tokens_str == "skip";
            }
            false
        });

        let skip_serialization = field.attrs.iter().any(|attr| {
            if attr.path().is_ident("serde")
                && let Meta::List(meta_list) = &attr.meta
            {
                let tokens_str = meta_list.tokens.to_string();
                return tokens_str.contains("skip");
            }
            false
        });

        let reference_attr = attr_parser::extract_reference_attribute(&field.attrs);
        let reference_entity = reference_attr.as_ref().map(|r| r.entity_name.clone());

        if is_primary_key {
            primary_key_field = Some(field_name_str.clone());
        }

        let field_type_enum = if let Some(ref ref_entity) = reference_entity {
            quote! { #api_path::FieldType::Reference(#ref_entity.to_string()) }
        } else {
            type_to_field_type(field_type, &api_path)
        };

        let is_required = !is_option_type(field_type);

        let edge_name_expr = match reference_attr.as_ref().and_then(|r| r.edge_name.as_ref()) {
            Some(edge) => quote! { Some(#edge.to_string()) },
            None => quote! { None },
        };

        field_schemas.push(quote! {
            #api_path::EntityFieldSchema {
                name: #field_name_str.to_string(),
                field_type: #field_type_enum,
                required: #is_required,
                indexed: #is_indexed,
                edge_name: #edge_name_expr,
            }
        });

        // Lenses are currently disabled
        let _ = &lens_definitions; // suppress unused warning

        if !skip_serialization {
            let sql_type = rust_type_to_sql_type(field_type);
            let nullable = is_option_type(field_type);

            let mut field_schema_builder = quote! {
                #api_path::FieldSchema::new(#field_name_str, #sql_type)
            };

            if is_primary_key {
                field_schema_builder = quote! { #field_schema_builder.primary_key() };
            }

            if is_indexed {
                field_schema_builder = quote! { #field_schema_builder.indexed() };
            }

            if nullable {
                field_schema_builder = quote! { #field_schema_builder.nullable() };
            }

            if is_jsonb {
                field_schema_builder = quote! { #field_schema_builder.jsonb() };
            }

            if let Some(ref dv) = default_value {
                field_schema_builder = quote! { #field_schema_builder.default_value(#dv) };
            }

            schema_fields.push(field_schema_builder);
        }

        if !skip_serialization {
            to_entity_fields.push(quote! {
                entity.set(#field_name_str, self.#field_name.clone())
            });

            let from_entity_conversion = if is_option_type(field_type) {
                quote! {
                    #field_name: entity.get(#field_name_str).and_then(|v| v.clone().try_into().ok())
                }
            } else {
                quote! {
                    #field_name: entity.get(#field_name_str)
                        .and_then(|v| v.clone().try_into().ok())
                        .ok_or_else(|| format!("Missing or invalid field: {}", #field_name_str))?
                }
            };

            from_entity_fields.push(from_entity_conversion);
        } else {
            let default_value = if is_option_type(field_type) {
                quote! { #field_name: None }
            } else if is_vec_type(field_type) {
                quote! { #field_name: Vec::new() }
            } else {
                quote! { #field_name: Default::default() }
            };
            from_entity_fields.push(default_value);
        }
    }

    let primary_key = primary_key_field.unwrap_or_else(|| "id".to_string());

    let graph_label_expr = match &entity_attr.graph_label {
        Some(label) => quote! { Some(#label.to_string()) },
        None => quote! { None },
    };

    let expanded = quote! {
        impl #name {
            /// flutter_rust_bridge:ignore
            pub fn entity_schema() -> #api_path::EntitySchema {
                #api_path::EntitySchema {
                    name: #entity_name.to_string(),
                    primary_key: #primary_key.to_string(),
                    fields: vec![
                        #(#field_schemas),*
                    ],
                    graph_label: #graph_label_expr,
                }
            }

            /// Returns the short name for this entity type (e.g., "task" for "todoist_task")
            /// Used for generating entity-typed parameters like "task_id"
            /// flutter_rust_bridge:ignore
            pub fn short_name() -> Option<&'static str> {
                #short_name_expr
            }
        }

        #(#lens_definitions)*

        impl #api_path::entity::IntoEntity for #name {
            /// flutter_rust_bridge:ignore
            fn to_entity(&self) -> #api_path::DynamicEntity {
                let mut entity = #api_path::DynamicEntity::new(#entity_name);
                #(#to_entity_fields;)*
                entity
            }
        }

        impl #api_path::entity::TryFromEntity for #name {
            /// flutter_rust_bridge:ignore
            fn from_entity(entity: #api_path::DynamicEntity) -> #api_path::entity::Result<Self> {
                Ok(Self {
                    #(#from_entity_fields),*
                })
            }
        }

        impl #api_path::HasSchema for #name {
            /// flutter_rust_bridge:ignore
            fn schema() -> #api_path::Schema {
                #api_path::Schema::new(
                    #entity_name,
                    vec![
                        #(#schema_fields),*
                    ]
                )
            }
        }
    };

    expanded
}

pub(crate) fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident == "Option";
    }
    false
}

pub(crate) fn is_vec_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
    {
        return segment.ident == "Vec";
    }
    false
}

pub(crate) fn type_to_field_type(
    ty: &syn::Type,
    api_path: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let type_str = quote! { #ty }.to_string();

    let inner_type = if type_str.starts_with("Option <") {
        type_str
            .trim_start_matches("Option <")
            .trim_end_matches('>')
            .trim()
    } else {
        type_str.as_str()
    };

    match inner_type {
        "String" => quote! { #api_path::FieldType::String },
        "i64" | "i32" | "u64" | "u32" | "usize" => {
            quote! { #api_path::FieldType::Integer }
        }
        "bool" => quote! { #api_path::FieldType::Boolean },
        t if t.contains("DateTime") => quote! { #api_path::FieldType::DateTime },
        _ => quote! { #api_path::FieldType::Json },
    }
}

pub(crate) fn rust_type_to_sql_type(ty: &syn::Type) -> String {
    let type_str = quote! { #ty }.to_string();

    let inner_type = if type_str.starts_with("Option <") {
        type_str
            .trim_start_matches("Option <")
            .trim_end_matches('>')
            .trim()
    } else {
        type_str.as_str()
    };

    match inner_type {
        "String" => "TEXT".to_string(),
        "i64" | "i32" | "u64" | "u32" | "usize" => "INTEGER".to_string(),
        "bool" => "INTEGER".to_string(),
        "f64" | "f32" => "REAL".to_string(),
        t if t.contains("DateTime") => "TEXT".to_string(),
        _ => "TEXT".to_string(),
    }
}
