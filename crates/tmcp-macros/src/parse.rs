//! Parsing of annotated impl blocks, attributes, and tool signatures.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Expr, ExprLit, ImplItem, ItemImpl, Lit, Meta, Result, ext::IdentExt, punctuated::Punctuated,
    spanned::Spanned,
};

use crate::model::{
    FlatParam, FreeToolInfo, GroupMeta, GroupMethod, ParamsKind, ServerInfo, TaskParamKind,
    ToolAttrs, ToolMethod, ToolReturnKind, ToolTaskSupport,
};

/// Collect doc comment strings from attributes, preserving paragraph breaks.
///
/// Consecutive non-blank doc lines are joined with `\n`; runs of blank doc
/// lines become a single blank line so paragraphs stay separated.
pub fn extract_doc_comment(attrs: &[syn::Attribute]) -> String {
    let mut docs: Vec<String> = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(meta) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &meta.value
        {
            let doc = s.value();
            let doc = doc.trim();
            if doc.is_empty() {
                if docs.last().is_some_and(|line| !line.is_empty()) {
                    docs.push(String::new());
                }
            } else {
                docs.push(doc.to_string());
            }
        }
    }
    while docs.last().is_some_and(|line| line.is_empty()) {
        docs.pop();
    }
    docs.join("\n")
}

/// Determine if the type is `&ServerCtx` (or a path that ends in ServerCtx).
pub fn is_server_ctx_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Reference(reference) => is_server_ctx_type(&reference.elem),
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident == "ServerCtx")
            .unwrap_or(false),
        _ => false,
    }
}

/// Determine if the type is the unit `()`.
pub fn is_unit_type(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Tuple(tuple) if tuple.elems.is_empty())
}

/// Check whether a type is `schema::CallToolResult`.
pub fn is_call_tool_result_type(ty: &syn::Type) -> bool {
    type_path_ends_with(ty, "CallToolResult")
}

/// Check whether a type is `schema::CreateTaskResult`.
fn is_create_task_result_type(ty: &syn::Type) -> bool {
    type_path_ends_with(ty, "CreateTaskResult")
}

/// Check whether a type is `schema::CallToolResponse`.
fn is_call_tool_response_type(ty: &syn::Type) -> bool {
    type_path_ends_with(ty, "CallToolResponse")
}

/// Check whether a type is `schema::TaskMetadata`.
fn is_task_metadata_type(ty: &syn::Type) -> bool {
    type_path_ends_with(ty, "TaskMetadata")
}

/// Check whether a type is `Option<schema::TaskMetadata>`.
fn is_option_task_metadata_type(ty: &syn::Type) -> bool {
    is_option_of(ty, is_task_metadata_type)
}

/// Return the final path segment identifier for a type.
fn type_path_last_ident(ty: &syn::Type) -> Option<&syn::Ident> {
    match ty {
        syn::Type::Reference(reference) => type_path_last_ident(&reference.elem),
        syn::Type::Path(type_path) => type_path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }
}

/// Return true when a type's final path segment matches the expected name.
pub fn type_path_ends_with(ty: &syn::Type, expected: &str) -> bool {
    type_path_last_ident(ty)
        .map(|ident| ident == expected)
        .unwrap_or(false)
}

/// Return true when a type is `String`.
pub fn is_string_type(ty: &syn::Type) -> bool {
    type_path_ends_with(ty, "String")
}

/// Return true when a type is `Option<T>` with an argument matching the predicate.
fn is_option_of(ty: &syn::Type, is_inner: fn(&syn::Type) -> bool) -> bool {
    let syn::Type::Path(type_path) = ty else {
        return false;
    };
    let Some(segment) = type_path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Option" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return false;
    };
    arguments
        .args
        .iter()
        .any(|argument| matches!(argument, syn::GenericArgument::Type(ty) if is_inner(ty)))
}

/// Return true when a type is `Option<Cursor>`.
pub fn is_option_cursor_type(ty: &syn::Type) -> bool {
    is_option_of(ty, |inner| type_path_ends_with(inner, "Cursor"))
}

/// Whether a parameter attribute is harvested onto generated flat-argument structs.
fn is_flat_param_attr(attr: &syn::Attribute) -> bool {
    attr.path().is_ident("serde") || attr.path().is_ident("schemars") || attr.path().is_ident("doc")
}

/// Filter parameter attributes to those relevant for serde/schemars/docs.
fn filter_flat_param_attrs(attrs: &[syn::Attribute]) -> Vec<syn::Attribute> {
    attrs
        .iter()
        .filter(|attr| is_flat_param_attr(attr))
        .cloned()
        .collect()
}

