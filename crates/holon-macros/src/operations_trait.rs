use holon_pattern::arcs::ArcEmit;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::arcs::TransitionArcs;
use holon_pattern::marking::ExistenceFlow;
use holon_pattern::marking::KindDelta;
use holon_pattern::marking::MarkingDelta;
use holon_pattern::marking::StructuralFlow;
use holon_pattern::marking::TextFlow;
use holon_pattern::pattern::BuiltinRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::OpGuard;
use holon_pattern::pattern::PathSegment;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;
use proc_macro2::TokenStream;
use quote::format_ident;
use quote::quote;
use syn::FnArg;
use syn::ItemTrait;
use syn::Meta;
use syn::Pat;
use syn::Type;
use syn::punctuated::Punctuated;

pub fn operations_trait_impl(attr: &str, trait_def: ItemTrait) -> TokenStream {
    // Parse provider_name from attribute: #[operations_trait(provider_name =
    // "todoist")]
    let provider_name = parse_provider_name_str(attr);

    let trait_name = &trait_def.ident;
    let operations_fn_name = format_ident!("{}", to_snake_case(&trait_name.to_string()));
    let operations_module_name =
        format_ident!("__operations_{}", to_snake_case(&trait_name.to_string()));

    // Check if trait has generic type parameters
    let has_generics = !trait_def.generics.params.is_empty();

    // Extract where clause constraints for the entity type parameter
    // Look for constraints on the generic parameter (usually T or E)
    // We need to map T -> E in the constraints
    let entity_constraints: Vec<_> = trait_def
        .generics
        .where_clause
        .as_ref()
        .map(|where_clause| {
            where_clause
                .predicates
                .iter()
                .filter_map(|pred| {
                    // Look for type bounds like `T: BlockEntity + Send + Sync`
                    if let syn::WherePredicate::Type(pred_type) = pred {
                        // Replace the type parameter name (T) with E in the predicate
                        // This is a simplified approach - we assume the first generic param is the
                        // entity type
                        let mut new_pred = pred_type.clone();
                        // Replace T with E in the type path
                        if let syn::Type::Path(type_path) = &mut new_pred.bounded_ty
                            && let Some(segment) = type_path.path.segments.first_mut()
                            && segment.ident == "T"
                        {
                            segment.ident = syn::Ident::new("E", segment.ident.span());
                        }
                        Some(quote! { #new_pred })
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Detect crate path for Result type and Value types (needed for dispatch
    // function generation)
    let pkg_name = std::env::var("CARGO_PKG_NAME").unwrap_or_default();
    // Only holon-core is "internal": the shared traits +
    // Result/UnknownOperationError live there, and every other crate (including
    // holon) reaches them via holon_core::.
    let is_internal = pkg_name == "holon-core";

    // Determine the Operation type path - Operation is now in holon-api
    // All crates should use holon_api::Operation
    let operation_type_path = quote! { holon_api::Operation };

    // OperationResult is re-exported from holon_core for external crates
    let operation_result_path = if pkg_name == "holon-core" {
        quote! { crate::OperationResult }
    } else {
        quote! { holon_core::OperationResult }
    };

    // UndoAction is still needed for extracting undo from OperationResult
    let undo_action_path = if pkg_name == "holon-core" {
        quote! { crate::UndoAction }
    } else {
        quote! { holon_core::UndoAction }
    };

    // UnknownOperationError and Result: for external crates use holon_core root.
    let unknown_op_error_path = if is_internal {
        quote! { crate::core::datasource::UnknownOperationError }
    } else {
        quote! { holon_core::UnknownOperationError }
    };
    let result_path = if is_internal {
        quote! { crate::core::datasource::Result }
    } else {
        quote! { holon_core::Result }
    };

    // Extract all async fn methods (skip associated types, consts, etc.)
    let methods: Vec<_> = trait_def
        .items
        .iter()
        .filter_map(|item| {
            // In syn 2.0, methods are TraitItem::Fn
            if let syn::TraitItem::Fn(method) = item {
                // Check if method is async (has asyncness)
                if method.sig.asyncness.is_some() {
                    Some(method)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();

    // Collect enum_from annotations for resolver function generation
    // Each entry is (operation_name, method_to_call, param_name)
    let enum_from_annotations: Vec<(String, String, String)> = methods
        .iter()
        .filter_map(|method| {
            let method_name = method.sig.ident.to_string();
            extract_enum_from(&method.attrs).map(|ef| (method_name, ef.method_name, ef.param_name))
        })
        .collect();

    // Generate OperationDescriptor function for each method
    let operation_fns: Vec<_> = methods
        .iter()
        .map(|method| {
            let method_name = &method.sig.ident;
            let fn_name = format_ident!("{}_OP", method_name.to_string().to_uppercase());

            // Extract doc comments for description
            let description = extract_doc_comments(&method.attrs);

            // Extract parameters (skip &self, only include required params)
            let params: Vec<_> = method
                .sig
                .inputs
                .iter()
                .skip(1) // Skip &self
                .filter_map(|arg| match arg {
                    FnArg::Typed(pat_type) => {
                        let param_name = param_wire_name(&extract_param_name(&pat_type.pat));
                        let (type_str, required) = infer_type(&pat_type.ty);

                        // Skip optional parameters (Option<T> types)
                        if !required {
                            return None;
                        }

                        let param_name_lit = param_name.clone();
                        let type_str_lit = type_str.clone();

                        // Parse type hint with entity ID detection
                        let type_hint_expr =
                            parse_param_type_hint(&param_name, &pat_type.attrs, &type_str_lit);

                        Some(quote! {
                            holon_api::OperationParam {
                                name: #param_name_lit.to_string(),
                                type_hint: #type_hint_expr,
                                description: String::new(), // TODO: Extract from doc comments
                            }
                        })
                    }
                    _ => None,
                })
                .collect();

            // Use stringify! for name and description (compile-time strings)
            let name_lit = method_name.to_string();
            let display_name = to_display_name(&name_lit);
            let desc_lit = if description.is_empty() {
                format!("Execute {}", display_name)
            } else {
                description.clone()
            };

            // The declared guard (ADR 0031): `#[require("…")]` literals are
            // parsed HERE, at expansion time, and emitted as the declarative
            // `OpGuard` value. A parse error is a compile error.
            let guard_field = match extract_require_guard(&method.attrs) {
                Ok(guard) => {
                    let expr = op_guard_tokens(&guard);
                    quote! { guard: #expr, }
                }
                Err(err) => {
                    let err = err.to_compile_error();
                    quote! { guard: { #err }, }
                }
            };

            // The declared transition arcs (ADR 0031 Increment 2): the
            // `#[reads]`/`#[emits]` place literals are parsed HERE, at
            // expansion time, so an unknown relation is a compile error.
            let arcs_field = match extract_transition_arcs(&method.attrs) {
                Ok(arcs) => {
                    let expr = transition_arcs_tokens(&arcs);
                    quote! { arcs: #expr, }
                }
                Err(err) => {
                    let err = err.to_compile_error();
                    quote! { arcs: { #err }, }
                }
            };

            // The declared marking delta (ADR 0032 §4): aspect flows per entity
            // kind, parsed HERE so a `varies_by` naming a parameter the method
            // does not take is a compile error.
            let delta_field = match extract_marking_delta(&method.attrs, &method.sig) {
                Ok(delta) => {
                    let expr = marking_delta_tokens(&delta);
                    quote! { marking_delta: #expr, }
                }
                Err(err) => {
                    let err = err.to_compile_error();
                    quote! { marking_delta: { #err }, }
                }
            };

            // Extract affected fields from #[operation(affects = [...])] attribute
            let affected_fields = extract_affected_fields(&method.attrs);
            let affected_fields_expr = if affected_fields.is_empty() {
                quote! { vec![] }
            } else {
                let fields: Vec<_> = affected_fields
                    .iter()
                    .map(|s| quote! { #s.to_string() })
                    .collect();
                quote! { vec![#(#fields),*] }
            };

            // Extract param_mappings from #[triggered_by(...)] attributes
            let param_mappings = extract_param_mappings(&method.attrs);
            let param_mappings_expr = if param_mappings.is_empty() {
                quote! { vec![] }
            } else {
                let mapping_exprs: Vec<_> = param_mappings
                    .iter()
                    .map(|m| {
                        let from = &m.availability_of;
                        let provides: Vec<_> = m
                            .providing
                            .iter()
                            .map(|s| quote! { #s.to_string() })
                            .collect();
                        quote! {
                            holon_api::ParamMapping {
                                from: #from.to_string(),
                                provides: vec![#(#provides),*],
                                defaults: std::collections::HashMap::new(),
                            }
                        }
                    })
                    .collect();
                quote! { vec![#(#mapping_exprs),*] }
            };

            // Extract UI menu exposure from #[menu_exposure(...)]. Absent ⇒
            // fail-closed `NotListed { ProviderDefault }` — a macro-generated
            // provider op stays invisible to the slash menu until it opts in.
            let menu_exposure_expr = menu_exposure_tokens(extract_menu_exposure(&method.attrs));

            // Extract sharing/audience boundary behavior from
            // #[boundary_behavior(...)]. Absent ⇒ fail-closed `Unclassified`
            // (ADR 0028 C3) — the boundary correspondence-lock rejects that for
            // structural ops.
            let boundary_behavior_expr =
                boundary_behavior_tokens(extract_boundary_behavior(&method.attrs));

            // Construct entity_name: if provider_name is set, use
            // "{provider_name}.{operation_name}", otherwise use passed entity_name
            let entity_name_expr = if let Some(ref provider) = provider_name {
                let provider_lit = provider.clone();
                let operation_name_lit = name_lit.clone();
                quote! {
                    holon_api::EntityName::new(format!("{}.{}", #provider_lit, #operation_name_lit))
                }
            } else {
                quote! {
                    holon_api::EntityName::new(entity_name)
                }
            };

            quote! {
                /// Generate operation descriptor for this method
                ///
                /// Parameters:
                /// - entity_name: Entity identifier (e.g., "todoist_task", "block")
                ///   Note: If provider_name is set in macro, entity_name will be "{provider_name}.{operation_name}"
                /// - entity_short_name: Short name for entity-typed params (e.g., "task", "project")
                /// - table: Database table name (e.g., "todoist_task", "block")
                /// - id_column: Primary key column name (default: "id")
                pub fn #fn_name(
                    entity_name: &str,
                    entity_short_name: &str,
                    table: &str,
                    id_column: &str
                ) -> holon_api::OperationDescriptor {
                    holon_api::OperationDescriptor {
                        entity_name: #entity_name_expr,
                        entity_short_name: entity_short_name.to_string(),
                        id_column: id_column.to_string(),
                        name: #name_lit.to_string(),
                        display_name: #display_name.to_string(),
                        description: #desc_lit.to_string(),
                        required_params: vec![
                            #(#params),*
                        ],
                        affected_fields: #affected_fields_expr,
                        param_mappings: #param_mappings_expr,
                        menu_exposure: #menu_exposure_expr,
                        boundary_behavior: #boundary_behavior_expr,
                        target_scope: holon_api::TargetScope::Block,
                        trigger: None,
                        bound_params: ::std::collections::HashMap::new(),
                        #guard_field
                        #arcs_field
                        #delta_field
                    }
                }
            }
        })
        .collect();

    // Generate operation constructor functions (*_op) for each method
    let operation_constructor_fns: Vec<_> = methods
        .iter()
        .map(|method| {
            let method_name = &method.sig.ident;
            let op_fn_name = format_ident!("{}_op", method_name);
            let name_lit = method_name.to_string();
            let display_name = to_display_name(&name_lit);
            let _description = extract_doc_comments(&method.attrs);

            // Extract all parameters (including Option<T>) for the constructor function
            let mut param_defs = Vec::new();
            let mut param_conversions = Vec::new();

            for arg in method.sig.inputs.iter().skip(1) {  // Skip &self
                if let FnArg::Typed(pat_type) = arg {
                    let param_name = param_wire_name(&extract_param_name(&pat_type.pat));
                    let param_name_ident = match &*pat_type.pat {
                        Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                        _ => syn::Ident::new(&param_name, proc_macro2::Span::call_site()),
                    };
                    let (type_str, is_required) = infer_type(&pat_type.ty);
                    let type_str_cleaned = type_str.replace(" ", "");

                    // Generate parameter definition
                    // Use quote! to properly format the type
                    let param_ty = &pat_type.ty;
                    param_defs.push(quote! {
                        #param_name_ident: #param_ty
                    });

                    // Generate conversion to Value for StorageEntity
                    let param_name_lit = param_name.clone();
                    let conversion = if type_str_cleaned == "String" || type_str_cleaned == "&str" {
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::String(#param_name_ident.to_string()))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.map(|v| holon_api::Value::String(v.to_string())).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    } else if type_str_cleaned == "bool" {
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::Boolean(#param_name_ident))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.map(holon_api::Value::Boolean).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    } else if type_str_cleaned.starts_with("i64") {
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::Integer(#param_name_ident))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.map(holon_api::Value::Integer).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    } else if type_str_cleaned.starts_with("i32") {
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::Integer(#param_name_ident as i64))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.map(|v| holon_api::Value::Integer(v as i64)).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    } else if type_str_cleaned == "EntityUri" {
                        // EntityUri (owned or &) flattens back to its boundary
                        // string representation for the Value side-channel.
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::String(#param_name_ident.as_str().to_string()))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.as_ref().map(|v| holon_api::Value::String(v.as_str().to_string())).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    } else if type_str_cleaned == "HashMap" {
                        // StorageEntity keys are Arc<str>; Value::Object keys are String
                        quote! {
                            (#param_name_lit.to_string(), holon_api::Value::Object(#param_name_ident.into_iter().map(|(k, v)| (k.to_string(), v)).collect()))
                        }
                    } else if type_str_cleaned.contains("DateTime") {
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::from_datetime(#param_name_ident))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.map(|v| holon_api::Value::from_datetime(v)).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    } else {
                        // ALLOW(fallback): default branch comment — try Value::from
                        if is_required {
                            quote! {
                                (#param_name_lit.to_string(), holon_api::Value::from(#param_name_ident))
                            }
                        } else {
                            quote! {
                                (#param_name_lit.to_string(), #param_name_ident.map(|v| holon_api::Value::from(v)).unwrap_or(holon_api::Value::Null))
                            }
                        }
                    };

                    param_conversions.push(conversion);
                }
            }

            // Construct entity_name expression (same logic as operation descriptor)
            let entity_name_expr = if let Some(ref provider) = provider_name {
                let provider_lit = provider.clone();
                let operation_name_lit = name_lit.clone();
                quote! {
                    format!("{}.{}", #provider_lit, #operation_name_lit)
                }
            } else {
                quote! {
                    entity_name
                }
            };

            quote! {
                /// Construct an Operation for this method
                ///
                /// # Parameters
                /// - entity_name: Entity identifier (e.g., "todoist_task", "block")
                /// - All method parameters (same as the trait method, minus &self)
                ///
                /// # Returns
                /// An Operation struct ready to be executed via OperationProvider
                pub fn #op_fn_name(
                    entity_name: &str,
                    #(#param_defs),*
                ) -> #operation_type_path {
                    #operation_type_path::from_params(
                        #entity_name_expr,
                        #name_lit,
                        #display_name,
                        vec![
                            #(#param_conversions),*
                        ]
                    )
                }
            }
        })
        .collect();

    // Generate dispatch function code for each method
    let dispatch_cases: Vec<_> = methods.iter()
        .map(|method| {
            let method_name = &method.sig.ident;
            let method_name_str = method_name.to_string();

            // Extract parameters and generate extraction code, building both lists together
            let mut param_extractions_code = Vec::new();
            let mut param_names_for_call = Vec::new();

            for arg in method.sig.inputs.iter().skip(1) {  // Skip &self
                if let FnArg::Typed(pat_type) = arg {
                    let param_name_ident = match &*pat_type.pat {
                        Pat::Ident(pat_ident) => pat_ident.ident.clone(),
                        _ => {
                            // ALLOW(fallback): default branch comment — best-effort name extraction
                            let name_str = extract_param_name(&pat_type.pat);
                            syn::Ident::new(&name_str, proc_macro2::Span::call_site())
                        }
                    };
                    let param_name_str = param_wire_name(&param_name_ident.to_string());
                    let (type_str, is_required) = infer_type(&pat_type.ty);
                    let is_optional = !is_required;  // Convert required flag to optional flag
                    let type_str_cleaned = type_str.replace(" ", "");

                    // Check if original type was a reference (for &str handling)
                    // Check the actual type structure, not stringified version
                    let is_ref_type = matches!(&*pat_type.ty, syn::Type::Reference(_));

                    // For Option<&str>, check if inner type is a reference
                    let is_option_ref_str = if is_optional {
                        if let syn::Type::Path(type_path) = &*pat_type.ty {
                            if let Some(segment) = type_path.path.segments.last() {
                                if segment.ident == "Option" {
                                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                                        if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                                            matches!(inner_ty, syn::Type::Reference(_))
                                        } else {
                                            false
                                        }
                                    } else {
                                        false
                                    }
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    // Generate extraction code based on type
                    let extraction = if type_str_cleaned == "String" || type_str_cleaned == "&str" {
                        if is_optional {
                            quote! {
                                let #param_name_ident: Option<String> = match params.get(#param_name_str) {
                                    None | Some(holon_api::Value::Null) => None,
                                    Some(v) => Some(v.as_string().map(|s| s.to_string())
                                        .ok_or_else(|| format!("Invalid type for optional parameter '{}' (expected String)", #param_name_str))?),
                                };
                            }
                        } else {
                            quote! {
                                let #param_name_ident: String = params.get(#param_name_str)
                                    .and_then(|v| v.as_string().map(|s| s.to_string()))
                                    .ok_or_else(|| format!("Missing or invalid parameter '{}' (expected String)", #param_name_str))?;
                            }
                        }
                    } else if type_str_cleaned == "bool" {
                        if is_optional {
                            quote! {
                                let #param_name_ident: Option<bool> = match params.get(#param_name_str) {
                                    None | Some(holon_api::Value::Null) => None,
                                    Some(v) => Some(v.as_bool()
                                        .ok_or_else(|| format!("Invalid type for optional parameter '{}' (expected bool)", #param_name_str))?),
                                };
                            }
                        } else {
                            quote! {
                                let #param_name_ident: bool = params.get(#param_name_str)
                                    .and_then(|v| v.as_bool())
                                    .ok_or_else(|| format!("Missing or invalid parameter '{}' (expected bool)", #param_name_str))?;
                            }
                        }
                    } else if type_str_cleaned.starts_with("i64") {
                        if is_optional {
                            quote! {
                                let #param_name_ident: Option<i64> = match params.get(#param_name_str) {
                                    None | Some(holon_api::Value::Null) => None,
                                    Some(v) => Some(v.as_i64()
                                        .ok_or_else(|| format!("Invalid type for optional parameter '{}' (expected i64)", #param_name_str))?),
                                };
                            }
                        } else {
                            quote! {
                                let #param_name_ident: i64 = params.get(#param_name_str)
                                    .and_then(|v| v.as_i64())
                                    .ok_or_else(|| format!("Missing or invalid parameter '{}' (expected i64)", #param_name_str))?;
                            }
                        }
                    } else if type_str_cleaned.starts_with("i32") {
                        if is_optional {
                            quote! {
                                let #param_name_ident: Option<i32> = match params.get(#param_name_str) {
                                    None | Some(holon_api::Value::Null) => None,
                                    Some(v) => Some(v.as_i64().map(|i| i as i32)
                                        .ok_or_else(|| format!("Invalid type for optional parameter '{}' (expected i32)", #param_name_str))?),
                                };
                            }
                        } else {
                            quote! {
                                let #param_name_ident: i32 = params.get(#param_name_str)
                                    .and_then(|v| v.as_i64().map(|i| i as i32))
                                    .ok_or_else(|| format!("Missing or invalid parameter '{}' (expected i32)", #param_name_str))?;
                            }
                        }
                    } else if type_str_cleaned == "HashMap" {
                        // For HashMap<String, Value>, extract the whole StorageEntity
                        // Check original type to confirm it's HashMap<String, Value>
                        let original_type_str = quote! { #pat_type.ty }.to_string();
                        let original_type_contains_value = original_type_str.contains("Value")
                            || original_type_str.contains("StorageEntity");
                        if original_type_contains_value {
                            quote! {
                                let #param_name_ident: holon_api::StorageEntity = params.clone();
                            }
                        } else {
                            quote! {
                                let #param_name_ident: holon_api::Value = params.get(#param_name_str)
                                    .cloned()
                                    .ok_or_else(|| format!("Missing parameter '{}' (expected Value)", #param_name_str))?;
                            }
                        }
                    } else if is_optional && type_str_cleaned.contains("DateTime") {
                        quote! {
                            let #param_name_ident: Option<chrono::DateTime<chrono::Utc>> = match params.get(#param_name_str) {
                                None | Some(holon_api::Value::Null) => None,
                                Some(v) => Some(v.as_datetime()
                                    .ok_or_else(|| format!("Invalid type for optional parameter '{}' (expected DateTime)", #param_name_str))?),
                            };
                        }
                    } else if type_str_cleaned == "Value" {
                        // For Value type, clone directly
                        if is_optional {
                            quote! {
                                let #param_name_ident: Option<holon_api::Value> = params.get(#param_name_str).cloned();
                            }
                        } else {
                            quote! {
                                let #param_name_ident: holon_api::Value = params.get(#param_name_str)
                                    .cloned()
                                    .ok_or_else(|| format!("Missing parameter '{}' (expected Value)", #param_name_str))?;
                            }
                        }
                    } else if type_str_cleaned == "EntityUri" {
                        // Parse-don't-validate: extract the boundary string and
                        // lift it to a typed EntityUri at the dispatch edge so
                        // trait methods see only the parsed form.
                        if is_optional {
                            quote! {
                                let #param_name_ident: Option<holon_api::EntityUri> = match params.get(#param_name_str) {
                                    None | Some(holon_api::Value::Null) => None,
                                    Some(v) => Some(v.as_string()
                                        // ALLOW(entity_uri_from_raw): MCP operation params HashMap string → EntityUri at dispatch edge
                                        .map(|s| holon_api::EntityUri::from_raw(s))
                                        .ok_or_else(|| format!("Invalid type for optional parameter '{}' (expected EntityUri-as-String)", #param_name_str))?),
                                };
                            }
                        } else {
                            quote! {
                                let #param_name_ident: holon_api::EntityUri = params.get(#param_name_str)
                                    // ALLOW(entity_uri_from_raw): MCP operation params HashMap string → EntityUri at dispatch edge
                                    .and_then(|v| v.as_string().map(|s| holon_api::EntityUri::from_raw(s)))
                                    .ok_or_else(|| format!("Missing or invalid parameter '{}' (expected EntityUri-as-String)", #param_name_str))?;
                            }
                        }
                    } else {
                        // For other types, try to clone Value and let the trait method handle conversion
                        quote! {
                            let #param_name_ident: holon_api::Value = params.get(#param_name_str)
                                .cloned()
                                .ok_or_else(|| format!("Missing parameter '{}' (expected Value)", #param_name_str))?;
                        }
                    };

                    param_extractions_code.push(extraction);

                    // EntityUri (owned `&EntityUri` or `Option<&EntityUri>`) is
                    // always extracted as a typed `EntityUri` / `Option<EntityUri>`
                    // (see the EntityUri extraction branch above), so we borrow it
                    // here. This must come before the `is_option_ref_str` arm:
                    // `Option<&EntityUri>` also has a reference inner type, and the
                    // `&str` arm would otherwise wrongly emit `.map(|s| s.as_str())`.
                    if type_str_cleaned == "EntityUri" {
                        if is_optional && is_option_ref_str {
                            // Option<&EntityUri>: extracted as Option<EntityUri>, borrow each.
                            param_names_for_call.push(quote! { #param_name_ident.as_ref() });
                        } else if is_optional {
                            // Option<EntityUri> (owned): move it.
                            param_names_for_call.push(quote! { #param_name_ident });
                        } else if is_ref_type {
                            // &EntityUri: extracted as owned EntityUri, borrow it.
                            param_names_for_call.push(quote! { &#param_name_ident });
                        } else {
                            // EntityUri (owned): move it.
                            param_names_for_call.push(quote! { #param_name_ident });
                        }
                    } else if (is_ref_type && type_str_cleaned == "String") || is_option_ref_str {
                        if is_optional {
                            // For Option<&str>, extract as Option<String> and borrow
                            param_names_for_call.push(quote! { #param_name_ident.as_ref().map(|s| s.as_str()) });
                        } else {
                            param_names_for_call.push(quote! { &*#param_name_ident });
                        }
                    } else {
                        param_names_for_call.push(quote! { #param_name_ident });
                    }
                }
            }

            // Check return type - handle different return types
            // Use syn to inspect the return type structure instead of string conversion
            let return_handling = match &method.sig.output {
                syn::ReturnType::Type(_, ty) => {
                    // Check if it's Result<T>
                    if let syn::Type::Path(type_path) = &**ty {
                        if let Some(segment) = type_path.path.segments.last() {
                            if segment.ident == "Result" {
                                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                                    if let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first() {
                                        // Check inner type
                                        match inner_ty {
                                            syn::Type::Path(inner_path) if inner_path.path.is_ident("String") => {
                                                // Result<String> -> Result<OperationResult>
                                                // This shouldn't happen anymore, but handle it
                                                quote! {
                                                    target.#method_name(#(#param_names_for_call),*).await.map(|_| #operation_result_path::irreversible(Vec::new()))
                                                }
                                            }
                                            syn::Type::Tuple(tuple) if tuple.elems.len() == 2 => {
                                                // Result<(String, OperationResult)> -> Result<OperationResult> (extract the OperationResult)
                                                quote! {
                                                    target.#method_name(#(#param_names_for_call),*).await.map(|(_, result)| result)
                                                }
                                            }
                                            syn::Type::Path(inner_path) => {
                                                // Check if it's OperationResult, UndoAction (for backward compat), or Option<Operation>
                                                if let Some(seg) = inner_path.path.segments.last() {
                                                    if seg.ident == "OperationResult" {
                                                        // Result<OperationResult> -> Result<OperationResult> (pass through)
                                                        quote! {
                                                            target.#method_name(#(#param_names_for_call),*).await
                                                        }
                                                    } else if seg.ident == "UndoAction" {
                                                        // Result<UndoAction> -> Result<OperationResult> (convert via From)
                                                        quote! {
                                                            target.#method_name(#(#param_names_for_call),*).await.map(|undo| #operation_result_path::from(undo))
                                                        }
                                                    } else if seg.ident == "Option" {
                                                        // Result<Option<Operation>> -> Result<OperationResult> (convert via Into then From)
                                                        quote! {
                                                            target.#method_name(#(#param_names_for_call),*).await.map(|opt| #operation_result_path::from(#undo_action_path::from(opt)))
                                                        }
                                                    } else {
                                                        // Other Result<T> -> pass through as Irreversible
                                                        quote! {
                                                            target.#method_name(#(#param_names_for_call),*).await.map(|_| #operation_result_path::irreversible(Vec::new()))
                                                        }
                                                    }
                                                } else {
                                                    quote! {
                                                        target.#method_name(#(#param_names_for_call),*).await.map(|_| #operation_result_path::irreversible(Vec::new()))
                                                    }
                                                }
                                            }
                                            _ => {
                                                quote! {
                                                    target.#method_name(#(#param_names_for_call),*).await
                                                }
                                            }
                                        }
                                    } else {
                                        quote! {
                                            target.#method_name(#(#param_names_for_call),*).await
                                        }
                                    }
                                } else {
                                    quote! {
                                        target.#method_name(#(#param_names_for_call),*).await
                                    }
                                }
                            } else {
                                quote! {
                                    target.#method_name(#(#param_names_for_call),*).await
                                }
                            }
                        } else {
                            quote! {
                                target.#method_name(#(#param_names_for_call),*).await
                            }
                        }
                    } else {
                        quote! {
                            target.#method_name(#(#param_names_for_call),*).await
                        }
                    }
                }
                syn::ReturnType::Default => {
                    quote! {
                        target.#method_name(#(#param_names_for_call),*).await
                    }
                }
            };

            quote! {
                #method_name_str => {
                    #(#param_extractions_code)*
                    #return_handling
                }
            }
        })
        .collect();

    // Generate function calls for the operations array
    let operation_calls: Vec<_> = methods
        .iter()
        .map(|method| {
            let method_name = &method.sig.ident;
            let fn_name = format_ident!("{}_OP", method_name.to_string().to_uppercase());
            quote! { #fn_name(entity_name, entity_short_name, table, id_column) }
        })
        .collect();

    // Generate the dispatch function differently based on whether trait has
    // generics
    let dispatch_fn = if has_generics {
        quote! {
            pub async fn dispatch_operation<DS, E>(
                target: &DS,
                op_name: &str,
                params: &StorageEntity
            ) -> Result<#operation_result_path>
            where
                DS: #trait_name<E> + Send + Sync,
                E: Send + Sync + 'static,
                #(#entity_constraints),*
            {
                match op_name {
                    #(#dispatch_cases),*
                    _ => Err(#unknown_op_error_path::new(
                        stringify!(#trait_name),
                        op_name,
                    ).into())
                }
            }
        }
    } else {
        quote! {
            pub async fn dispatch_operation<DS>(
                target: &DS,
                op_name: &str,
                params: &StorageEntity
            ) -> Result<#operation_result_path>
            where
                DS: #trait_name + Send + Sync,
            {
                match op_name {
                    #(#dispatch_cases),*
                    _ => Err(#unknown_op_error_path::new(
                        stringify!(#trait_name),
                        op_name,
                    ).into())
                }
            }
        }
    };

    // Generate resolver function that resolves enum_from annotations
    let resolver_fn_name = format_ident!("{}_with_resolver", operations_fn_name);

    // Generate resolver statements for each enum_from annotation
    let resolver_statements: Vec<_> = enum_from_annotations
        .iter()
        .map(|(op_name, method_name, param_name)| {
            let method_ident = syn::Ident::new(method_name, proc_macro2::Span::call_site());
            quote! {
                // Resolve enum values for #op_name.#param_name from #method_name()
                if let Some(op) = ops.iter_mut().find(|o| o.name == #op_name) {
                    if let Some(param) = op.required_params.iter_mut().find(|p| p.name == #param_name) {
                        // Convert CompletionStateInfo to Value
                        // Note: CompletionStateInfo is from holon_core, accessible via the trait
                        let values: Vec<holon_api::Value> = ds.#method_ident()
                            .into_iter()
                            .map(|info| {
                                // Serialize CompletionStateInfo to serde_json::Value, then convert to holon_api::Value
                                let json_value = serde_json::to_value(&info)
                                    .unwrap_or_else(|_| serde_json::Value::Null);
                                holon_api::Value::from_json_value(json_value)
                            })
                            .collect();
                        param.type_hint = holon_api::TypeHint::OneOf {
                            values,
                        };
                    }
                }
            }
        })
        .collect();

    // Generate resolver function with trait bounds matching the dispatch function
    let resolver_fn = if has_generics {
        quote! {
            /// All operations for this trait with resolved enum values
            ///
            /// This function resolves #[enum_from] annotations by calling the
            /// specified methods on the datasource to get valid enum values.
            ///
            /// Parameters:
            /// - ds: Datasource implementing this trait
            /// - entity_name: Entity identifier (e.g., "todoist_task", "block")
            /// - entity_short_name: Short name for entity-typed params (e.g., "task", "project")
            /// - table: Database table name (e.g., "todoist_task", "block")
            /// - id_column: Primary key column name (default: "id")
            pub fn #resolver_fn_name<DS, E>(
                ds: &DS,
                entity_name: &str,
                entity_short_name: &str,
                table: &str,
                id_column: &str
            ) -> Vec<holon_api::OperationDescriptor>
            where
                DS: #trait_name<E> + Send + Sync,
                #(#entity_constraints),*
            {
                let mut ops = #operations_fn_name(entity_name, entity_short_name, table, id_column);
                #(#resolver_statements)*
                ops
            }
        }
    } else {
        quote! {
            /// All operations for this trait with resolved enum values
            ///
            /// This function resolves #[enum_from] annotations by calling the
            /// specified methods on the datasource to get valid enum values.
            ///
            /// Parameters:
            /// - ds: Datasource implementing this trait
            /// - entity_name: Entity identifier (e.g., "todoist_task", "block")
            /// - entity_short_name: Short name for entity-typed params (e.g., "task", "project")
            /// - table: Database table name (e.g., "todoist_task", "block")
            /// - id_column: Primary key column name (default: "id")
            pub fn #resolver_fn_name<DS>(
                ds: &DS,
                entity_name: &str,
                entity_short_name: &str,
                table: &str,
                id_column: &str
            ) -> Vec<holon_api::OperationDescriptor>
            where
                DS: #trait_name + Send + Sync,
            {
                let mut ops = #operations_fn_name(entity_name, entity_short_name, table, id_column);
                #(#resolver_statements)*
                ops
            }
        }
    };

    // `#[entity_ref(..)]` / `#[not_entity]` are descriptor metadata read by
    // `parse_param_type_hint`. Rust has no inert-attribute registration for
    // function params, so they must not survive into the emitted trait.
    let mut trait_def = trait_def.clone();
    for item in &mut trait_def.items {
        if let syn::TraitItem::Fn(method) = item {
            for arg in &mut method.sig.inputs {
                if let syn::FnArg::Typed(pat_type) = arg {
                    pat_type.attrs.retain(|a| {
                        !a.path().is_ident("entity_ref") && !a.path().is_ident("not_entity")
                    });
                }
            }
        }
    }

    let expanded = quote! {
        // Original trait, minus the param-level descriptor attributes
        #trait_def

        // Generated operations module
        #[doc(hidden)]
        // Descriptor/dispatch fns mirror the widest trait method, so their arity
        // is the trait's — not something a caller can restructure here.
        #[allow(clippy::too_many_arguments)]
        pub mod #operations_module_name {
            use super::*;
            use holon_api::StorageEntity;
            use holon_api::Value;
            use #result_path;
            // Operation is now in holon-api, use holon_api::Operation
            use #operation_type_path as Operation;

            #(#operation_fns)*

            // Operation constructor functions (*_op)
            #(#operation_constructor_fns)*

            /// All operations for this trait
            ///
            /// Parameters:
            /// - entity_name: Entity identifier (e.g., "todoist_task", "block")
            /// - entity_short_name: Short name for entity-typed params (e.g., "task", "project")
            /// - table: Database table name (e.g., "todoist_task", "block")
            /// - id_column: Primary key column name (default: "id")
            pub fn #operations_fn_name(
                entity_name: &str,
                entity_short_name: &str,
                table: &str,
                id_column: &str
            ) -> Vec<holon_api::OperationDescriptor> {
                vec![
                    #(#operation_calls),*
                ]
            }

            #resolver_fn

            /// Dispatch operation to appropriate trait method
            ///
            /// Extracts parameters from StorageEntity and calls the appropriate trait method.
            /// Returns an error if the operation name is not recognized or parameters are invalid.
            ///
            /// Note: For generic traits, the entity type `E` must satisfy all constraints required by the trait.
            /// For example, `BlockOperations<E>` requires `E: BlockEntity`.
            #dispatch_fn
        }
    };

    expanded
}

/// Parse provider_name from macro attribute string:
/// #[operations_trait(provider_name = "todoist")]
fn parse_provider_name_str(attr_str: &str) -> Option<String> {
    if attr_str.is_empty() {
        return None;
    }

    // Look for provider_name = "value" pattern
    if let Some(start) = attr_str.find("provider_name")
        && let Some(equals) = attr_str[start..].find('=')
    {
        let value_start = attr_str[start + equals + 1..].find('"')? + start + equals + 1;
        let value_end = attr_str[value_start + 1..].find('"')? + value_start + 1;
        return Some(attr_str[value_start + 1..value_end].to_string());
    }
    None
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap_or(c));
    }
    result
}

fn to_display_name(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = true;

    for c in s.chars() {
        if c == '_' {
            result.push(' ');
            capitalize_next = true;
        } else if c.is_uppercase() && !result.is_empty() {
            result.push(' ');
            result.push(c);
            capitalize_next = false;
        } else if capitalize_next {
            result.push(c.to_uppercase().next().unwrap_or(c));
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }

    result
}

/// Extract doc comments from attributes
pub fn extract_doc_comments(attrs: &[syn::Attribute]) -> String {
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc") {
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
                    let tokens = &meta_list.tokens;
                    let token_str = quote! { #tokens }.to_string();
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

use crate::attr_parser::ParsedEnumFrom;
use crate::attr_parser::ParsedParamMapping;
use crate::attr_parser::{self};

fn extract_param_mappings(attrs: &[syn::Attribute]) -> Vec<ParsedParamMapping> {
    attr_parser::extract_param_mappings(attrs)
}

fn extract_enum_from(attrs: &[syn::Attribute]) -> Option<ParsedEnumFrom> {
    attr_parser::extract_enum_from(attrs)
}

/// rustfmt's `format_strings` mangles long literals, and a corrupted guard that
/// still parses is worse than a broken build (ADR 0031 P2). Guards compose by
/// named sub-pattern instead.
const MAX_GUARD_LITERAL_LEN: usize = 80;

/// Parse every `#[require("…")]` on a method into the declared [`OpGuard`].
///
/// ADR 0031 P2: the literal is parsed HERE, at expansion time, so a parse error
/// is a compile error. P6=A: the guard is RELATIONAL — a predicate over the
/// state the op touches. Parameter validity belongs in typed params, never in a
/// guard, so there is no parameter subject to bind.
///
/// Several `#[require]`s conjoin, which is also the composition escape hatch
/// for the length lint.
fn extract_require_guard(attrs: &[syn::Attribute]) -> syn::Result<OpGuard> {
    let mut bodies = Vec::new();
    let mut sources: Vec<String> = Vec::new();

    for attr in attrs {
        let is_require = attr.path().is_ident("require")
            || (attr.path().segments.len() == 2
                && attr.path().segments[0].ident == "holon_macros"
                && attr.path().segments[1].ident == "require");
        if !is_require {
            continue;
        }
        let Meta::List(meta_list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "#[require] takes one guard string: #[require(\"has_tag(\\\"Page\\\")\")]",
            ));
        };
        let lit: syn::LitStr = syn::parse2(meta_list.tokens.clone()).map_err(|_| {
            syn::Error::new_spanned(
                &meta_list.tokens,
                "#[require] takes a guard STRING literal parsed by the Pattern parser (ADR 0031 \
                 P2), not a Rust expression",
            )
        })?;
        let text = lit.value();
        if text.len() > MAX_GUARD_LITERAL_LEN {
            return Err(syn::Error::new_spanned(
                &lit,
                format!(
                    "guard literal is {} characters, over the {MAX_GUARD_LITERAL_LEN}-character \
                     limit — rustfmt mangles long literals and a corrupted guard may still parse. \
                     Split it across several #[require(\"…\")] attributes (they conjoin).",
                    text.len()
                ),
            ));
        }
        let body = holon_pattern::pattern::parse_guard_body(&text)
            .map_err(|e| syn::Error::new_spanned(&lit, format!("invalid guard: {e}")))?;
        bodies.push(body);
        sources.push(text);
    }

    if bodies.is_empty() {
        return Ok(OpGuard::None);
    }
    let body = if bodies.len() == 1 {
        bodies.pop().expect("len == 1")
    } else {
        Pattern::And(bodies)
    };
    // Subject inference (and the mixed-subject rejection) is the parser's, so
    // the `#[require]` surface and the `when:` sugar yield identical guards.
    let guard = Guard::from_body(body).map_err(|e| {
        syn::Error::new_spanned(
            attrs
                .first()
                .expect("bodies non-empty ⇒ an attribute exists"),
            format!("invalid guard: {e}"),
        )
    })?;
    // Several `#[require]`s conjoin, so the quoted source must be the joined
    // text — quoting one literal would misdescribe what actually refused.
    Ok(OpGuard::Declared {
        guard,
        source: sources.join(" and "),
    })
}

/// Emit the parsed guard as a literal constructor expression. The macro's
/// output is plain, serializable data (ADR 0031's dual-consumer requirement) —
/// no closure, nothing to run at construction time.
fn op_guard_tokens(guard: &OpGuard) -> proc_macro2::TokenStream {
    match guard {
        OpGuard::None => quote! { holon_api::pattern::OpGuard::None },
        OpGuard::Declared { guard, source } => {
            let subject = match &guard.subject {
                Subject::Clock => quote! { holon_api::pattern::Subject::Clock },
                Subject::Block => quote! { holon_api::pattern::Subject::Block },
                Subject::Relation(r) => {
                    quote! { holon_api::pattern::Subject::Relation(#r.to_string()) }
                }
            };
            let body = pattern_tokens(&guard.body);
            quote! {
                holon_api::pattern::OpGuard::Declared {
                    guard: holon_api::pattern::Guard { subject: #subject, body: #body },
                    source: #source.to_string(),
                }
            }
        }
    }
}

/// Exclusion reasons are prose, so rustfmt's `format_strings` can rewrap them.
/// Prose cannot silently change meaning the way a guard can, but a reason long
/// enough to wrap is a reason that is explaining a design problem instead of
/// naming an authority.
const MAX_ARC_REASON_LEN: usize = 80;

/// One item inside `#[emits(...)]`: a place literal, or `excluded(place,
/// reason)`.
enum ParsedEmit {
    Writes(syn::LitStr),
    Excluded(syn::LitStr, syn::LitStr),
}

impl syn::parse::Parse for ParsedEmit {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(syn::LitStr) {
            return Ok(ParsedEmit::Writes(input.parse()?));
        }
        let keyword: syn::Ident = input.parse().map_err(|_| {
            input.error(
                "#[emits] takes place STRING literals, or excluded(\"relation.field\", \"why\")",
            )
        })?;
        if keyword != "excluded" {
            return Err(syn::Error::new_spanned(
                &keyword,
                format!("unknown #[emits] form {keyword}; the only form is excluded(…)"),
            ));
        }
        let inner;
        syn::parenthesized!(inner in input);
        let place: syn::LitStr = inner.parse()?;
        inner.parse::<syn::Token![,]>().map_err(|_| {
            syn::Error::new_spanned(
                &place,
                "excluded(…) requires a REASON: silence about a written place is the red this \
                 declaration exists to prevent",
            )
        })?;
        let reason: syn::LitStr = inner.parse()?;
        Ok(ParsedEmit::Excluded(place, reason))
    }
}

/// Parse one `relation.field` literal into an [`ArcPlace`], reporting a parse
/// failure at the literal's own span (the `#[require]` precedent).
fn parse_arc_place(lit: &syn::LitStr) -> syn::Result<ArcPlace> {
    ArcPlace::parse(&lit.value())
        .map_err(|e| syn::Error::new_spanned(lit, format!("invalid arc place: {e}")))
}

/// Attribute-path match that accepts both the bare and the `holon_macros::`
/// qualified spelling, as `#[require]` does.
fn attr_is(attr: &syn::Attribute, name: &str) -> bool {
    attr.path().is_ident(name)
        || (attr.path().segments.len() == 2
            && attr.path().segments[0].ident == "holon_macros"
            && attr.path().segments[1].ident == name)
}

/// Parse every `#[reads(...)]` / `#[emits(...)]` on a method into the declared
/// [`TransitionArcs`] (ADR 0031 Increment 2).
///
/// Places are parsed HERE, at expansion time, so an unknown relation is a
/// compile error pointing at the literal. Absent both attributes the op is
/// [`TransitionArcs::Undeclared`] — "not simulatable", never "writes nothing".
fn extract_transition_arcs(attrs: &[syn::Attribute]) -> syn::Result<TransitionArcs> {
    let mut reads: Vec<ArcPlace> = Vec::new();
    let mut emits: Vec<ArcEmit> = Vec::new();
    let mut saw_reads = false;
    let mut saw_emits = false;

    for attr in attrs {
        let is_reads = attr_is(attr, "reads");
        let is_emits = attr_is(attr, "emits");
        if !is_reads && !is_emits {
            continue;
        }
        let Meta::List(meta_list) = &attr.meta else {
            return Err(syn::Error::new_spanned(
                attr,
                "#[reads]/#[emits] take a parenthesized list of \"relation.field\" literals",
            ));
        };
        if is_reads {
            saw_reads = true;
            let lits = meta_list
                .parse_args_with(Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated)?;
            for lit in &lits {
                reads.push(parse_arc_place(lit)?);
            }
        } else {
            saw_emits = true;
            let items = meta_list
                .parse_args_with(Punctuated::<ParsedEmit, syn::Token![,]>::parse_terminated)?;
            for item in &items {
                match item {
                    ParsedEmit::Writes(lit) => emits.push(ArcEmit::Writes(parse_arc_place(lit)?)),
                    ParsedEmit::Excluded(place, reason) => {
                        let text = reason.value();
                        if text.len() > MAX_ARC_REASON_LEN {
                            return Err(syn::Error::new_spanned(
                                reason,
                                format!(
                                    "exclusion reason is {} characters, over the \
                                     {MAX_ARC_REASON_LEN}-character limit — name the authority \
                                     that owns the place, do not explain the design here",
                                    text.len()
                                ),
                            ));
                        }
                        emits.push(ArcEmit::Excluded {
                            place: parse_arc_place(place)?,
                            reason: text,
                        });
                    }
                }
            }
        }
    }

    if !saw_reads && !saw_emits {
        return Ok(TransitionArcs::Undeclared);
    }
    if !saw_emits {
        return Err(syn::Error::new_spanned(
            attrs
                .iter()
                .find(|a| attr_is(a, "reads"))
                .expect("saw_reads ⇒ a #[reads] attribute exists"),
            "an op that declares #[reads] must also declare #[emits] — declaring inputs while \
             staying silent about outputs is exactly what Undeclared already says. Write \
             #[emits()] for a genuinely read-only op.",
        ));
    }
    Ok(TransitionArcs::Declared { reads, emits })
}

/// Emit the parsed arcs as a literal constructor expression — plain
/// serializable data, for the same dual-consumer reason as the guard.
fn transition_arcs_tokens(arcs: &TransitionArcs) -> proc_macro2::TokenStream {
    match arcs {
        TransitionArcs::Undeclared => quote! { holon_api::arcs::TransitionArcs::Undeclared },
        TransitionArcs::Declared { reads, emits } => {
            let read_exprs = reads.iter().map(arc_place_tokens);
            let emit_exprs = emits.iter().map(|e| match e {
                ArcEmit::Writes(place) => {
                    let place = arc_place_tokens(place);
                    quote! { holon_api::arcs::ArcEmit::Writes(#place) }
                }
                ArcEmit::Excluded { place, reason } => {
                    let place = arc_place_tokens(place);
                    quote! {
                        holon_api::arcs::ArcEmit::Excluded {
                            place: #place,
                            reason: #reason.to_string(),
                        }
                    }
                }
            });
            quote! {
                holon_api::arcs::TransitionArcs::Declared {
                    reads: vec![#(#read_exprs),*],
                    emits: vec![#(#emit_exprs),*],
                }
            }
        }
    }
}

/// One kind clause inside `#[marking_delta(...)]`:
/// `block(structural = relocates, text = untouched, existence = reads)`.
struct ParsedKindDelta {
    kind: syn::Ident,
    aspects: Vec<(syn::Ident, syn::Ident)>,
}

impl syn::parse::Parse for ParsedKindDelta {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let kind: syn::Ident = input.parse().map_err(|_| {
            input.error(
                "#[marking_delta] takes kind clauses — \
                 block(structural = …, text = …, existence = …) — and an optional \
                 varies_by(\"param\", …)",
            )
        })?;
        let inner;
        syn::parenthesized!(inner in input);
        let mut aspects = Vec::new();
        while !inner.is_empty() {
            let aspect: syn::Ident = inner.parse()?;
            inner.parse::<syn::Token![=]>()?;
            let flow: syn::Ident = inner.parse()?;
            aspects.push((aspect, flow));
            if inner.peek(syn::Token![,]) {
                inner.parse::<syn::Token![,]>()?;
            }
        }
        Ok(ParsedKindDelta { kind, aspects })
    }
}

/// One item at the top level of `#[marking_delta(...)]`.
enum ParsedDeltaItem {
    Kind(ParsedKindDelta),
    VariesBy(Vec<syn::LitStr>),
}

impl syn::parse::Parse for ParsedDeltaItem {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let fork = input.fork();
        let head: syn::Ident = fork.parse()?;
        if head == "varies_by" {
            input.parse::<syn::Ident>()?;
            let inner;
            syn::parenthesized!(inner in input);
            let lits = Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated(&inner)?;
            return Ok(ParsedDeltaItem::VariesBy(lits.into_iter().collect()));
        }
        Ok(ParsedDeltaItem::Kind(input.parse()?))
    }
}

/// Every parameter name the method accepts, so `varies_by` cannot name one that
/// does not exist.
fn method_param_names(sig: &syn::Signature) -> Vec<String> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat) => match &*pat.pat {
                syn::Pat::Ident(ident) => Some(ident.ident.to_string()),
                _ => None,
            },
            syn::FnArg::Receiver(_) => None,
        })
        .collect()
}

fn parse_structural_flow(flow: &syn::Ident) -> syn::Result<StructuralFlow> {
    match flow.to_string().as_str() {
        "untouched" => Ok(StructuralFlow::Untouched),
        "reads" => Ok(StructuralFlow::Reads),
        "produces" => Ok(StructuralFlow::Produces),
        "consumes" => Ok(StructuralFlow::Consumes),
        "relocates" => Ok(StructuralFlow::Relocates),
        other => Err(syn::Error::new_spanned(
            flow,
            format!(
                "unknown structural flow {other}; one of untouched, reads, produces, consumes, \
                 relocates"
            ),
        )),
    }
}

fn parse_text_flow(flow: &syn::Ident) -> syn::Result<TextFlow> {
    match flow.to_string().as_str() {
        "untouched" => Ok(TextFlow::Untouched),
        "reads" => Ok(TextFlow::Reads),
        "produces" => Ok(TextFlow::Produces),
        "consumes" => Err(syn::Error::new_spanned(
            flow,
            "text tokens are CRDT-shared and never exclusively held, so nothing consumes them \
             (ADR 0032 §4)",
        )),
        other => Err(syn::Error::new_spanned(
            flow,
            format!("unknown text flow {other}; one of untouched, reads, produces"),
        )),
    }
}

fn parse_existence_flow(flow: &syn::Ident) -> syn::Result<ExistenceFlow> {
    match flow.to_string().as_str() {
        "untouched" => Ok(ExistenceFlow::Untouched),
        "reads" => Ok(ExistenceFlow::Reads),
        "produces" => Ok(ExistenceFlow::Produces),
        "consumes" => Err(syn::Error::new_spanned(
            flow,
            "existence tokens are never consumed; a deletion PRODUCES the absent state \
             (ADR 0032 §4)",
        )),
        other => Err(syn::Error::new_spanned(
            flow,
            format!("unknown existence flow {other}; one of untouched, reads, produces"),
        )),
    }
}

/// Parse `#[marking_delta(...)]` into the declared [`MarkingDelta`]. Absent,
/// the op is [`MarkingDelta::Undeclared`] — "cannot say", never "changes
/// nothing".
fn extract_marking_delta(
    attrs: &[syn::Attribute],
    sig: &syn::Signature,
) -> syn::Result<MarkingDelta> {
    let Some(attr) = attrs.iter().find(|a| attr_is(a, "marking_delta")) else {
        return Ok(MarkingDelta::Undeclared);
    };
    let Meta::List(meta_list) = &attr.meta else {
        return Err(syn::Error::new_spanned(
            attr,
            "#[marking_delta] takes a parenthesized list of kind clauses",
        ));
    };

    let items = meta_list
        .parse_args_with(Punctuated::<ParsedDeltaItem, syn::Token![,]>::parse_terminated)?;

    let mut kinds: Vec<KindDelta> = Vec::new();
    let mut varies_by: Option<Vec<String>> = None;

    for item in items {
        match item {
            ParsedDeltaItem::VariesBy(lits) => {
                let params = method_param_names(sig);
                let mut names = Vec::new();
                for lit in &lits {
                    let name = lit.value();
                    if !params.contains(&name) {
                        return Err(syn::Error::new_spanned(
                            lit,
                            format!(
                                "varies_by({name:?}) names no parameter of this method; it takes \
                                 {params:?}"
                            ),
                        ));
                    }
                    names.push(name);
                }
                if names.is_empty() {
                    return Err(syn::Error::new_spanned(
                        attr,
                        "varies_by() with no parameters claims the delta is dynamic without \
                         saying what decides it — declare the static delta instead",
                    ));
                }
                varies_by = Some(names);
            }
            ParsedDeltaItem::Kind(parsed) => {
                let mut structural = None;
                let mut text = None;
                let mut existence = None;
                for (aspect, flow) in &parsed.aspects {
                    match aspect.to_string().as_str() {
                        "structural" => structural = Some(parse_structural_flow(flow)?),
                        "text" => text = Some(parse_text_flow(flow)?),
                        "existence" => existence = Some(parse_existence_flow(flow)?),
                        other => {
                            return Err(syn::Error::new_spanned(
                                aspect,
                                format!(
                                    "unknown aspect {other}; the aspects are structural, text, \
                                     existence (ADR 0032 §4)"
                                ),
                            ));
                        }
                    }
                }
                let missing: Vec<&str> = [
                    ("structural", structural.is_none()),
                    ("text", text.is_none()),
                    ("existence", existence.is_none()),
                ]
                .into_iter()
                .filter(|(_, absent)| *absent)
                .map(|(name, _)| name)
                .collect();
                if !missing.is_empty() {
                    return Err(syn::Error::new_spanned(
                        &parsed.kind,
                        format!(
                            "kind {} leaves {missing:?} unstated; every aspect is declared, and \
                             `untouched` is how an op says it moves none of that aspect's tokens",
                            parsed.kind
                        ),
                    ));
                }
                let relation = parsed.kind.to_string();
                if kinds.iter().any(|k| k.kind.as_str() == relation) {
                    return Err(syn::Error::new_spanned(
                        &parsed.kind,
                        format!("kind {relation} is declared twice"),
                    ));
                }
                kinds.push(KindDelta {
                    kind: ArcRelation::new(relation),
                    structural: structural.expect("checked above"),
                    text: text.expect("checked above"),
                    existence: existence.expect("checked above"),
                });
            }
        }
    }

    if kinds.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "#[marking_delta] with no kind clause says nothing that Undeclared does not already \
             say — omit the attribute or name a kind",
        ));
    }

    Ok(match varies_by {
        Some(varies_by) => MarkingDelta::Envelope { kinds, varies_by },
        None => MarkingDelta::Static { kinds },
    })
}

/// Emit the parsed delta as a literal constructor expression — plain
/// serializable data, for the same dual-consumer reason as the arcs.
fn marking_delta_tokens(delta: &MarkingDelta) -> proc_macro2::TokenStream {
    let kind_exprs = |kinds: &Vec<KindDelta>| {
        let exprs: Vec<_> = kinds
            .iter()
            .map(|k| {
                let relation = k.kind.as_str();
                let structural = format_ident!("{}", structural_variant(k.structural));
                let text = format_ident!("{}", text_variant(k.text));
                let existence = format_ident!("{}", existence_variant(k.existence));
                quote! {
                    holon_api::marking::KindDelta {
                        kind: holon_api::arcs::ArcRelation::new(#relation),
                        structural: holon_api::marking::StructuralFlow::#structural,
                        text: holon_api::marking::TextFlow::#text,
                        existence: holon_api::marking::ExistenceFlow::#existence,
                    }
                }
            })
            .collect();
        quote! { vec![#(#exprs),*] }
    };

    match delta {
        MarkingDelta::Undeclared => quote! { holon_api::marking::MarkingDelta::Undeclared },
        MarkingDelta::Static { kinds } => {
            let kinds = kind_exprs(kinds);
            quote! { holon_api::marking::MarkingDelta::Static { kinds: #kinds } }
        }
        MarkingDelta::Envelope { kinds, varies_by } => {
            let kinds = kind_exprs(kinds);
            let params = varies_by.iter().map(|p| quote! { #p.to_string() });
            quote! {
                holon_api::marking::MarkingDelta::Envelope {
                    kinds: #kinds,
                    varies_by: vec![#(#params),*],
                }
            }
        }
    }
}

fn structural_variant(flow: StructuralFlow) -> &'static str {
    match flow {
        StructuralFlow::Untouched => "Untouched",
        StructuralFlow::Reads => "Reads",
        StructuralFlow::Produces => "Produces",
        StructuralFlow::Consumes => "Consumes",
        StructuralFlow::Relocates => "Relocates",
    }
}

fn text_variant(flow: TextFlow) -> &'static str {
    match flow {
        TextFlow::Untouched => "Untouched",
        TextFlow::Reads => "Reads",
        TextFlow::Produces => "Produces",
    }
}

fn existence_variant(flow: ExistenceFlow) -> &'static str {
    match flow {
        ExistenceFlow::Untouched => "Untouched",
        ExistenceFlow::Reads => "Reads",
        ExistenceFlow::Produces => "Produces",
    }
}

fn arc_place_tokens(place: &ArcPlace) -> proc_macro2::TokenStream {
    let relation = place.relation.as_str();
    let field = &place.field;
    quote! {
        holon_api::arcs::ArcPlace::new(#relation, #field)
    }
}

fn pattern_tokens(pattern: &Pattern) -> proc_macro2::TokenStream {
    match pattern {
        Pattern::HasTag(tag) => quote! { holon_api::pattern::Pattern::HasTag(#tag.to_string()) },
        Pattern::BlockExists(path) => {
            let segs = path.segments.iter().map(|s| match s {
                PathSegment::Lit(l) => {
                    quote! { holon_api::pattern::PathSegment::Lit(#l.to_string()) }
                }
                PathSegment::Builtin(BuiltinRef::Today) => quote! {
                    holon_api::pattern::PathSegment::Builtin(
                        holon_api::pattern::BuiltinRef::Today,
                    )
                },
            });
            quote! {
                holon_api::pattern::Pattern::BlockExists(holon_api::pattern::PathPattern {
                    segments: vec![#(#segs),*],
                })
            }
        }
        Pattern::Parent(inner) => {
            let inner = pattern_tokens(inner);
            quote! { holon_api::pattern::Pattern::Parent(Box::new(#inner)) }
        }
        Pattern::Not(inner) => {
            let inner = pattern_tokens(inner);
            quote! { holon_api::pattern::Pattern::Not(Box::new(#inner)) }
        }
        Pattern::And(ps) => {
            let ps = ps.iter().map(pattern_tokens);
            quote! { holon_api::pattern::Pattern::And(vec![#(#ps),*]) }
        }
        Pattern::Or(ps) => {
            let ps = ps.iter().map(pattern_tokens);
            quote! { holon_api::pattern::Pattern::Or(vec![#(#ps),*]) }
        }
        // The guard-string grammar has no field syntax, and the parser is the
        // only producer of the patterns reaching here.
        Pattern::Field { .. } => unreachable!(
            "Pattern::Field is unreachable from a #[require] guard string: the grammar is \
             not/and/or over block_exists, has_tag and parent"
        ),
    }
}

/// Extract parameter name from pattern
pub fn extract_param_name(pat: &Pat) -> String {
    match pat {
        Pat::Ident(pat_ident) => pat_ident.ident.to_string(),
        Pat::Wild(_) => "_".to_string(),
        _ => quote! { #pat }.to_string(),
    }
}

/// The wire/operation name for a parameter: its Rust identifier with a single
/// leading underscore stripped. A leading underscore is the Rust "intentionally
/// unused" idiom (e.g. a param required by the trait shape but unused by a
/// `#[require(...)]` precondition). It must not leak into the generated
/// OperationDescriptor param name or the StorageEntity keys used for dispatch
/// and precondition lookup — otherwise a caller passing the logical name (`id`)
/// could never match a binding generated from `_id`. The binding identifier
/// keeps the underscore (so the unused-variable lint stays suppressed); only
/// the key/name string is normalized here. The wildcard `_` is left untouched.
fn param_wire_name(ident_name: &str) -> String {
    ident_name
        .strip_prefix('_')
        .filter(|stripped| !stripped.is_empty())
        .unwrap_or(ident_name)
        .to_string()
}

/// Infer type string and required flag from Rust type
pub fn infer_type(ty: &Type) -> (String, bool) {
    // Handle reference types: &str, &T
    if let Type::Reference(ref_type) = ty {
        return infer_type(&ref_type.elem);
    }

    // Handle Option<T> via AST path matching
    if let Type::Path(type_path) = ty
        && let Some(segment) = type_path.path.segments.last()
        && segment.ident == "Option"
        && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        && let Some(syn::GenericArgument::Type(inner_ty)) = args.args.first()
    {
        let (inner_type, _) = infer_type(inner_ty);
        return (inner_type, false);
    }

    let type_name = extract_type_name(ty);
    (infer_type_string(&type_name), true)
}

/// Extract the leaf type name from a syn::Type using AST matching.
fn extract_type_name(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                segment.ident.to_string()
            } else {
                quote! { #ty }.to_string().replace(" ", "")
            }
        }
        Type::Reference(ref_type) => extract_type_name(&ref_type.elem),
        _ => quote! { #ty }.to_string().replace(" ", ""),
    }
}

/// Infer type string from cleaned type name
fn infer_type_string(type_str: &str) -> String {
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
        s if s.contains("StorageEntity") => "HashMap".to_string(),
        s if s.contains("HashMap") => "HashMap".to_string(),
        s if s.contains("Vec") => "Vec".to_string(),
        s if s.contains("DateTime") => "DateTime".to_string(),
        s if s.contains("Value") => "Value".to_string(),
        _ => type_str.to_string(),
    }
}

/// Parse parameter type hint with entity ID detection
fn parse_param_type_hint(
    param_name: &str,
    attrs: &[syn::Attribute],
    rust_type_str: &str,
) -> proc_macro2::TokenStream {
    let mut entity_ref_override: Option<String> = None;
    let mut not_entity = false;

    for attr in attrs {
        if attr.path().is_ident("entity_ref")
            && let Meta::List(meta_list) = &attr.meta
        {
            let tokens = &meta_list.tokens;
            let token_str = quote! { #tokens }.to_string();
            if let Some(stripped) = token_str
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
            {
                entity_ref_override = Some(stripped.to_string());
            }
        }

        if attr.path().is_ident("not_entity") {
            not_entity = true;
        }
    }

    if let Some(entity_name) = entity_ref_override {
        quote! {
            holon_api::TypeHint::EntityId {
                entity_name: holon_api::EntityName::new(#entity_name),
            }
        }
    } else if not_entity {
        infer_type_hint_from_rust_type(rust_type_str)
    } else if let Some(entity_name) = param_name.strip_suffix("_id").filter(|s| !s.is_empty()) {
        let entity_name_lit = entity_name.to_string();
        quote! {
            holon_api::TypeHint::EntityId {
                entity_name: holon_api::EntityName::new(#entity_name_lit),
            }
        }
    } else {
        infer_type_hint_from_rust_type(rust_type_str)
    }
}

/// Infer TypeHint from Rust type string
fn infer_type_hint_from_rust_type(rust_type_str: &str) -> proc_macro2::TokenStream {
    match rust_type_str {
        "String" | "&str" | "str" => {
            quote! { holon_api::TypeHint::String }
        }
        "bool" => {
            quote! { holon_api::TypeHint::Bool }
        }
        "i64" | "i32" | "u64" | "u32" | "usize" | "integer" => {
            quote! { holon_api::TypeHint::Number }
        }
        s if s.contains("DateTime") => {
            quote! { holon_api::TypeHint::String }
        }
        _ => {
            quote! { holon_api::TypeHint::String }
        }
    }
}

fn extract_affected_fields(attrs: &[syn::Attribute]) -> Vec<String> {
    attr_parser::extract_affected_fields(attrs)
}

fn extract_menu_exposure(attrs: &[syn::Attribute]) -> Option<String> {
    attr_parser::extract_menu_exposure(attrs)
}

fn extract_boundary_behavior(attrs: &[syn::Attribute]) -> Option<String> {
    attr_parser::extract_boundary_behavior(attrs)
}

/// Map a `#[boundary_behavior(<variant>)]` marker (or its absence) to the
/// `holon_api::BoundaryBehavior` construction tokens. Absent / unknown ⇒ the
/// fail-closed `Unclassified` behaviour (any boundary interaction rejected
/// loudly until an op deliberately classifies itself). ADR 0028 C3.
fn boundary_behavior_tokens(variant: Option<String>) -> proc_macro2::TokenStream {
    match variant.as_deref() {
        Some("private_only") => quote! { holon_api::BoundaryBehavior::PrivateOnly },
        Some("crossing_widens") => quote! {
            holon_api::BoundaryBehavior::Crossing { widens_audience: true }
        },
        Some("crossing_same_audience") => quote! {
            holon_api::BoundaryBehavior::Crossing { widens_audience: false }
        },
        Some("forbidden_at_page_boundary") => {
            quote! { holon_api::BoundaryBehavior::ForbiddenAtPageBoundary }
        }
        Some("policy_edit") => quote! { holon_api::BoundaryBehavior::PolicyEdit },
        Some("identity_op") => quote! { holon_api::BoundaryBehavior::IdentityOp },
        // A PRESENT attr with an unknown variant is a typo, not an omission —
        // erroring here keeps fail-closed from silently absorbing misspellings.
        Some(unknown) => {
            let msg = format!(
                "unknown boundary_behavior variant `{unknown}` (expected one of: \
                 private_only, crossing_widens, crossing_same_audience, \
                 forbidden_at_page_boundary, policy_edit, identity_op)"
            );
            quote! { compile_error!(#msg) }
        }
        None => quote! { holon_api::BoundaryBehavior::Unclassified },
    }
}

/// Map a `#[menu_exposure(<variant>)]` marker (or its absence) to the
/// `holon_api::MenuExposure` construction tokens. Absent / unknown ⇒ the
/// fail-closed `ProviderDefault` surface (invisible to the menu until an op
/// deliberately declares `listed`).
fn menu_exposure_tokens(variant: Option<String>) -> proc_macro2::TokenStream {
    match variant.as_deref() {
        Some("listed") => quote! {
            holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet { slash_menu: true, action_bar: false },
            }
        },
        Some("action_bar") => quote! {
            holon_api::MenuExposure::Listed {
                surfaces: holon_api::SurfaceSet { slash_menu: true, action_bar: true },
            }
        },
        Some("keyboard_gesture") => quote! {
            holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::KeyboardGesture,
            }
        },
        Some("pointer_gesture") => quote! {
            holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::PointerGesture,
            }
        },
        Some("navigation") => quote! {
            holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Navigation,
            }
        },
        Some("external") => quote! {
            holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::External,
            }
        },
        Some("internal") => quote! {
            holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::Internal,
            }
        },
        _ => quote! {
            holon_api::MenuExposure::NotListed {
                surface: holon_api::NonMenuSurface::ProviderDefault,
            }
        },
    }
}

#[cfg(test)]
mod arc_attribute_tests {
    use super::*;

    fn arcs_of(attrs: Vec<syn::Attribute>) -> syn::Result<TransitionArcs> {
        extract_transition_arcs(&attrs)
    }

    /// A typo'd place is a COMPILE ERROR at the literal, not a declaration that
    /// is silently true forever. This is the macro-side half of the closed
    /// field list; `holon-pattern` owns the vocabulary itself.
    #[test]
    fn an_unknown_field_is_a_macro_error_naming_the_place() {
        let err = arcs_of(vec![
            syn::parse_quote!(#[emits("block.totally_bogus_field_xyz")]),
        ])
        .expect_err("an unknown place must not expand");
        let msg = err.to_string();
        assert!(
            msg.contains("totally_bogus_field_xyz") && msg.contains("has no place"),
            "the error must name the offending place: {msg}"
        );
    }

    #[test]
    fn an_unknown_relation_is_a_macro_error() {
        let err = arcs_of(vec![syn::parse_quote!(#[reads("document.title")])])
            .expect_err("an unknown relation must not expand");
        assert!(err.to_string().contains("unknown arc relation"));
    }

    /// Declaring inputs while staying silent about outputs is what `Undeclared`
    /// already says, so the half-declaration is refused.
    #[test]
    fn reads_without_emits_is_refused() {
        let err = arcs_of(vec![syn::parse_quote!(#[reads("block.content")])])
            .expect_err("reads alone must not expand");
        assert!(err.to_string().contains("must also declare #[emits]"));
    }

    /// An exclusion without a reason is silence wearing a declaration's
    /// clothes.
    #[test]
    fn excluded_without_a_reason_is_refused() {
        let err = arcs_of(vec![
            syn::parse_quote!(#[emits(excluded("block.sort_key"))]),
        ])
        .expect_err("a reasonless exclusion must not expand");
        assert!(err.to_string().contains("requires a REASON"));
    }

    #[test]
    fn absent_attributes_yield_undeclared() {
        assert_eq!(
            arcs_of(vec![syn::parse_quote!(#[doc = "unrelated"])]).expect("no arc attrs"),
            TransitionArcs::Undeclared
        );
    }
}
