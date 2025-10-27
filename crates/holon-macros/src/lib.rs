use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{DeriveInput, FnArg, ItemFn, ItemTrait, Meta, Pat, Type, parse_macro_input};

pub(crate) mod attr_parser;
mod builder_registry;
mod entity;
mod operations_trait;
mod widget_builder;

#[proc_macro_derive(
    Entity,
    attributes(entity, primary_key, indexed, reference, lens, jsonb)
)]
pub fn derive_entity(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    TokenStream::from(entity::derive_entity_impl(input))
}

/// Generate operation descriptors for all async methods in a trait
///
/// This macro generates:
/// - One function `fn OPERATION_NAME_OP() -> OperationDescriptor` per async method
/// - One function `fn TRAIT_NAME_operations() -> Vec<OperationDescriptor>` returning all operations
/// - A module `__operations_trait_name` (snake_case) containing all operations
///
/// Usage:
/// ```rust
/// #[operations_trait]
/// #[async_trait]
/// pub trait CrudOperations<T>: Send + Sync {
///     /// Set single field
///     async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<()>;
/// }
/// ```
///
/// The generated operations can be accessed via:
/// ```rust
/// use crate::core::datasource::mutable_data_source_operations;
/// let ops = mutable_data_source_operations();
/// ```
#[proc_macro_attribute]
pub fn operations_trait(attr: TokenStream, item: TokenStream) -> TokenStream {
    let trait_def = parse_macro_input!(item as ItemTrait);
    let attr_str = attr.to_string();

    // Delegate to the module implementation which has full support for:
    // - enum_from annotations
    // - OperationResult return type
    // - _with_resolver function generation
    TokenStream::from(operations_trait::operations_trait_impl(
        &attr_str, trait_def,
    ))
}