/// Remove harvested parameter attributes from `#[tool]` methods in an impl block.
///
/// Serde, schemars, and doc attributes on tool parameters feed the generated
/// flat-argument structs; they are not legal on function parameters, so they
/// must not survive in the re-emitted impl block.
fn strip_tool_param_attrs(impl_block: &mut ItemImpl) {
    for item in &mut impl_block.items {
        let ImplItem::Fn(method) = item else { continue };
        if !method.attrs.iter().any(|attr| attr.path().is_ident("tool")) {
            continue;
        }
        method.attrs.retain(|attr| !attr.path().is_ident("tool"));
        for input in &mut method.sig.inputs {
            let syn::FnArg::Typed(pat_type) = input else {
                continue;
            };
            if is_server_ctx_type(&pat_type.ty) {
                continue;
            }
            pat_type.attrs.retain(|attr| !is_flat_param_attr(attr));
        }
    }
}

/// Parse a flat parameter definition from a typed function argument.
fn parse_flat_param(param: &syn::PatType) -> Result<FlatParam> {
    let ident = match param.pat.as_ref() {
        syn::Pat::Ident(pat_ident) if pat_ident.subpat.is_none() => pat_ident.ident.clone(),
        _ => {
            return Err(syn::Error::new(
                param.pat.span(),
                "flat tool parameters must be simple identifiers",
            ));
        }
    };

    Ok(FlatParam {
        ident,
        ty: (*param.ty).clone(),
        attrs: filter_flat_param_attrs(&param.attrs),
    })
}

/// Parse a boolean literal from an expression.
fn parse_bool_lit(expr: &Expr) -> Result<bool> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Bool(b), ..
    }) = expr
    {
        Ok(b.value())
    } else {
        Err(syn::Error::new(expr.span(), "expected a boolean literal"))
    }
}

/// Parse a string literal from an expression.
fn parse_string_lit(expr: &Expr) -> Result<String> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = expr
    {
        Ok(s.value())
    } else {
        Err(syn::Error::new(expr.span(), "expected a string literal"))
    }
}

/// Parse an identifier from a string literal or path expression.
pub fn parse_ident_from_expr(expr: &Expr) -> Result<syn::Ident> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => syn::parse_str::<syn::Ident>(&s.value()).map_err(|_| {
            syn::Error::new(expr.span(), "expected a valid identifier string literal")
        }),
        Expr::Path(path) => path
            .path
            .get_ident()
            .cloned()
            .ok_or_else(|| syn::Error::new(expr.span(), "expected an identifier")),
        _ => Err(syn::Error::new(
            expr.span(),
            "expected a string literal or identifier",
        )),
    }
}

/// Parse task support value from a string literal or identifier.
fn parse_tool_task_support(expr: &Expr) -> Result<ToolTaskSupport> {
    let value = match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(s), ..
        }) => s.value(),
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default(),
        _ => {
            return Err(syn::Error::new(
                expr.span(),
                "expected a string literal or identifier for task_support",
            ));
        }
    };
    match value.to_lowercase().as_str() {
        "forbidden" => Ok(ToolTaskSupport::Forbidden),
        "optional" => Ok(ToolTaskSupport::Optional),
        "required" => Ok(ToolTaskSupport::Required),
        _ => Err(syn::Error::new(
            expr.span(),
            "task_support must be \"forbidden\", \"optional\", or \"required\"",
        )),
    }
}

/// Parse icon strings from an attribute expression.
fn parse_icons_from_expr(expr: &Expr) -> Result<Vec<String>> {
    match expr {
        Expr::Array(array) => array.elems.iter().map(parse_string_lit).collect(),
        _ => Err(syn::Error::new(
            expr.span(),
            "icons must be an array of string literals",
        )),
    }
}

/// Parse an `output_schema` type path from an attribute expression.
fn parse_output_schema_type(expr: &Expr) -> Result<syn::Type> {
    match expr {
        Expr::Path(path) => Ok(syn::Type::Path(syn::TypePath {
            qself: None,
            path: path.path.clone(),
        })),
        Expr::Group(group) => parse_output_schema_type(group.expr.as_ref()),
        _ => Err(syn::Error::new(
            expr.span(),
            "output_schema must be a type path",
        )),
    }
}

