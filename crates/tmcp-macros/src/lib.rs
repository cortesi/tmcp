//! Macros to make defining MCP servers easier
//!
//! Using the #[mcp_server] macro on an impl block, this crate will pick up all methods
//! marked with #[tool] and derive the necessary ServerHandler::call_tool, ServerHandler::list_tools and
//! ServerHandler::initialize methods. The name of the server is derived from the name of the struct
//! converted to snake_case (e.g., MyServer becomes my_server), and the description is derived
//! from the doc comment on the impl block. The version is set to "0.1.0" by default.
//!
//! The macro supports customization through attributes:
//! - `initialize_fn`: Specify a custom initialize function instead of using the default
//! - `name`: Override the server name used in initialization
//! - `version`: Override the server version used in initialization
//! - `instructions`: Override the server instructions used in initialization
//! - `protocol_version`: Control protocol version negotiation ("latest" or "client")
//!
//! All tool methods must be async and have one of the following signatures:
//!
//! ```ignore
//! async fn tool_name(&self, context: &ServerCtx, params: ToolParams) -> Result<schema::CallToolResult>
//! async fn tool_name(&self, context: &ServerCtx, params: ToolParams) -> ToolResult<T>
//! async fn tool_name(&self, context: &ServerCtx) -> ToolResult<T>
//! async fn tool_name(&self, params: ToolParams) -> ToolResult<T>
//! async fn tool_name(&self) -> ToolResult<T>
//! async fn tool_name(&self, context: &ServerCtx, a: Type1, b: Type2) -> ToolResult<T>
//! async fn tool_name(&self, a: Type1, b: Type2) -> ToolResult<T>
//! ```
//!
//! The `context: &ServerCtx` parameter is optional. Tools may also omit parameters or
//! accept `()` for parameter-less tools.
//! Single-argument tools still default to struct-style parameters; use `#[tool(flat)]`
//! to force flat argument handling for one-argument tools.
//!
//! The parameter struct (ToolParams in this example) must implement `schemars::JsonSchema` and
//! `serde::Deserialize`.
//!
//! The `#[tool]` attribute can also accept metadata that feeds into tool annotations and execution
//! hints. Supported arguments:
//! - `read_only`, `destructive`, `idempotent`, `open_world` (or `read_only = true/false`, etc.)
//! - `title = "..."` (tool display title)
//! - `task_support = "forbidden" | "optional" | "required"`
//! - `output_schema = TypeName` (explicit output schema)
//! - `icon = "https://..."` or `icons("a", "b")`
//! - `defaults` (treat missing arguments as an empty object and rely on serde defaults)
//! - `flat` (force flat handling for single-argument tools)
//!
//! Example usage:
//!
//! ```ignore
//! use tmcp::{ServerCtx, schema};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
//! struct EchoParams {
//!     /// The message to echo back
//!     message: String,
//! }
//!
//! /// Basic server connection that provides an echo tool
//! #[derive(Debug, Default)]
//! struct Basic {}
//!
//! #[mcp_server]
//! /// This is the description field for the server.
//! impl Basic {
//!     #[tool]
//!     async fn echo(
//!         &self,
//!         context: &ServerCtx,
//!         params: EchoParams,
//!     ) -> Result<schema::CallToolResult> {
//!         Ok(schema::CallToolResult::new().with_text_content(params.message))
//!     }
//! }
//! ```
//!
//! Example with custom initialize function:
//!
//! ```ignore
//! #[mcp_server(initialize_fn = my_custom_initialize)]
//! impl MyServer {
//!     async fn my_custom_initialize(
//!         &self,
//!         context: &ServerCtx,
//!         protocol_version: String,
//!         capabilities: schema::ClientCapabilities,
//!         client_info: schema::Implementation,
//!     ) -> Result<schema::InitializeResult> {
//!         // Custom initialization logic
//!         Ok(schema::InitializeResult {
//!             protocol_version: schema::LATEST_PROTOCOL_VERSION.to_string(),
//!             capabilities: schema::ServerCapabilities {
//!                 tools: Some(schema::ToolsCapability {
//!                     list_changed: Some(true),
//!                 }),
//!                 ..Default::default()
//!             },
//!             server_info: schema::Implementation::new("my_custom_server", "2.0.0"),
//!             },
//!             instructions: Some("Custom server with advanced features".to_string()),
//!             meta: None,
//!         })
//!     }
//!
//!     #[tool]
//!     async fn my_tool(&self, context: &ServerCtx, params: MyParams) -> Result<schema::CallToolResult> {
//!         // Tool implementation
//!     }
//! }
//! ```

use std::result::Result as StdResult;

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Expr, ExprLit, ImplItem, ItemImpl, Lit, Meta, parse::Parse, punctuated::Punctuated,
    spanned::Spanned,
};

/// Internal result type for macro parsing.
type Result<T> = StdResult<T, syn::Error>;

#[derive(Debug)]
/// Description of an impl method tagged as a tool.
struct ToolMethod {
    /// Name of the tool method.
    name: String,
    /// Collected doc comments for the tool.
    docs: String,
    /// Whether the tool expects a ServerCtx parameter.
    has_ctx: bool,
    /// Parameter type metadata for the tool method.
    params_kind: ParamsKind,
    /// Tool return type kind for call routing.
    return_kind: ToolReturnKind,
    /// Parsed tool attribute metadata.
    attrs: ToolAttrs,
}