/// Extract doc comments from attributes
fn extract_doc_comments(attrs: &[syn::Attribute]) -> String {
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
            // Handle both NameValue (/// doc) and List (/// doc) formats
            match &attr.meta {
                Meta::NameValue(meta) => {
                    if let syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) = &meta.value
                    {
                        let doc = s.value();
                        let cleaned = doc.trim();
                        if !cleaned.is_empty() {
                            docs.push(cleaned.to_string());
                        }
                    }
                }
                Meta::List(meta_list) => {
                    // Try to parse as a string literal
                    let tokens = &meta_list.tokens;
                    let token_str = quote! { #tokens }.to_string();
                    // Remove quotes if present
                    let cleaned = token_str
                        .strip_prefix('"')
                        .and_then(|s| s.strip_suffix('"'))
                        .unwrap_or(&token_str)
                        .trim();
                    if !cleaned.is_empty() {
                        docs.push(cleaned.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    docs.join(" ")
}

fn extract_affected_fields(attrs: &[syn::Attribute]) -> Vec<String> {
    attr_parser::extract_affected_fields(attrs)
}

/// Extract parameter name from pattern
fn extract_param_name(pat: &Pat) -> String {
    match pat {
        Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
        Pat::Wild(_) => "_".to_string(),
        _ => {
            // Fallback: try to stringify the pattern
            quote! { #pat }.to_string()
        }
    }
}

/// Infer type string and required flag from Rust type
fn infer_type(ty: &Type) -> (String, bool) {
    let type_str = quote! { #ty }.to_string();
    let cleaned = type_str.replace(" ", "");

    // Check if it's an Option type
    if cleaned.starts_with("Option<") {
        let inner = cleaned
            .strip_prefix("Option<")
            .and_then(|s| s.strip_suffix(">"))
            .unwrap_or(&cleaned);
        let inner_type = infer_type_string(inner);
        return (inner_type, false);
    }

    // Check for reference types (strip & but don't affect required flag)
    let inner = if cleaned.starts_with("&") {
        cleaned.strip_prefix("&").unwrap_or(&cleaned)
    } else {
        cleaned.as_str()
    };

    let type_str = infer_type_string(inner);
    (type_str, true)
}

/// Infer type string from cleaned type name
fn infer_type_string(type_str: &str) -> String {
    // Remove lifetime parameters
    let without_lifetime = type_str.split('<').next().unwrap_or(type_str);

    match without_lifetime {
        "str" => "String".to_string(),
        "String" => "String".to_string(),
        "i64" => "i64".to_string(),
        "i32" => "i32".to_string(),
        "u64" => "u64".to_string(),
        "u32" => "u32".to_string(),
        "usize" => "usize".to_string(),
        "bool" => "bool".to_string(),
        "f64" => "f64".to_string(),
        "f32" => "f32".to_string(),
        s if s.contains("HashMap") => "HashMap".to_string(),
        s if s.contains("Vec") => "Vec".to_string(),
        s if s.contains("DateTime") => "DateTime".to_string(),
        s if s.contains("Value") => "Value".to_string(),
        _ => type_str.to_string(),
    }
}

/// Pass-through attribute for #[affects(...)] - allows Rust to accept the attribute
/// The actual parsing is done by extract_affected_fields() in the operations_trait macro.
#[proc_macro_attribute]
pub fn affects(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through unchanged - this just allows Rust to accept the attribute
    item
}

/// Pass-through attribute for #[triggered_by(...)] - allows Rust to accept the attribute
/// The actual parsing is done by extract_param_mappings() in the operations_trait macro.
///
/// This attribute declares that an operation is triggered by the availability of a
/// contextual parameter. When that parameter is present (e.g., from a gesture like
/// drag-drop), only operations that declare they're triggered by it will be considered.
///
/// Usage:
/// ```rust
/// // Transform case: tree_position provides parent_id and after_block_id
/// #[triggered_by(availability_of = "tree_position", providing = ["parent_id", "after_block_id"])]
/// async fn move_block(&self, id: &str, parent_id: &str, after_block_id: Option<&str>) -> Result<()>
///
/// // Identity case: completed triggers and provides itself
/// #[triggered_by(availability_of = "completed")]
/// async fn set_completion(&self, id: &str, completed: bool) -> Result<()>
/// ```
#[proc_macro_attribute]
pub fn triggered_by(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through unchanged - this just allows Rust to accept the attribute
    item
}

/// Pass-through attribute for #[enum_from(...)] - allows Rust to accept the attribute
/// The actual parsing is done by the operations_trait macro.
///
/// This attribute declares that a parameter's valid values come from calling
/// another method on the datasource (e.g., completion_states_with_progress).
///
/// Usage:
/// ```rust
/// #[enum_from(method = "completion_states_with_progress", param = "task_state")]
/// async fn set_state(&self, id: &str, task_state: String) -> Result<OperationResult>
/// ```
#[proc_macro_attribute]
pub fn enum_from(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Pass through unchanged - this just allows Rust to accept the attribute
    item
}

/// Generate an OperationDescriptor for a standalone async function
///
/// This macro generates a const `OPERATION_NAME_OP: OperationDescriptor` for a single function.
/// Useful for operations that aren't part of a trait.
///
/// Usage:
/// ```rust
/// #[operation]
/// /// Delete a block by ID
/// async fn delete_block(id: &str) -> Result<()> {
///     // Implementation
/// }
/// ```
///
/// The generated descriptor can be accessed via:
/// ```rust
/// use crate::operations::DELETE_BLOCK_OP;
/// let op = DELETE_BLOCK_OP();
/// ```
#[proc_macro_attribute]
pub fn operation(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let fn_item = parse_macro_input!(item as ItemFn);

    // Detect crate path (same logic as Entity macro)
    let pkg_name = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    let is_internal = pkg_name == "holon" || pkg_name == "holon-core";
    let crate_path = if is_internal {
        quote! { crate }
    } else {
        quote! { holon }
    };

    let fn_name = &fn_item.sig.ident;
    let const_name = format_ident!("{}_OP", fn_name.to_string().to_uppercase());

    // Extract doc comments for description
    let description = extract_doc_comments(&fn_item.attrs);

    // Extract parameters (skip &self if present)
    let params: Vec<_> = fn_item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Receiver(_) => None, // Skip &self
            FnArg::Typed(pat_type) => {
                let param_name = extract_param_name(&pat_type.pat);
                let (type_str, _required) = infer_type(&pat_type.ty);
                let param_name_lit = param_name.clone();
                let type_str_lit = type_str.clone();
                Some(quote! {
                    #crate_path::core::datasource::OperationParam {
                        name: #param_name_lit.to_string(),
                        type_hint: holon_api::TypeHint::from_string(#type_str_lit),
                        description: String::new(),
                    }
                })
            }
        })
        .collect();

    let name_lit = fn_name.to_string();
    let desc_lit = if description.is_empty() {
        String::new()
    } else {
        description.clone()
    };

    // Extract affected fields from #[operation(affects = [...])] attribute
    let affected_fields = extract_affected_fields(&fn_item.attrs);
    let affected_fields_expr = if affected_fields.is_empty() {
        quote! { vec![] }
    } else {
        let fields: Vec<_> = affected_fields
            .iter()
            .map(|s| quote! { #s.to_string() })
            .collect();
        quote! { vec![#(#fields),*] }
    };

    let expanded = quote! {
        // Original function (unchanged)
        #fn_item

        // Generated operation descriptor
        pub fn #const_name() -> #crate_path::core::datasource::OperationDescriptor {
            #crate_path::core::datasource::OperationDescriptor {
                name: #name_lit.to_string(),
                description: #desc_lit.to_string(),
                params: vec![
                    #(#params),*
                ],
                affected_fields: #affected_fields_expr,
            }
        }
    };

    TokenStream::from(expanded)
}

/// No-op proc macro for #[require(...)] attribute
/// This allows the attribute to be recognized by Rust's parser
/// The actual processing is done by the operations_trait macro
#[proc_macro_attribute]
pub fn require(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Just return the item unchanged - the operations_trait macro will process the require attributes
    // This is a no-op macro that just passes through the item
    // We clone the token stream to ensure proper span preservation for rust-analyzer
    item
}

/// Auto-discover builder modules and generate registration function.
///
/// Scans a directory for `.rs` files, emits `mod` declarations for each,
/// and generates a `register_all()` function that registers each module's
/// `build` function with a `RenderInterpreter`.
///
/// Usage:
/// ```ignore
/// builder_registry!("src/render/builders", skip: [prelude, operation_helpers]);
/// ```
#[proc_macro]
pub fn builder_registry(input: TokenStream) -> TokenStream {
    builder_registry::builder_registry_impl(input)
}

/// Attribute macro for shadow widget builders.
///
/// Generates arg extraction code, a `WIDGET_META` constant, and optionally
/// the entire `build` function body.
///
/// # Modes
///
/// **Auto-body** — no body provided, macro generates everything:
/// ```ignore
/// #[widget_builder]
/// fn badge(label: String);
/// ```
///
/// **Custom body** — extraction is generated, body is user-provided:
/// ```ignore
/// #[widget_builder]
/// fn section(title: String, children: Collection) {
///     let mut result = vec![ViewModel::leaf("text", Value::String(title.clone()))];
///     result.extend(children);
///     ViewModel::layout("section", result)
/// }
/// ```
///
/// **Raw** — no extraction, just adds `WIDGET_META`:
/// ```ignore
/// #[widget_builder(raw)]
/// fn tree(ba: BA<'_>) -> ViewModel {
///     // fully custom
/// }
/// ```
///
/// # Param types
///
/// | Type | Extraction |
/// |---|---|
/// | `String` | `get_positional_string(N)` / `get_string(name)` |
/// | `bool` | `get_bool(name)` |
/// | `f64` | `get_f64(name)` |
/// | `Value` | `positional[N]` / `named[name]` |
/// | `Collection` | template expansion over `data_rows` |
/// | `Expr` | `get_template(name)` |
#[proc_macro]
pub fn widget_builder(input: TokenStream) -> TokenStream {
    widget_builder::widget_builder_impl(input)
}