/// Parse tool metadata from a #[tool(...)] attribute.
fn parse_tool_attrs(attrs: &[syn::Attribute]) -> Result<Option<ToolAttrs>> {
    let mut tool_attrs = ToolAttrs::default();
    let mut found = false;

    for attr in attrs {
        if !attr.path().is_ident("tool") {
            continue;
        }
        found = true;

        for meta in parse_attr_metas(attr, "#[tool]")? {
            match meta {
                Meta::Path(path) => {
                    if let Some(ident) = path.get_ident() {
                        match ident.to_string().as_str() {
                            "read_only" => tool_attrs.read_only = Some(true),
                            "destructive" => tool_attrs.destructive = Some(true),
                            "idempotent" => tool_attrs.idempotent = Some(true),
                            "open_world" => tool_attrs.open_world = Some(true),
                            "always" => tool_attrs.always = true,
                            "defaults" => tool_attrs.defaults = true,
                            "flat" => tool_attrs.flat = true,
                            _ => {
                                return Err(syn::Error::new(
                                    ident.span(),
                                    format!("Unknown #[tool] flag: {ident}"),
                                ));
                            }
                        }
                    } else {
                        return Err(syn::Error::new(path.span(), "invalid #[tool] flag"));
                    }
                }
                Meta::NameValue(meta) => {
                    let ident = meta.path.get_ident().ok_or_else(|| {
                        syn::Error::new(meta.path.span(), "invalid #[tool] argument")
                    })?;
                    match ident.to_string().as_str() {
                        "title" => {
                            tool_attrs.title = Some(parse_string_lit(&meta.value)?);
                        }
                        "read_only" => {
                            tool_attrs.read_only = Some(parse_bool_lit(&meta.value)?);
                        }
                        "destructive" => {
                            tool_attrs.destructive = Some(parse_bool_lit(&meta.value)?);
                        }
                        "idempotent" => {
                            tool_attrs.idempotent = Some(parse_bool_lit(&meta.value)?);
                        }
                        "open_world" => {
                            tool_attrs.open_world = Some(parse_bool_lit(&meta.value)?);
                        }
                        "task_support" => {
                            tool_attrs.task_support = Some(parse_tool_task_support(&meta.value)?);
                        }
                        "output_schema" => {
                            tool_attrs.output_schema = Some(parse_output_schema_type(&meta.value)?);
                        }
                        "icon" => {
                            tool_attrs.icons.push(parse_string_lit(&meta.value)?);
                        }
                        "icons" => {
                            tool_attrs.icons.extend(parse_icons_from_expr(&meta.value)?);
                        }
                        "defaults" => {
                            tool_attrs.defaults = parse_bool_lit(&meta.value)?;
                        }
                        "flat" => {
                            tool_attrs.flat = parse_bool_lit(&meta.value)?;
                        }
                        "always" => {
                            tool_attrs.always = parse_bool_lit(&meta.value)?;
                        }
                        _ => {
                            return Err(syn::Error::new(
                                ident.span(),
                                format!("Unknown #[tool] argument: {ident}"),
                            ));
                        }
                    }
                }
                Meta::List(list) => {
                    if list.path.is_ident("icons") {
                        let entries = list
                            .parse_args_with(Punctuated::<Expr, syn::Token![,]>::parse_terminated)
                            .map_err(|_| {
                                syn::Error::new(
                                    list.span(),
                                    "icons(...) must contain string literals",
                                )
                            })?;
                        for entry in entries {
                            tool_attrs.icons.push(parse_string_lit(&entry)?);
                        }
                    } else {
                        return Err(syn::Error::new(
                            list.span(),
                            "unsupported #[tool] list argument",
                        ));
                    }
                }
            }
        }
    }

    if found {
        Ok(Some(tool_attrs))
    } else {
        Ok(None)
    }
}

/// Parse the comma-separated metas of an attribute, allowing a bare path.
fn parse_attr_metas(attr: &syn::Attribute, label: &str) -> Result<Vec<Meta>> {
    match &attr.meta {
        Meta::Path(_) => Ok(Vec::new()),
        Meta::List(list) => Ok(list
            .parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)?
            .into_iter()
            .collect()),
        Meta::NameValue(meta) => Err(syn::Error::new(
            meta.span(),
            format!("{label} does not support name-value syntax"),
        )),
    }
}