#[derive(Debug, Clone)]
/// A single flat parameter in a tool signature.
struct FlatParam {
    /// Identifier for the parameter.
    ident: syn::Ident,
    /// Type of the parameter.
    ty: syn::Type,
    /// Attributes to attach to the generated struct field.
    attrs: Vec<syn::Attribute>,
}

#[derive(Debug, Clone)]
/// Parameter shape for a tool method.
enum ParamsKind {
    /// Tool takes no parameters.
    None,
    /// Tool takes unit parameters (`()`).
    Unit,
    /// Tool takes a typed parameter struct.
    Typed(Box<syn::Type>),
    /// Tool takes flat parameters expanded from the signature.
    Flat(Vec<FlatParam>),
}

#[derive(Debug, Clone)]
/// Return type shape for a tool method.
enum ToolReturnKind {
    /// Tool returns `Result<CallToolResult>` (tmcp::Result).
    Result,
    /// Tool returns `ToolResult`.
    ToolResult {
        /// Optional output type for `ToolResult<T>`.
        output: Box<Option<syn::Type>>,
    },
}

#[derive(Debug, Default, Clone)]
/// Metadata parsed from a #[tool(...)] attribute.
struct ToolAttrs {
    /// Optional display title for the tool.
    title: Option<String>,
    /// Whether the tool should be treated as read-only.
    read_only: Option<bool>,
    /// Whether the tool is destructive.
    destructive: Option<bool>,
    /// Whether the tool is idempotent.
    idempotent: Option<bool>,
    /// Whether the tool can access open-world resources.
    open_world: Option<bool>,
    /// Task support requirements for the tool.
    task_support: Option<ToolTaskSupport>,
    /// Optional output schema override.
    output_schema: Option<syn::Type>,
    /// Icon URLs for the tool.
    icons: Vec<String>,
    /// Whether to apply default argument handling.
    defaults: bool,
    /// Whether to force flat handling for single-argument tools.
    flat: bool,
}

#[derive(Debug, Clone, Copy)]
/// Whether task support is forbidden, optional, or required.
enum ToolTaskSupport {
    /// Task metadata must not be provided.
    Forbidden,
    /// Task metadata may be provided.
    Optional,
    /// Task metadata must be provided.
    Required,
}

#[derive(Debug)]
/// Summary of the server impl block and its tool methods.
struct ServerInfo {
    /// Name of the server struct.
    struct_name: String,
    /// Doc comment used as the server description.
    description: String,
    /// Tool methods discovered in the impl block.
    tools: Vec<ToolMethod>,
}

#[derive(Debug, Default)]
/// Parsed macro arguments for #[mcp_server].
struct ServerMacroArgs {
    /// Optional custom initialize function name.
    initialize_fn: Option<syn::Ident>,
    /// Optional server name override.
    name: Option<Expr>,
    /// Optional server version override.
    version: Option<Expr>,
    /// Optional instructions override.
    instructions: Option<Expr>,
    /// Protocol version negotiation strategy.
    protocol_version: Option<ProtocolVersionStrategy>,
}

impl Parse for ServerMacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;

            if ident == "initialize_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.initialize_fn = Some(fn_name);
            } else if ident == "name" {
                let expr: Expr = input.parse()?;
                args.name = Some(expr);
            } else if ident == "version" {
                let expr: Expr = input.parse()?;
                args.version = Some(expr);
            } else if ident == "instructions" {
                let expr: Expr = input.parse()?;
                args.instructions = Some(expr);
            } else if ident == "protocol_version" {
                let expr: Expr = input.parse()?;
                args.protocol_version = Some(parse_protocol_version_strategy(&expr)?);
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Unknown argument: {ident}"),
                ));
            }

            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(args)
    }
}

#[derive(Debug, Clone, Copy)]
/// Strategy for selecting the protocol version to use.
enum ProtocolVersionStrategy {
    /// Always use the latest supported protocol version.
    Latest,
    /// Use the client's requested protocol version.
    Client,
}

/// Parse the protocol version strategy from a literal or identifier.
fn parse_protocol_version_strategy(expr: &Expr) -> Result<ProtocolVersionStrategy> {
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
                "protocol_version must be \"latest\" or \"client\"",
            ));
        }
    };
    match value.to_lowercase().as_str() {
        "latest" => Ok(ProtocolVersionStrategy::Latest),
        "client" | "requested" => Ok(ProtocolVersionStrategy::Client),
        _ => Err(syn::Error::new(
            expr.span(),
            "protocol_version must be \"latest\" or \"client\"",
        )),
    }
}

/// Collect and normalize doc comment strings from attributes.
fn extract_doc_comment(attrs: &[syn::Attribute]) -> String {
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(meta) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &meta.value
        {
            let doc = s.value();
            let doc = doc.trim();
            if !doc.is_empty() {
                docs.push(doc.to_string());
            }
        }
    }
    docs.join("\n")
}

/// Determine if the type is `&ServerCtx` (or a path that ends in ServerCtx).
fn is_server_ctx_type(ty: &syn::Type) -> bool {
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
fn is_unit_type(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Tuple(tuple) if tuple.elems.is_empty())
}