/// Parse group metadata from `#[group(...)]` or `#[tmcp_group_meta(...)]` attributes.
///
/// This is the single grammar for group metadata: it is used both by
/// `#[derive(Group)]` (all keys) and by `#[group]` factory methods (which
/// accept only the `name` key).
pub fn parse_group_meta(attrs: &[syn::Attribute]) -> Result<Option<GroupMeta>> {
    let mut meta = GroupMeta::default();
    let mut found = false;

    for attr in attrs {
        if !(attr.path().is_ident("tmcp_group_meta") || attr.path().is_ident("group")) {
            continue;
        }
        found = true;

        for meta_item in parse_attr_metas(attr, "#[group]")? {
            match meta_item {
                Meta::NameValue(item) => {
                    let ident = item.path.get_ident().ok_or_else(|| {
                        syn::Error::new(item.path.span(), "invalid #[group] argument")
                    })?;
                    match ident.to_string().as_str() {
                        "name" => {
                            meta.name = Some(parse_string_lit(&item.value)?);
                        }
                        "description" => {
                            meta.description = Some(parse_string_lit(&item.value)?);
                        }
                        "show_deactivator" => {
                            meta.show_deactivator = Some(parse_bool_lit(&item.value)?);
                        }
                        "on_activate" => {
                            meta.on_activate = Some(parse_ident_from_expr(&item.value)?);
                        }
                        "on_deactivate" => {
                            meta.on_deactivate = Some(parse_ident_from_expr(&item.value)?);
                        }
                        _ => {
                            return Err(syn::Error::new(
                                ident.span(),
                                format!("Unknown #[group] argument: {ident}"),
                            ));
                        }
                    }
                }
                Meta::Path(path) => {
                    return Err(syn::Error::new(path.span(), "unsupported #[group] flag"));
                }
                Meta::List(list) => {
                    return Err(syn::Error::new(
                        list.span(),
                        "unsupported #[group] list argument",
                    ));
                }
            }
        }
    }

    Ok(found.then_some(meta))
}

/// Determine the tool return type kind.
fn parse_tool_return(output: &syn::ReturnType) -> Result<ToolReturnKind> {
    const RETURN_ERROR: &str = "tool methods must return Result<schema::CallToolResult>, \
        Result<schema::CreateTaskResult>, Result<schema::CallToolResponse>, or ToolResult";

    let ty = match output {
        syn::ReturnType::Type(_, ty) => ty.as_ref(),
        _ => {
            return Err(syn::Error::new(output.span(), RETURN_ERROR));
        }
    };

    let syn::Type::Path(type_path) = ty else {
        return Err(syn::Error::new(ty.span(), RETURN_ERROR));
    };

    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new(ty.span(), RETURN_ERROR));
    };

    match segment.ident.to_string().as_str() {
        "Result" => {
            let inner = match &segment.arguments {
                syn::PathArguments::AngleBracketed(args) => args.args.iter().find_map(|arg| {
                    if let syn::GenericArgument::Type(ty) = arg {
                        Some(ty)
                    } else {
                        None
                    }
                }),
                _ => None,
            }
            .ok_or_else(|| syn::Error::new(segment.span(), RETURN_ERROR))?;

            if is_call_tool_result_type(inner) {
                Ok(ToolReturnKind::CallResult)
            } else if is_create_task_result_type(inner) {
                Ok(ToolReturnKind::TaskResult)
            } else if is_call_tool_response_type(inner) {
                Ok(ToolReturnKind::CallResponse)
            } else {
                Err(syn::Error::new(inner.span(), RETURN_ERROR))
            }
        }
        "ToolResult" => {
            let output = match &segment.arguments {
                syn::PathArguments::AngleBracketed(args) => args.args.iter().find_map(|arg| {
                    if let syn::GenericArgument::Type(ty) = arg {
                        Some(ty.clone())
                    } else {
                        None
                    }
                }),
                _ => None,
            };
            Ok(ToolReturnKind::ToolResult {
                output: Box::new(output),
            })
        }
        _ => Err(syn::Error::new(segment.ident.span(), RETURN_ERROR)),
    }
}

/// Normalize task-support metadata implied by a task parameter.
fn normalize_task_support(
    signature: &syn::Signature,
    task_param: TaskParamKind,
    task_support: Option<ToolTaskSupport>,
) -> Result<Option<ToolTaskSupport>> {
    let implied = match task_param {
        TaskParamKind::None => {
            return match task_support {
                Some(ToolTaskSupport::Optional | ToolTaskSupport::Required) => {
                    Err(syn::Error::new(
                        signature.span(),
                        "task_support = \"optional\" or \"required\" requires a TaskMetadata or \
                         Option<TaskMetadata> parameter",
                    ))
                }
                declared => Ok(declared),
            };
        }
        TaskParamKind::Required => ToolTaskSupport::Required,
        TaskParamKind::Optional => ToolTaskSupport::Optional,
    };

    match task_support {
        None => Ok(Some(implied)),
        Some(declared) if declared == implied => Ok(Some(implied)),
        Some(_) => {
            let (param, requirement) = match implied {
                ToolTaskSupport::Required => ("TaskMetadata", "required"),
                _ => ("Option<TaskMetadata>", "optional"),
            };
            Err(syn::Error::new(
                signature.span(),
                format!("{param} parameters require task_support = \"{requirement}\""),
            ))
        }
    }
}

/// Parse the context, task, and ordinary parameters after a receiver or state argument.
fn parse_tool_tail(
    signature: &syn::Signature,
    params: &[&syn::FnArg],
    mut attrs: ToolAttrs,
) -> Result<(bool, TaskParamKind, ParamsKind, ToolReturnKind, ToolAttrs)> {
    let mut has_ctx = false;
    let mut start_index = 0;
    if let Some(syn::FnArg::Typed(pat_type)) = params.first()
        && is_server_ctx_type(pat_type.ty.as_ref())
    {
        has_ctx = true;
        start_index = 1;
    }

    let mut task_param = TaskParamKind::None;
    if params.len() > start_index
        && let syn::FnArg::Typed(pat_type) = params[start_index]
    {
        let ty = pat_type.ty.as_ref();
        if is_task_metadata_type(ty) {
            task_param = TaskParamKind::Required;
            start_index += 1;
        } else if is_option_task_metadata_type(ty) {
            task_param = TaskParamKind::Optional;
            start_index += 1;
        }
    }

    attrs.task_support = normalize_task_support(signature, task_param, attrs.task_support)?;

    let remaining = &params[start_index..];
    let params_kind = if remaining.is_empty() {
        if attrs.flat {
            return Err(syn::Error::new(
                signature.inputs.span(),
                "#[tool(flat)] requires at least one non-context parameter",
            ));
        }
        ParamsKind::None
    } else if remaining.len() == 1 {
        let syn::FnArg::Typed(pat_type) = remaining[0] else {
            return Err(syn::Error::new(
                remaining[0].span(),
                "parameter must be a typed parameter",
            ));
        };
        let ty = pat_type.ty.as_ref();
        if is_server_ctx_type(ty) {
            return Err(syn::Error::new(
                pat_type.ty.span(),
                "only one &ServerCtx parameter is allowed",
            ));
        }
        if is_unit_type(ty) {
            if attrs.flat {
                return Err(syn::Error::new(
                    pat_type.ty.span(),
                    "#[tool(flat)] cannot be used with unit parameters",
                ));
            }
            ParamsKind::Unit
        } else if attrs.flat {
            ParamsKind::Flat(vec![parse_flat_param(pat_type)?])
        } else {
            ParamsKind::Typed(Box::new(ty.clone()))
        }
    } else {
        let mut flat_params = Vec::new();
        for param in remaining {
            let syn::FnArg::Typed(pat_type) = param else {
                return Err(syn::Error::new(
                    param.span(),
                    "flat tool parameters must be typed",
                ));
            };
            let ty = pat_type.ty.as_ref();
            if is_server_ctx_type(ty) {
                return Err(syn::Error::new(
                    pat_type.ty.span(),
                    "only one &ServerCtx parameter is allowed",
                ));
            }
            if is_unit_type(ty) {
                return Err(syn::Error::new(
                    pat_type.ty.span(),
                    "unit parameters are not supported in flat tool signatures",
                ));
            }
            flat_params.push(parse_flat_param(pat_type)?);
        }
        ParamsKind::Flat(flat_params)
    };

    let return_kind = parse_tool_return(&signature.output)?;
    Ok((has_ctx, task_param, params_kind, return_kind, attrs))
}

/// Validate that the receiver of a dispatched method is `&self`.
fn validate_self_receiver(params: &[&syn::FnArg], role: &str) -> Result<()> {
    match params.first() {
        Some(syn::FnArg::Receiver(receiver))
            if receiver.reference.is_some() && receiver.mutability.is_none() =>
        {
            Ok(())
        }
        Some(syn::FnArg::Receiver(receiver)) => Err(syn::Error::new(
            receiver.span(),
            format!("{role} methods take &self; use interior mutability for state"),
        )),
        Some(arg) => Err(syn::Error::new(arg.span(), "first parameter must be &self")),
        None => unreachable!("callers check for an empty parameter list"),
    }
}

/// Parse a tool method from an impl item if it has a #[tool] attribute.
pub fn parse_tool_method(method: &syn::ImplItemFn) -> Result<Option<ToolMethod>> {
    let Some(attrs) = parse_tool_attrs(&method.attrs)? else {
        return Ok(None);
    };

    let ident = method.sig.ident.clone();
    let name = ident.unraw().to_string();
    let docs = extract_doc_comment(&method.attrs);

    // Validate method signature
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            "tool methods must be async",
        ));
    }

    // Check parameters
    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.is_empty() {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            "tool methods must take &self",
        ));
    }

    // Validate the receiver: generated dispatch calls tools through &self.
    validate_self_receiver(&params, "tool")?;

    let (has_ctx, task_param, params_kind, return_kind, attrs) =
        parse_tool_tail(&method.sig, &params[1..], attrs)?;

    Ok(Some(ToolMethod {
        ident,
        name,
        docs,
        has_ctx,
        task_param,
        params_kind,
        return_kind,
        attrs,
    }))
}