/// Check whether a type is `schema::CallToolResult`.
fn is_call_tool_result_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|segment| segment.ident == "CallToolResult")
            .unwrap_or(false),
        _ => false,
    }
}

/// Filter parameter attributes to those relevant for serde/schemars/docs.
fn filter_flat_param_attrs(attrs: &[syn::Attribute]) -> Vec<syn::Attribute> {
    attrs
        .iter()
        .filter(|attr| {
            attr.path().is_ident("serde")
                || attr.path().is_ident("schemars")
                || attr.path().is_ident("doc")
        })
        .cloned()
        .collect()
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

/// Generate a unique struct identifier for flat tool arguments.
fn flat_args_struct_ident(server_name: &str, tool_name: &str) -> syn::Ident {
    let server = server_name.trim_start_matches("r#");
    let tool = tool_name.trim_start_matches("r#");
    format_ident!("__TmcpToolArgs_{}_{}", server, tool)
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

/// Parse tool metadata from a #[tool(...)] attribute.
fn parse_tool_attrs(attrs: &[syn::Attribute]) -> Result<Option<ToolAttrs>> {
    let mut tool_attrs = ToolAttrs::default();
    let mut found = false;

    for attr in attrs {
        if !attr.path().is_ident("tool") {
            continue;
        }
        found = true;

        let metas = match &attr.meta {
            Meta::Path(_) => Punctuated::<Meta, syn::Token![,]>::new(),
            Meta::List(list) => {
                list.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)?
            }
            Meta::NameValue(meta) => {
                return Err(syn::Error::new(
                    meta.span(),
                    "#[tool] does not support name-value syntax",
                ));
            }
        };

        for meta in metas {
            match meta {
                Meta::Path(path) => {
                    if let Some(ident) = path.get_ident() {
                        match ident.to_string().as_str() {
                            "read_only" => tool_attrs.read_only = Some(true),
                            "destructive" => tool_attrs.destructive = Some(true),
                            "idempotent" => tool_attrs.idempotent = Some(true),
                            "open_world" => tool_attrs.open_world = Some(true),
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
                            let expr = &meta.value;
                            let ty = match expr {
                                Expr::Path(path) => syn::Type::Path(syn::TypePath {
                                    qself: None,
                                    path: path.path.clone(),
                                }),
                                Expr::Group(group) => {
                                    if let Expr::Path(path) = group.expr.as_ref() {
                                        syn::Type::Path(syn::TypePath {
                                            qself: None,
                                            path: path.path.clone(),
                                        })
                                    } else {
                                        return Err(syn::Error::new(
                                            group.span(),
                                            "output_schema must be a type path",
                                        ));
                                    }
                                }
                                _ => {
                                    return Err(syn::Error::new(
                                        expr.span(),
                                        "output_schema must be a type path",
                                    ));
                                }
                            };
                            tool_attrs.output_schema = Some(ty);
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

/// Determine the tool return type kind.
fn parse_tool_return(output: &syn::ReturnType) -> Result<ToolReturnKind> {
    let ty = match output {
        syn::ReturnType::Type(_, ty) => ty.as_ref(),
        _ => {
            return Err(syn::Error::new(
                output.span(),
                "tool methods must return Result<schema::CallToolResult> or ToolResult",
            ));
        }
    };

    let syn::Type::Path(type_path) = ty else {
        return Err(syn::Error::new(
            ty.span(),
            "tool methods must return Result<schema::CallToolResult> or ToolResult",
        ));
    };

    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new(
            ty.span(),
            "tool methods must return Result<schema::CallToolResult> or ToolResult",
        ));
    };

    match segment.ident.to_string().as_str() {
        "Result" => Ok(ToolReturnKind::Result),
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
        _ => Err(syn::Error::new(
            segment.ident.span(),
            "tool methods must return Result<schema::CallToolResult> or ToolResult",
        )),
    }
}

/// Parse a tool method from an impl item if it has a #[tool] attribute.
fn parse_tool_method(method: &syn::ImplItemFn) -> Result<Option<ToolMethod>> {
    let Some(attrs) = parse_tool_attrs(&method.attrs)? else {
        return Ok(None);
    };

    let name = method.sig.ident.to_string();
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
            "tool methods must include &self or &mut self",
        ));
    }

    // Validate &self or &mut self
    match params[0] {
        syn::FnArg::Receiver(_) => {
            // Accept both &self and &mut self
        }
        _ => {
            return Err(syn::Error::new(
                params[0].span(),
                "first parameter must be &self or &mut self",
            ));
        }
    }

    let mut has_ctx = false;
    let mut start_index = 1;
    if params.len() > 1 {
        match params[1] {
            syn::FnArg::Typed(pat_type) => {
                if is_server_ctx_type(pat_type.ty.as_ref()) {
                    has_ctx = true;
                    start_index = 2;
                }
            }
            _ => {
                return Err(syn::Error::new(
                    params[1].span(),
                    "second parameter must be &ServerCtx or a typed parameter",
                ));
            }
        }
    }

    let remaining = &params[start_index..];
    let params_kind = if remaining.is_empty() {
        if attrs.flat {
            return Err(syn::Error::new(
                method.sig.inputs.span(),
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

    let return_kind = parse_tool_return(&method.sig.output)?;

    Ok(Some(ToolMethod {
        name,
        docs,
        has_ctx,
        params_kind,
        return_kind,
        attrs,
    }))
}

/// Parse a server impl block and gather tool metadata.
fn parse_impl_block(input: &TokenStream) -> Result<(ItemImpl, ServerInfo)> {
    let impl_block = syn::parse2::<ItemImpl>(input.clone())?;

    // Extract struct name
    let struct_name = match &*impl_block.self_ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new(impl_block.self_ty.span(), "Invalid type name"))?
            .ident
            .to_string(),
        _ => {
            return Err(syn::Error::new(
                impl_block.self_ty.span(),
                "Expected a struct or type name",
            ));
        }
    };

    // Extract description from doc comment
    let description = extract_doc_comment(&impl_block.attrs);

    // Extract tool methods
    let mut tools = Vec::new();
    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item
            && let Some(tool) = parse_tool_method(method)?
        {
            tools.push(tool);
        }
    }

    Ok((
        impl_block,
        ServerInfo {
            struct_name,
            description,
            tools,
        },
    ))
}

/// Generate the ServerHandler::call_tool implementation.
fn generate_call_tool(info: &ServerInfo) -> TokenStream {
    let tool_matches = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let method = syn::Ident::new(name, proc_macro2::Span::call_site());
        let defaults = tool.attrs.defaults;
        let (args_prelude, call_expr) = match (&tool.has_ctx, &tool.params_kind) {
            (false, ParamsKind::None) => (
                quote! { let _ = arguments; },
                quote! { self.#method().await },
            ),
            (true, ParamsKind::None) => (
                quote! { let _ = arguments; },
                quote! { self.#method(context).await },
            ),
            (false, ParamsKind::Unit) => (
                quote! { let _ = arguments; },
                quote! { self.#method(()).await },
            ),
            (true, ParamsKind::Unit) => (
                quote! { let _ = arguments; },
                quote! { self.#method(context, ()).await },
            ),
            (has_ctx, ParamsKind::Typed(params_type)) => {
                let params_type = params_type.as_ref();
                let call = if *has_ctx {
                    quote! { self.#method(context, params).await }
                } else {
                    quote! { self.#method(params).await }
                };

                (
                    quote! {
                        let params: #params_type = match tmcp::Arguments::into_tool_params(
                            arguments,
                            #defaults,
                        ) {
                            Ok(params) => params,
                            Err(err) => {
                                return Ok(err.into());
                            }
                        };
                    },
                    call,
                )
            }
            (has_ctx, ParamsKind::Flat(params)) => {
                let struct_ident = flat_args_struct_ident(&info.struct_name, name);
                let param_idents: Vec<_> = params.iter().map(|param| &param.ident).collect();

                let call = if *has_ctx {
                    quote! { self.#method(context, #(#param_idents),*).await }
                } else {
                    quote! { self.#method(#(#param_idents),*).await }
                };

                (
                    quote! {
                        let params: #struct_ident = match tmcp::Arguments::into_tool_params(
                            arguments,
                            #defaults,
                        ) {
                            Ok(params) => params,
                            Err(err) => {
                                return Ok(err.into());
                            }
                        };
                        let #struct_ident { #(#param_idents),* } = params;
                    },
                    call,
                )
            }
        };

        let call = match &tool.return_kind {
            ToolReturnKind::Result => quote! {
                #args_prelude
                #call_expr
            },
            ToolReturnKind::ToolResult { .. } => quote! {
                #args_prelude
                let result = #call_expr;
                Ok(match result {
                    Ok(value) => value.into(),
                    Err(err) => err.into(),
                })
            },
        };

        quote! {
            #name => {
                #call
            }
        }
    });

    quote! {
        async fn call_tool(
            &self,
            context: &tmcp::ServerCtx,
            name: String,
            arguments: Option<tmcp::Arguments>,
            _task: Option<tmcp::schema::TaskMetadata>,
        ) -> tmcp::Result<tmcp::schema::CallToolResult> {
            match name.as_str() {
                #(#tool_matches)*
                _ => Err(tmcp::Error::ToolNotFound(name))
            }
        }
    }
}

/// Generate the ServerHandler::list_tools implementation.
fn generate_list_tools(info: &ServerInfo) -> TokenStream {
    let tools = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let description = &tool.docs;
        let schema = match tool.params_kind {
            ParamsKind::None | ParamsKind::Unit => {
                quote! { tmcp::schema::ToolSchema::empty() }
            }
            ParamsKind::Typed(ref params_type) => {
                let params_type = params_type.as_ref();
                quote! { tmcp::schema::ToolSchema::from_json_schema::<#params_type>() }
            }
            ParamsKind::Flat(_) => {
                let struct_ident = flat_args_struct_ident(&info.struct_name, name);
                quote! { tmcp::schema::ToolSchema::from_json_schema::<#struct_ident>() }
            }
        };

        let description_setter = if description.is_empty() {
            quote! {}
        } else {
            quote! { tool = tool.with_description(#description); }
        };

        let title_setter = tool
            .attrs
            .title
            .as_ref()
            .map(|title| quote! { tool = tool.with_annotation_title(#title); })
            .unwrap_or_default();

        let read_only_setter = tool
            .attrs
            .read_only
            .map(|value| quote! { tool = tool.with_read_only_hint(#value); })
            .unwrap_or_default();

        let destructive_setter = tool
            .attrs
            .destructive
            .map(|value| quote! { tool = tool.with_destructive_hint(#value); })
            .unwrap_or_default();

        let idempotent_setter = tool
            .attrs
            .idempotent
            .map(|value| quote! { tool = tool.with_idempotent_hint(#value); })
            .unwrap_or_default();

        let open_world_setter = tool
            .attrs
            .open_world
            .map(|value| quote! { tool = tool.with_open_world_hint(#value); })
            .unwrap_or_default();

        let task_support_setter = tool
            .attrs
            .task_support
            .map(|support| {
                let support_expr = match support {
                    ToolTaskSupport::Forbidden => {
                        quote! { tmcp::schema::ToolTaskSupport::Forbidden }
                    }
                    ToolTaskSupport::Optional => {
                        quote! { tmcp::schema::ToolTaskSupport::Optional }
                    }
                    ToolTaskSupport::Required => {
                        quote! { tmcp::schema::ToolTaskSupport::Required }
                    }
                };
                quote! { tool = tool.with_task_support(#support_expr); }
            })
            .unwrap_or_default();

        let output_schema = tool
            .attrs
            .output_schema
            .clone()
            .or_else(|| match &tool.return_kind {
                ToolReturnKind::ToolResult { output } => output.as_ref().clone(),
                _ => None,
            })
            .filter(|ty| !is_call_tool_result_type(ty));

        let output_schema_setter = output_schema
            .map(|ty| {
                quote! { tool = tool.with_output_schema(tmcp::schema::ToolSchema::from_json_schema::<#ty>()); }
            })
            .unwrap_or_default();

        let icons_setter = if tool.attrs.icons.is_empty() {
            quote! {}
        } else {
            let icons = tool
                .attrs
                .icons
                .iter()
                .map(|icon| quote! { tmcp::schema::Icon::new(#icon) });
            quote! {
                tool = tool.with_icons(vec![#(#icons),*]);
            }
        };

        quote! {
            {
                let mut tool = tmcp::schema::Tool::new(#name, #schema);
                #description_setter
                #title_setter
                #read_only_setter
                #destructive_setter
                #idempotent_setter
                #open_world_setter
                #task_support_setter
                #output_schema_setter
                #icons_setter
                tool
            }
        }
    });

    quote! {
        async fn list_tools(
            &self,
            _context: &tmcp::ServerCtx,
            _cursor: Option<tmcp::schema::Cursor>,
        ) -> tmcp::Result<tmcp::schema::ListToolsResult> {
            Ok(tmcp::schema::ListToolsResult {
                tools: vec![#(#tools),*],
                next_cursor: None,
            })
        }
    }
}

/// Generate struct definitions for flat tool argument lists.
fn generate_flat_arg_structs(info: &ServerInfo) -> Vec<TokenStream> {
    info.tools
        .iter()
        .filter_map(|tool| match &tool.params_kind {
            ParamsKind::Flat(params) => Some((tool, params)),
            _ => None,
        })
        .map(|(tool, params)| {
            let struct_ident = flat_args_struct_ident(&info.struct_name, &tool.name);
            let fields = params.iter().map(|param| {
                let ident = &param.ident;
                let ty = &param.ty;
                let attrs = &param.attrs;
                quote! {
                    #(#attrs)*
                    #ident: #ty,
                }
            });

            quote! {
                #[doc(hidden)]
                #[allow(non_camel_case_types)]
                #[derive(serde::Deserialize, schemars::JsonSchema)]
                struct #struct_ident {
                    #(#fields)*
                }
            }
        })
        .collect()
}

/// Generate the ServerHandler::initialize implementation.
fn generate_initialize(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    custom_init_fn: Option<&syn::Ident>,
) -> TokenStream {
    if let Some(init_fn) = custom_init_fn {
        // Use the custom initialize function
        quote! {
            async fn initialize(
                &self,
                context: &tmcp::ServerCtx,
                protocol_version: String,
                capabilities: tmcp::schema::ClientCapabilities,
                client_info: tmcp::schema::Implementation,
            ) -> tmcp::Result<tmcp::schema::InitializeResult> {
                self.#init_fn(context, protocol_version, capabilities, client_info).await
            }
        }
    } else {
        // Use the default implementation
        generate_default_initialize(info, args)
    }
}

/// Determine if tools capability should be advertised based on tool count.
fn has_tools(info: &ServerInfo) -> bool {
    !info.tools.is_empty()
}

/// Generate the default ServerHandler::initialize implementation.
fn generate_default_initialize(info: &ServerInfo, args: &ServerMacroArgs) -> TokenStream {
    let snake_case_name = info.struct_name.to_snake_case();
    let description = &info.description;
    let has_tools = has_tools(info);

    let name_expr = args
        .name
        .as_ref()
        .map(|expr| quote! { #expr })
        .unwrap_or_else(|| quote! { #snake_case_name });

    let version_expr = args
        .version
        .as_ref()
        .map(|expr| quote! { #expr })
        .unwrap_or_else(|| quote! { env!("CARGO_PKG_VERSION") });

    let instructions_setter = if let Some(instructions) = &args.instructions {
        quote! { init = init.with_instructions(#instructions); }
    } else if description.is_empty() {
        quote! {}
    } else {
        quote! { init = init.with_instructions(#description); }
    };

    let (protocol_param, protocol_version_setter) = match args.protocol_version {
        Some(ProtocolVersionStrategy::Client) => (
            quote! { protocol_version: String },
            quote! {
                if !protocol_version.is_empty() {
                    init = init.with_mcp_version(protocol_version);
                }
            },
        ),
        _ => (quote! { _protocol_version: String }, quote! {}),
    };

    quote! {
        async fn initialize(
            &self,
            _context: &tmcp::ServerCtx,
            #protocol_param,
            _capabilities: tmcp::schema::ClientCapabilities,
            _client_info: tmcp::schema::Implementation,
        ) -> tmcp::Result<tmcp::schema::InitializeResult> {
            let mut init = tmcp::schema::InitializeResult::new(#name_expr)
                .with_version(#version_expr)
                .with_tools(#has_tools);
            #instructions_setter
            #protocol_version_setter
            Ok(init)
        }
    }
}

/// Validate the signature of a custom initialize function.
fn validate_custom_initialize_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    // Find the method in the impl block
    let method = impl_block.items.iter().find_map(|item| {
        if let ImplItem::Fn(method) = item {
            if method.sig.ident == *fn_name {
                Some(method)
            } else {
                None
            }
        } else {
            None
        }
    });

    let method = method.ok_or_else(|| {
        syn::Error::new(
            fn_name.span(),
            format!("Custom initialize function '{fn_name}' not found in impl block"),
        )
    })?;

    // Validate it's async
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            "Custom initialize function must be async",
        ));
    }

    // Validate parameters
    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.len() != 5 {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            "Custom initialize function must have exactly 5 parameters: &self, context: &ServerCtx, protocol_version: String, capabilities: ClientCapabilities, client_info: Implementation",
        ));
    }

    // Validate &self
    match params[0] {
        syn::FnArg::Receiver(_) => {}
        _ => {
            return Err(syn::Error::new(
                params[0].span(),
                "First parameter must be &self",
            ));
        }
    }

    // Validate return type exists
    match &method.sig.output {
        syn::ReturnType::Type(_, _) => {
            // We just check it exists, full type validation would be complex
        }
        _ => {
            return Err(syn::Error::new(
                method.sig.output.span(),
                "Custom initialize function must return Result<InitializeResult>",
            ));
        }
    }

    Ok(())
}

/// Parse the #[mcp_server] macro inputs and emit the expanded tokens.
fn inner_mcp_server(attr: TokenStream, input: &TokenStream) -> Result<TokenStream> {
    // Parse macro attributes
    let args = syn::parse2::<ServerMacroArgs>(attr).unwrap_or_default();
    let (impl_block, info) = parse_impl_block(input)?;

    if info.tools.is_empty() {
        return Err(syn::Error::new(
            input.span(),
            "No tool methods found. Use #[tool] to mark methods as MCP tools",
        ));
    }

    // Validate custom initialize function if provided
    if let Some(ref init_fn) = args.initialize_fn {
        validate_custom_initialize_fn(&impl_block, init_fn)?;
    }

    let struct_name = syn::Ident::new(&info.struct_name, proc_macro2::Span::call_site());
    let call_tool = generate_call_tool(&info);
    let list_tools = generate_list_tools(&info);
    let initialize = generate_initialize(&info, &args, args.initialize_fn.as_ref());
    let flat_structs = generate_flat_arg_structs(&info);

    Ok(quote! {
        #(#flat_structs)*
        #impl_block

        #[async_trait::async_trait]
        impl tmcp::ServerHandler for #struct_name {
            #initialize
            #list_tools
            #call_tool
        }
    })
}

/// Derive the ServerHandler methods from an impl block.
#[proc_macro_attribute]
pub fn mcp_server(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr_tokens = TokenStream::from(attr);
    let input_tokens = TokenStream::from(input);

    match inner_mcp_server(attr_tokens, &input_tokens) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Mark a method as an mcp tool.
#[proc_macro_attribute]
pub fn tool(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    input
}

/// Collect derive identifiers from attributes.
fn collect_derive_idents(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut idents = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        let derive_args = attr
            .parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
            .unwrap_or_default();
        for path in derive_args {
            if let Some(ident) = path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            {
                idents.push(ident);
            }
        }
    }
    idents
}

/// Add derive attributes from the provided paths if they are missing.
fn add_missing_derives(item: &mut syn::DeriveInput, derive_paths: &[syn::Path]) {
    let existing = collect_derive_idents(&item.attrs);
    let mut missing = Vec::new();

    for path in derive_paths {
        if let Some(ident) = path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            && !existing.iter().any(|e| e == &ident)
        {
            missing.push(path.clone());
        }
    }

    if !missing.is_empty() {
        let derive_attr: syn::Attribute = syn::parse_quote! {
            #[derive(#(#missing),*)]
        };
        item.attrs.insert(0, derive_attr);
    }
}

/// Add serde + schemars derives for tool parameter structs.
#[proc_macro_attribute]
pub fn tool_params(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut item = syn::parse_macro_input!(input as syn::DeriveInput);
    add_missing_derives(
        &mut item,
        &[
            syn::parse_quote!(serde::Deserialize),
            syn::parse_quote!(schemars::JsonSchema),
        ],
    );
    quote!(#item).into()
}

/// Add serde + schemars derives plus ToolResponse for tool result structs.
#[proc_macro_attribute]
pub fn tool_result(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut item = syn::parse_macro_input!(input as syn::DeriveInput);
    add_missing_derives(
        &mut item,
        &[
            syn::parse_quote!(serde::Serialize),
            syn::parse_quote!(schemars::JsonSchema),
            syn::parse_quote!(tmcp::ToolResponse),
        ],
    );
    quote!(#item).into()
}

/// Derive `ToolResponse` by encoding the type as structured content.
#[proc_macro_derive(ToolResponse)]
pub fn derive_tool_response(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    let ident = &input.ident;
    let mut generics = input.generics.clone();
    {
        let where_clause = generics.make_where_clause();
        where_clause
            .predicates
            .push(syn::parse_quote! { Self: ::serde::Serialize });
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics ::tmcp::ToolResponse for #ident #ty_generics #where_clause {
            fn into_call_tool_result(self) -> ::tmcp::schema::CallToolResult {
                match ::tmcp::schema::CallToolResult::structured(self) {
                    Ok(result) => result,
                    Err(err) => ::tmcp::schema::CallToolResult::error("INTERNAL", err.to_string()),
                }
            }
        }
    };

    expanded.into()
}

/// Adds a _meta field to a struct with proper serde attributes and builder methods.
///
/// This macro adds the following field to the struct:
/// ```ignore
/// #[serde(skip_serializing_if = "Option::is_none")]
/// pub _meta: Option<HashMap<String, Value>>,
/// ```
///
/// And generates these builder methods:
/// - `with_meta(mut self, meta: HashMap<String, Value>) -> Self`
/// - `with_meta_entry(mut self, key: impl Into<String>, value: Value) -> Self`
#[proc_macro_attribute]
pub fn with_meta(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut input = syn::parse_macro_input!(input as syn::DeriveInput);

    // Only process structs
    let syn::Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new(
            input.ident.span(),
            "with_meta can only be applied to structs",
        )
        .to_compile_error()
        .into();
    };

    let syn::Fields::Named(fields) = &mut data_struct.fields else {
        return syn::Error::new(
            input.ident.span(),
            "with_meta can only be applied to structs with named fields",
        )
        .to_compile_error()
        .into();
    };

    // Create the _meta field
    let meta_field: syn::Field = syn::parse_quote! {
        /// Optional metadata field for extensions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub _meta: Option<std::collections::HashMap<String, serde_json::Value>>
    };

    // Add the field
    fields.named.push(meta_field);

    // Generate the struct name and generics
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the output with builder methods
    let output = quote! {
        #input

        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Set the metadata map
            pub fn with_meta(mut self, meta: std::collections::HashMap<String, serde_json::Value>) -> Self {
                self._meta = Some(meta);
                self
            }

            /// Add a single metadata entry
            pub fn with_meta_entry(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
                self._meta
                    .get_or_insert_with(std::collections::HashMap::new)
                    .insert(key.into(), value);
                self
            }
        }
    };

    output.into()
}

/// Adds name and title fields to a struct with proper serde attributes, documentation, and builder methods.
///
/// This macro adds the following fields to the struct:
/// ```ignore
/// /// Intended for programmatic or logical use, but used as a display name in past specs or fallback (if title isn't present).
/// pub name: String,
///
/// /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
/// /// even by those unfamiliar with domain-specific terminology.
/// ///
/// /// If not provided, the name should be used for display.
/// #[serde(skip_serializing_if = "Option::is_none")]
/// pub title: Option<String>,
/// ```
///
/// And generates these builder methods:
/// - `with_name(mut self, name: impl Into<String>) -> Self`
/// - `with_title(mut self, title: impl Into<String>) -> Self`
#[proc_macro_attribute]
pub fn with_basename(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut input = syn::parse_macro_input!(input as syn::DeriveInput);

    // Only process structs
    let syn::Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new(
            input.ident.span(),
            "with_basename can only be applied to structs",
        )
        .to_compile_error()
        .into();
    };

    let syn::Fields::Named(fields) = &mut data_struct.fields else {
        return syn::Error::new(
            input.ident.span(),
            "with_basename can only be applied to structs with named fields",
        )
        .to_compile_error()
        .into();
    };

    // Create the name field
    let name_field: syn::Field = syn::parse_quote! {
        /// Intended for programmatic or logical use, but used as a display name in past specs or fallback (if title isn't present).
        pub name: String
    };

    // Create the title field
    let title_field: syn::Field = syn::parse_quote! {
        /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
        /// even by those unfamiliar with domain-specific terminology.
        ///
        /// If not provided, the name should be used for display.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>
    };

    // Add the fields
    fields.named.push(name_field);
    fields.named.push(title_field);

    // Generate the struct name and generics
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the output with builder methods
    let output = quote! {
        #input

        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Set the name field
            pub fn with_name(mut self, name: impl Into<String>) -> Self {
                self.name = name.into();
                self
            }

            /// Set the title field
            pub fn with_title(mut self, title: impl Into<String>) -> Self {
                self.title = Some(title.into());
                self
            }
        }
    };

    output.into()
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
        assert_eq!(result, "First line\nSecond line\nThird line");
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
            async fn test_tool(&mut self, context: &ServerCtx, other: &ServerCtx) -> Result<schema::CallToolResult> {
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
    fn test_parse_tool_method_both_self_types() {
        // Test that both &self and &mut self are accepted
        let method1: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };
        assert!(parse_tool_method(&method1).unwrap().is_some());

        let method2: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&mut self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };
        assert!(parse_tool_method(&method2).unwrap().is_some());
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

    #[test]
    fn test_generate_call_tool() {
        let info = ServerInfo {
            struct_name: "TestServer".to_string(),
            description: "Test description".to_string(),
            tools: vec![
                ToolMethod {
                    name: "echo".to_string(),
                    docs: "Echo tool".to_string(),
                    has_ctx: true,
                    params_kind: ParamsKind::Typed(Box::new(syn::parse_quote! { EchoParams })),
                    return_kind: ToolReturnKind::Result,
                    attrs: ToolAttrs::default(),
                },
                ToolMethod {
                    name: "ping".to_string(),
                    docs: "Ping tool".to_string(),
                    has_ctx: true,
                    params_kind: ParamsKind::Typed(Box::new(syn::parse_quote! { PingParams })),
                    return_kind: ToolReturnKind::Result,
                    attrs: ToolAttrs::default(),
                },
            ],
        };

        let generated = generate_call_tool(&info);
        let generated_str = generated.to_string();

        assert!(generated_str.contains("call_tool"));
        assert!(generated_str.contains("match name . as_str ()"));
        assert!(generated_str.contains("\"echo\" =>"));
        assert!(generated_str.contains("\"ping\" =>"));
        assert!(generated_str.contains("self . echo (context , params) . await"));
        assert!(generated_str.contains("self . ping (context , params) . await"));
    }

    #[test]
    fn test_generate_list_tools() {
        let info = ServerInfo {
            struct_name: "TestServer".to_string(),
            description: "Test description".to_string(),
            tools: vec![ToolMethod {
                name: "echo".to_string(),
                docs: "Echo tool".to_string(),
                has_ctx: true,
                params_kind: ParamsKind::Typed(Box::new(syn::parse_quote! { EchoParams })),
                return_kind: ToolReturnKind::Result,
                attrs: ToolAttrs::default(),
            }],
        };

        let generated = generate_list_tools(&info);
        let generated_str = generated.to_string();

        assert!(generated_str.contains("list_tools"));
        assert!(generated_str.contains("Tool :: new"));
        assert!(generated_str.contains("with_description"));
        assert!(generated_str.contains("\"echo\""));
        assert!(generated_str.contains("\"Echo tool\""));
    }

    #[test]
    fn test_full_macro_expansion() {
        let input = quote! {
            /// Test server
            impl TestServer {
                #[tool]
                /// Echo back the input
                async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
        let result_str = result.to_string();

        // Check that original impl block is preserved
        assert!(result_str.contains("impl TestServer"));
        assert!(result_str.contains("async fn echo"));

        // Check that ServerHandler impl is generated
        assert!(result_str.contains("impl tmcp :: ServerHandler for TestServer"));
        assert!(result_str.contains("async fn initialize"));
        assert!(result_str.contains("async fn list_tools"));
        assert!(result_str.contains("async fn call_tool"));

        // Check that snake_case conversion is applied
        assert!(result_str.contains(r#"InitializeResult :: new ("test_server")"#));
    }

    #[test]
    fn test_full_macro_expansion_flat_args() {
        let input = quote! {
            impl TestServer {
                #[tool]
                async fn add(&self, a: i32, b: i32) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
        let result_str = result.to_string();

        assert!(result_str.contains("__TmcpToolArgs_TestServer_add"));
        assert!(result_str.contains("async fn call_tool"));
        assert!(result_str.contains("async fn list_tools"));
    }

    #[test]
    fn test_no_tools_error() {
        let input = quote! {
            impl TestServer {
                async fn helper(&self) -> String {
                    "helper".to_string()
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No tool methods found")
        );
    }

    #[test]
    fn test_snake_case_conversion() {
        // Test various struct name patterns
        let test_cases = vec![
            ("TestServer", "test_server"),
            ("MyMCPServer", "my_mcp_server"),
            ("HTTPServer", "http_server"),
            ("SimpleServer", "simple_server"),
            ("MyHTTPAPIServer", "my_httpapi_server"),
        ];

        for (struct_name, expected_snake_case) in test_cases {
            let struct_ident = syn::Ident::new(struct_name, proc_macro2::Span::call_site());
            let input = quote! {
                impl #struct_ident {
                    #[tool]
                    async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                        Ok(schema::CallToolResult::new())
                    }
                }
            };

            let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
            let result_str = result.to_string();

            let expected_pattern = format!(r#"InitializeResult :: new ("{expected_snake_case}")"#);
            assert!(
                result_str.contains(&expected_pattern),
                "Expected server name '{expected_snake_case}' for struct '{struct_name}', but got: {result_str}"
            );
        }
    }

    #[test]
    fn test_empty_description_no_instructions() {
        let input = quote! {
            impl TestServer {
                #[tool]
                async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
        let result_str = result.to_string();

        // Check that with_instructions is NOT called when description is empty
        assert!(!result_str.contains("with_instructions"));
        assert!(result_str.contains("InitializeResult :: new"));
        assert!(result_str.contains("with_version"));
        assert!(result_str.contains("with_tools"));
    }
}