/// Parse a free function tagged as a delegated tool.
pub fn parse_free_tool_function(
    attr: &proc_macro2::TokenStream,
    function: &syn::ItemFn,
) -> Result<FreeToolInfo> {
    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            function.sig.generics.span(),
            "delegated tool functions must not be generic",
        ));
    }
    if function.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            function.sig.span(),
            "delegated tool functions must be async",
        ));
    }
    let synthetic: syn::Attribute = if attr.is_empty() {
        syn::parse_quote!(#[tool])
    } else {
        syn::parse_quote!(#[tool(#attr)])
    };
    let attrs = parse_tool_attrs(&[synthetic])?.expect("synthetic tool attribute");
    let params: Vec<_> = function.sig.inputs.iter().collect();
    let Some(syn::FnArg::Typed(state)) = params.first() else {
        return Err(syn::Error::new(
            function.sig.inputs.span(),
            "delegated tool functions must take a shared state reference first",
        ));
    };
    let syn::Type::Reference(reference) = state.ty.as_ref() else {
        return Err(syn::Error::new(
            state.ty.span(),
            "delegated tool state must be a shared reference",
        ));
    };
    if reference.mutability.is_some() {
        return Err(syn::Error::new(
            reference.span(),
            "delegated tool state must be a shared reference",
        ));
    }
    let (has_ctx, task_param, params_kind, return_kind, attrs) =
        parse_tool_tail(&function.sig, &params[1..], attrs)?;
    let ident = function.sig.ident.clone();
    Ok(FreeToolInfo {
        tool: ToolMethod {
            name: ident.unraw().to_string(),
            ident,
            docs: extract_doc_comment(&function.attrs),
            has_ctx,
            task_param,
            params_kind,
            return_kind,
            attrs,
        },
        state_ty: (*reference.elem).clone(),
        visibility: function.vis.clone(),
    })
}

/// Parse a group factory method from an impl item if it has a #[group] attribute.
pub fn parse_group_method(method: &syn::ImplItemFn) -> Result<Option<GroupMethod>> {
    let group_attrs: Vec<_> = method
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("group"))
        .cloned()
        .collect();
    let Some(meta) = parse_group_meta(&group_attrs)? else {
        return Ok(None);
    };

    if meta.description.is_some()
        || meta.show_deactivator.is_some()
        || meta.on_activate.is_some()
        || meta.on_deactivate.is_some()
    {
        return Err(syn::Error::new(
            method.sig.span(),
            "#[group] on methods only supports the `name` key; set other group metadata on \
             the group type's #[derive(Group)]",
        ));
    }

    if method.sig.asyncness.is_some() {
        return Err(syn::Error::new(
            method.sig.span(),
            "group methods must be synchronous",
        ));
    }

    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.is_empty() {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            "group methods must take &self",
        ));
    }

    // Validate the receiver: generated dispatch calls group factories through &self.
    validate_self_receiver(&params, "group")?;

    if params.len() > 1 {
        return Err(syn::Error::new(
            params[1].span(),
            "group methods must not take additional parameters",
        ));
    }

    if matches!(method.sig.output, syn::ReturnType::Default) {
        return Err(syn::Error::new(
            method.sig.output.span(),
            "group methods must return a group type",
        ));
    }

    Ok(Some(GroupMethod {
        ident: method.sig.ident.clone(),
        segment_override: meta.name,
    }))
}

/// Collect the type and const parameter identifiers declared on an impl block.
pub fn generic_param_idents(generics: &syn::Generics) -> Vec<&syn::Ident> {
    generics
        .params
        .iter()
        .filter_map(|param| match param {
            syn::GenericParam::Type(param) => Some(&param.ident),
            syn::GenericParam::Const(param) => Some(&param.ident),
            syn::GenericParam::Lifetime(_) => None,
        })
        .collect()
}

/// Check whether a token stream mentions any of the given identifiers.
pub fn tokens_mention_ident(tokens: TokenStream, idents: &[&syn::Ident]) -> bool {
    tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Ident(ident) => idents.iter().any(|target| **target == ident),
        proc_macro2::TokenTree::Group(group) => tokens_mention_ident(group.stream(), idents),
        _ => false,
    })
}

/// Reject flat tool parameters whose types mention impl generic parameters.
///
/// Flat parameters are lifted into a standalone arguments struct emitted next
/// to the impl block, where the impl's generic parameters are not in scope.
fn validate_flat_params(info: &ServerInfo) -> Result<()> {
    let idents = generic_param_idents(&info.generics);
    if idents.is_empty() {
        return Ok(());
    }
    for tool in &info.tools {
        let ParamsKind::Flat(params) = &tool.params_kind else {
            continue;
        };
        for param in params {
            let ty = &param.ty;
            if tokens_mention_ident(quote! { #ty }, &idents) {
                return Err(syn::Error::new(
                    param.ty.span(),
                    "flat tool parameters cannot use the impl block's generic parameters; \
                     use a typed params struct instead",
                ));
            }
        }
    }
    Ok(())
}

/// Parse a server impl block and gather tool metadata.
pub fn parse_impl_block(input: &TokenStream) -> Result<(ItemImpl, ServerInfo)> {
    let mut impl_block = syn::parse2::<ItemImpl>(input.clone())?;

    // Extract the server type name from the last path segment of the self type.
    let struct_name = match &*impl_block.self_ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new(impl_block.self_ty.span(), "Invalid type name"))?
            .ident
            .unraw()
            .to_string(),
        _ => {
            return Err(syn::Error::new(
                impl_block.self_ty.span(),
                "Expected a struct or type name",
            ));
        }
    };
    let self_ty = (*impl_block.self_ty).clone();
    let generics = impl_block.generics.clone();

    // Extract description from doc comment
    let description = extract_doc_comment(&impl_block.attrs);

    // Extract tool and group methods
    let mut tools = Vec::new();
    let mut groups = Vec::new();
    for item in &impl_block.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        let tool = parse_tool_method(method)?;
        let group = parse_group_method(method)?;
        if tool.is_some() && group.is_some() {
            return Err(syn::Error::new(
                method.sig.span(),
                "methods cannot be both #[tool] and #[group]",
            ));
        }
        if let Some(tool) = tool {
            tools.push(tool);
        } else if let Some(group) = group {
            groups.push(group);
        }
    }

    let mut seen_names = HashSet::new();
    for tool in &tools {
        if !seen_names.insert(tool.name.as_str()) {
            return Err(syn::Error::new(
                tool.ident.span(),
                format!("duplicate tool name `{}`", tool.name),
            ));
        }
    }

    strip_tool_param_attrs(&mut impl_block);

    let info = ServerInfo {
        self_ty,
        generics,
        struct_name,
        description,
        tools,
        groups,
    };
    validate_flat_params(&info)?;

    Ok((impl_block, info))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_doc_extraction() {
        let attrs = vec![
            syn::parse_quote! { #[doc = " First line"] },
            syn::parse_quote! { #[doc = " Second line"] },
            syn::parse_quote! { #[doc = ""] },
            syn::parse_quote! { #[doc = " Third line"] },
        ];

        let result = extract_doc_comment(&attrs);
        assert_eq!(result, "First line\nSecond line\n\nThird line");
    }

    #[test]
    fn test_doc_extraction_trims_and_collapses_blank_lines() {
        let attrs = vec![
            syn::parse_quote! { #[doc = ""] },
            syn::parse_quote! { #[doc = " Paragraph one"] },
            syn::parse_quote! { #[doc = ""] },
            syn::parse_quote! { #[doc = ""] },
            syn::parse_quote! { #[doc = " Paragraph two"] },
            syn::parse_quote! { #[doc = ""] },
        ];

        let result = extract_doc_comment(&attrs);
        assert_eq!(result, "Paragraph one\n\nParagraph two");
    }

    #[test]
    fn test_parse_tool_method_valid() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            /// This is a test tool
            async fn test_tool(&self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method).unwrap();
        assert!(result.is_some());

        let tool = result.unwrap();
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.docs, "This is a test tool");
    }

    #[test]
    fn test_parse_tool_method_not_async() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            fn test_tool(&mut self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be async"));
    }

    #[test]
    fn test_parse_tool_method_wrong_params() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, context: &ServerCtx, other: &ServerCtx) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("only one &ServerCtx parameter is allowed")
        );
    }

    #[test]
    fn test_parse_tool_method_flat_params() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, _ctx: &ServerCtx, a: i32, b: i32) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let tool = parse_tool_method(&method).unwrap().unwrap();
        assert!(matches!(tool.params_kind, ParamsKind::Flat(_)));
    }

    #[test]
    fn test_parse_tool_method_flat_single_arg() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool(flat)]
            async fn test_tool(&self, value: String) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let tool = parse_tool_method(&method).unwrap().unwrap();
        assert!(matches!(tool.params_kind, ParamsKind::Flat(_)));
    }

    #[test]
    fn test_parse_tool_method_flat_pattern_error() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool(flat)]
            async fn test_tool(&self, _ctx: &ServerCtx, (a, b): (i32, i32)) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("flat tool parameters must be simple identifiers")
        );
    }

    #[test]
    fn test_parse_tool_method_rejects_mut_self() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };
        assert!(parse_tool_method(&method).unwrap().is_some());

        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&mut self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };
        let err = parse_tool_method(&method).unwrap_err();
        assert!(
            err.to_string()
                .contains("tool methods take &self; use interior mutability for state")
        );
    }

    #[test]
    fn test_parse_tool_method_raw_ident() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn r#move(&self, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let tool = parse_tool_method(&method).unwrap().unwrap();
        assert_eq!(tool.name, "move");
        assert_eq!(tool.ident.to_string(), "r#move");
    }

    #[test]
    fn test_task_support_requires_task_param() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool(task_support = "optional")]
            async fn test_tool(&self, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let err = parse_tool_method(&method).unwrap_err();
        assert!(err.to_string().contains("requires a TaskMetadata"));
    }

    #[test]
    fn test_task_param_implies_task_support() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, task: schema::TaskMetadata, params: TestParams) -> Result<schema::CreateTaskResult> {
                unimplemented!()
            }
        };
        let tool = parse_tool_method(&method).unwrap().unwrap();
        assert_eq!(tool.attrs.task_support, Some(ToolTaskSupport::Required));

        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool(task_support = "required")]
            async fn test_tool(&self, task: Option<schema::TaskMetadata>, params: TestParams) -> Result<schema::CallToolResponse> {
                unimplemented!()
            }
        };
        let err = parse_tool_method(&method).unwrap_err();
        assert!(
            err.to_string()
                .contains("Option<TaskMetadata> parameters require task_support = \"optional\"")
        );
    }

    #[test]
    fn test_group_method_rejects_derive_only_keys() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[group(description = "nope")]
            fn child(&self) -> Child {
                Child
            }
        };

        let err = parse_group_method(&method).unwrap_err();
        assert!(err.to_string().contains("only supports the `name` key"));
    }

    #[test]
    fn test_duplicate_tool_names_rejected() {
        let input = quote! {
            impl TestServer {
                #[tool]
                async fn echo(&self, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }

                #[tool]
                async fn echo(&self, params: OtherParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let err = parse_impl_block(&input).unwrap_err();
        assert!(err.to_string().contains("duplicate tool name `echo`"));
    }

    #[test]
    fn test_flat_param_attrs_stripped_from_impl() {
        let input = quote! {
            impl TestServer {
                #[tool]
                async fn label(
                    &self,
                    /// The label text
                    text: String,
                    #[serde(default)] count: i64,
                ) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let (impl_block, info) = parse_impl_block(&input).unwrap();

        let ParamsKind::Flat(params) = &info.tools[0].params_kind else {
            panic!("expected flat params");
        };
        assert_eq!(params[0].attrs.len(), 1);
        assert_eq!(params[1].attrs.len(), 1);

        let impl_str = quote! { #impl_block }.to_string();
        assert!(!impl_str.contains("serde"));
        assert!(!impl_str.contains("The label text"));
    }

    #[test]
    fn test_parse_impl_block() {
        let input = quote! {
            /// Test server implementation
            impl TestServer {
                #[tool]
                /// Echo tool
                async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }

                #[tool]
                async fn ping(&self, context: &ServerCtx, params: PingParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }

                // This method should be ignored
                async fn helper(&self) -> String {
                    "helper".to_string()
                }
            }
        };

        let (_, info) = parse_impl_block(&input).unwrap();
        assert_eq!(info.struct_name, "TestServer");
        assert_eq!(info.description, "Test server implementation");
        assert_eq!(info.tools.len(), 2);
        assert!(info.groups.is_empty());
        assert_eq!(info.tools[0].name, "echo");
        assert_eq!(info.tools[0].docs, "Echo tool");
        assert_eq!(info.tools[1].name, "ping");
    }

    #[test]
    fn test_parse_tool_method_no_params() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, _context: &ServerCtx) -> ToolResult {
                Ok(schema::CallToolResult::new())
            }
        };

        let tool = parse_tool_method(&method).unwrap().unwrap();
        assert!(matches!(tool.params_kind, ParamsKind::None));
        assert!(matches!(
            tool.return_kind,
            ToolReturnKind::ToolResult { .. }
        ));
    }

    #[test]
    fn test_parse_tool_method_unit_params() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, _context: &ServerCtx, _params: ()) -> ToolResult {
                Ok(schema::CallToolResult::new())
            }
        };

        let tool = parse_tool_method(&method).unwrap().unwrap();
        assert!(matches!(tool.params_kind, ParamsKind::Unit));
        assert!(matches!(
            tool.return_kind,
            ToolReturnKind::ToolResult { .. }
        ));
    }
}
