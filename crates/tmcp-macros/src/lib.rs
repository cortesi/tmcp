//! Macros to make defining MCP servers easier
//!
//! Using the #[mcp_server] macro on an impl block, this crate will pick up all methods
//! marked with #[tool] and derive the necessary ServerHandler::call_tool, ServerHandler::list_tools and
//! ServerHandler::initialize methods. Resource callbacks can be wired into the same generated
//! ServerHandler implementation for servers whose resources are discovered dynamically. The name of
//! the server is derived from the name of the struct converted to snake_case (e.g., MyServer becomes
//! my_server), and the description is derived from the doc comment on the impl block. The version is
//! set to "0.1.0" by default.
//!
//! The macro supports customization through attributes:
//! - `initialize_fn`: Specify a custom initialize function instead of using the default
//! - `name`: Override the server name used in initialization
//! - `version`: Override the server version used in initialization
//! - `instructions`: Override the server instructions used in initialization
//! - `protocol_version`: Control protocol version negotiation ("latest" or "client")
//! - `toolset`: Use a ToolSet field for progressive discovery
//! - `resources_fn`: Forward `resources/list` to an async method
//! - `read_resource_fn`: Forward `resources/read` to an async method
//! - `resource_templates_fn`: Forward `resources/templates/list` to an async method
//! - `shutdown_fn`: Forward shutdown handling to an async method
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
//! Resource callback methods must use these signatures:
//!
//! ```ignore
//! async fn resources(
//!     &self,
//!     context: &ServerCtx,
//!     cursor: Option<schema::Cursor>,
//! ) -> Result<schema::ListResourcesResult>
//!
//! async fn read_resource(
//!     &self,
//!     context: &ServerCtx,
//!     uri: String,
//! ) -> Result<schema::ReadResourceResult>
//!
//! async fn resource_templates(
//!     &self,
//!     context: &ServerCtx,
//!     cursor: Option<schema::Cursor>,
//! ) -> Result<schema::ListResourceTemplatesResult>
//!
//! async fn shutdown(&self) -> Result<()>
//! ```
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
//! - `always` (always visible when using ToolSet-backed servers)
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

#![warn(missing_docs)]

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

#[derive(Debug)]
/// Description of an impl method tagged as a group factory.
struct GroupMethod {
    /// Method identifier for constructing the group.
    ident: syn::Ident,
    /// Optional segment override for this group edge.
    segment_override: Option<String>,
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
    /// Whether the tool should always be visible when using ToolSet.
    always: bool,
}

#[derive(Debug, Default)]
/// Metadata parsed from a #[group(...)] attribute.
struct GroupMeta {
    /// Optional group name override.
    name: Option<String>,
    /// Optional description override.
    description: Option<String>,
    /// Optional deactivator visibility override.
    show_deactivator: Option<bool>,
    /// Optional activation hook method name.
    on_activate: Option<syn::Ident>,
    /// Optional deactivation hook method name.
    on_deactivate: Option<syn::Ident>,
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
    /// Group factory methods discovered in the impl block.
    groups: Vec<GroupMethod>,
}

#[derive(Debug, Default)]
/// Parsed macro arguments for #[mcp_server].
struct ServerMacroArgs {
    /// Optional custom initialize function name.
    initialize_fn: Option<syn::Ident>,
    /// Optional function used to list dynamic resources.
    resources_fn: Option<syn::Ident>,
    /// Optional function used to read dynamic resources.
    read_resource_fn: Option<syn::Ident>,
    /// Optional function used to list dynamic resource templates.
    resource_templates_fn: Option<syn::Ident>,
    /// Optional function called when the server shuts down.
    shutdown_fn: Option<syn::Ident>,
    /// Optional server name override.
    name: Option<Expr>,
    /// Optional server version override.
    version: Option<Expr>,
    /// Optional instructions override.
    instructions: Option<Expr>,
    /// Protocol version negotiation strategy.
    protocol_version: Option<ProtocolVersionStrategy>,
    /// Optional ToolSet field name for progressive discovery.
    toolset: Option<syn::Ident>,
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
            } else if ident == "resources_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.resources_fn = Some(fn_name);
            } else if ident == "read_resource_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.read_resource_fn = Some(fn_name);
            } else if ident == "resource_templates_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.resource_templates_fn = Some(fn_name);
            } else if ident == "shutdown_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.shutdown_fn = Some(fn_name);
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
            } else if ident == "toolset" {
                let expr: Expr = input.parse()?;
                args.toolset = Some(parse_ident_from_expr(&expr)?);
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

impl ServerMacroArgs {
    /// Return true if any resource protocol method should be generated.
    fn has_resource_callbacks(&self) -> bool {
        self.resources_fn.is_some()
            || self.read_resource_fn.is_some()
            || self.resource_templates_fn.is_some()
    }

    /// Return true if resource listing capability should report list changes.
    fn resources_list_changed(&self) -> bool {
        self.resources_fn.is_some() || self.resource_templates_fn.is_some()
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

/// Parse an identifier from a string literal or path expression.
fn parse_ident_from_expr(expr: &Expr) -> Result<syn::Ident> {
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

/// Parse group metadata from a #[tmcp_group_meta(...)] attribute.
fn parse_group_meta(attrs: &[syn::Attribute]) -> Result<GroupMeta> {
    let mut meta = GroupMeta::default();

    for attr in attrs {
        if !(attr.path().is_ident("tmcp_group_meta") || attr.path().is_ident("group")) {
            continue;
        }

        let metas = match &attr.meta {
            Meta::Path(_) => Punctuated::<Meta, syn::Token![,]>::new(),
            Meta::List(list) => {
                list.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)?
            }
            Meta::NameValue(meta) => {
                return Err(syn::Error::new(
                    meta.span(),
                    "#[group] does not support name-value syntax",
                ));
            }
        };

        for meta_item in metas {
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

    Ok(meta)
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

/// Parse a group factory method from an impl item if it has a #[group] attribute.
fn parse_group_method(method: &syn::ImplItemFn) -> Result<Option<GroupMethod>> {
    let mut segment_override = None;
    let mut found = false;

    for attr in &method.attrs {
        if !attr.path().is_ident("group") {
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
                    "#[group] does not support name-value syntax",
                ));
            }
        };

        for meta in metas {
            match meta {
                Meta::NameValue(item) => {
                    let ident = item.path.get_ident().ok_or_else(|| {
                        syn::Error::new(item.path.span(), "invalid #[group] argument")
                    })?;
                    match ident.to_string().as_str() {
                        "name" => {
                            segment_override = Some(parse_string_lit(&item.value)?);
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

    if !found {
        return Ok(None);
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
            "group methods must include &self or &mut self",
        ));
    }

    match params[0] {
        syn::FnArg::Receiver(_) => {}
        _ => {
            return Err(syn::Error::new(
                params[0].span(),
                "first parameter must be &self or &mut self",
            ));
        }
    }

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
        segment_override,
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

    Ok((
        impl_block,
        ServerInfo {
            struct_name,
            description,
            tools,
            groups,
        },
    ))
}

/// Generate the ServerHandler::call_tool implementation.
fn generate_call_tool(info: &ServerInfo) -> TokenStream {
    let receiver = quote! { self };
    let tool_matches = info
        .tools
        .iter()
        .map(|tool| generate_tool_call_arm(tool, &receiver, &info.struct_name));

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

/// Generate a tool call match arm for a receiver expression.
fn generate_tool_call_arm(
    tool: &ToolMethod,
    receiver: &TokenStream,
    owner_name: &str,
) -> TokenStream {
    let name = &tool.name;
    let method = syn::Ident::new(name, proc_macro2::Span::call_site());
    let defaults = tool.attrs.defaults;
    let (args_prelude, call_expr) = match (&tool.has_ctx, &tool.params_kind) {
        (false, ParamsKind::None) => (
            quote! { let _ = arguments; },
            quote! { #receiver.#method().await },
        ),
        (true, ParamsKind::None) => (
            quote! { let _ = arguments; },
            quote! { #receiver.#method(context).await },
        ),
        (false, ParamsKind::Unit) => (
            quote! { let _ = arguments; },
            quote! { #receiver.#method(()).await },
        ),
        (true, ParamsKind::Unit) => (
            quote! { let _ = arguments; },
            quote! { #receiver.#method(context, ()).await },
        ),
        (has_ctx, ParamsKind::Typed(params_type)) => {
            let params_type = params_type.as_ref();
            let call = if *has_ctx {
                quote! { #receiver.#method(context, params).await }
            } else {
                quote! { #receiver.#method(params).await }
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
            let struct_ident = flat_args_struct_ident(owner_name, name);
            let param_idents: Vec<_> = params.iter().map(|param| &param.ident).collect();

            let call = if *has_ctx {
                quote! { #receiver.#method(context, #(#param_idents),*).await }
            } else {
                quote! { #receiver.#method(#(#param_idents),*).await }
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
}

/// Generate the ServerHandler::list_tools implementation.
fn tool_schema_expr(tool: &ToolMethod, owner_name: &str) -> TokenStream {
    match tool.params_kind {
        ParamsKind::None | ParamsKind::Unit => {
            quote! { tmcp::schema::ToolSchema::empty() }
        }
        ParamsKind::Typed(ref params_type) => {
            let params_type = params_type.as_ref();
            quote! { tmcp::schema::ToolSchema::from_json_schema::<#params_type>() }
        }
        ParamsKind::Flat(_) => {
            let struct_ident = flat_args_struct_ident(owner_name, &tool.name);
            quote! { tmcp::schema::ToolSchema::from_json_schema::<#struct_ident>() }
        }
    }
}

/// Build a Tool expression with metadata annotations applied.
fn build_tool_expr(
    tool: &ToolMethod,
    name_expr: &TokenStream,
    schema_expr: &TokenStream,
) -> TokenStream {
    let description = &tool.docs;
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
            let mut tool = tmcp::schema::Tool::new(#name_expr, #schema_expr);
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
}

/// Generate the ServerHandler::list_tools implementation.
fn generate_list_tools(info: &ServerInfo) -> TokenStream {
    let tools = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let name_expr = quote! { #name };
        let schema = tool_schema_expr(tool, &info.struct_name);
        build_tool_expr(tool, &name_expr, &schema)
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
fn generate_flat_arg_structs_for(struct_name: &str, tools: &[ToolMethod]) -> Vec<TokenStream> {
    tools
        .iter()
        .filter_map(|tool| match &tool.params_kind {
            ParamsKind::Flat(params) => Some((tool, params)),
            _ => None,
        })
        .map(|(tool, params)| {
            let struct_ident = flat_args_struct_ident(struct_name, &tool.name);
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

/// Generate struct definitions for flat tool argument lists.
fn generate_flat_arg_structs(info: &ServerInfo) -> Vec<TokenStream> {
    generate_flat_arg_structs_for(&info.struct_name, &info.tools)
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
        let tools_list_changed = has_tools(info).then_some(true);
        generate_default_initialize(info, args, tools_list_changed)
    }
}

/// Determine if tools capability should be advertised based on tool count.
fn has_tools(info: &ServerInfo) -> bool {
    !info.tools.is_empty()
}

/// Generate the default ServerHandler::initialize implementation.
fn generate_default_initialize(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    tools_list_changed: Option<bool>,
) -> TokenStream {
    let prologue = quote! {};
    generate_default_initialize_with_prologue(info, args, tools_list_changed, &prologue)
}

/// Generate the default initialize implementation with a custom prologue.
fn generate_default_initialize_with_prologue(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    tools_list_changed: Option<bool>,
    prologue: &TokenStream,
) -> TokenStream {
    let snake_case_name = info.struct_name.to_snake_case();
    let description = &info.description;

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

    let tools_capability_setter = tools_list_changed
        .map(|list_changed| quote! { init = init.with_tools(#list_changed); })
        .unwrap_or_default();

    let resources_capability_setter = if args.has_resource_callbacks() {
        let list_changed = args.resources_list_changed();
        quote! { init = init.with_resources(false, #list_changed); }
    } else {
        quote! {}
    };

    quote! {
        async fn initialize(
            &self,
            _context: &tmcp::ServerCtx,
            #protocol_param,
            _capabilities: tmcp::schema::ClientCapabilities,
            _client_info: tmcp::schema::Implementation,
        ) -> tmcp::Result<tmcp::schema::InitializeResult> {
            #prologue
            let mut init = tmcp::schema::InitializeResult::new(#name_expr)
                .with_version(#version_expr);
            #tools_capability_setter
            #resources_capability_setter
            #instructions_setter
            #protocol_version_setter
            Ok(init)
        }
    }
}

/// Generate tool registration statements for ToolSet-backed servers.
fn generate_toolset_registration(info: &ServerInfo, toolset_field: &syn::Ident) -> TokenStream {
    let group_registrations = info.groups.iter().map(|group| {
        let method = &group.ident;
        let override_expr = group
            .segment_override
            .as_ref()
            .map(|name| quote! { Some(#name) })
            .unwrap_or_else(|| quote! { None });
        quote! {
            {
                let group = self.#method();
                tmcp::GroupRegistration::register_with_override(
                    &group,
                    &self.#toolset_field,
                    None,
                    #override_expr,
                )
                .expect("group registration failed");
            }
        }
    });

    let tool_registrations = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let name_expr = quote! { #name };
        let schema = tool_schema_expr(tool, &info.struct_name);
        let tool_expr = build_tool_expr(tool, &name_expr, &schema);
        quote! {
            {
                let tool = #tool_expr;
                self.#toolset_field
                    .register_schema(#name, tool, tmcp::Visibility::Always)
                    .expect("tool registration failed");
            }
        }
    });

    quote! {
        #(#group_registrations)*
        #(#tool_registrations)*
    }
}

/// Generate a registration guard for ToolSet-backed servers.
fn generate_toolset_ensure_registered(
    info: &ServerInfo,
    toolset_field: &syn::Ident,
) -> TokenStream {
    let registrations = generate_toolset_registration(info, toolset_field);
    quote! {
        #[doc(hidden)]
        fn __ensure_tools_registered(&self) {
            self.#toolset_field.ensure_registered(|| {
                #registrations
            });
        }
    }
}

/// Generate the ServerHandler::list_tools implementation for ToolSet-backed servers.
fn generate_toolset_list_tools(toolset_field: &syn::Ident) -> TokenStream {
    quote! {
        async fn list_tools(
            &self,
            _context: &tmcp::ServerCtx,
            cursor: Option<tmcp::schema::Cursor>,
        ) -> tmcp::Result<tmcp::schema::ListToolsResult> {
            self.__ensure_tools_registered();
            self.#toolset_field.list_tools(cursor)
        }
    }
}

/// Generate group dispatch checks for ToolSet-backed servers.
fn generate_group_dispatch_chain(groups: &[GroupMethod], receiver: &TokenStream) -> TokenStream {
    let checks = groups.iter().map(|group| {
        let method = &group.ident;
        let segment_expr = group
            .segment_override
            .as_ref()
            .map(|name| quote! { #name.to_string() })
            .unwrap_or_else(|| quote! { tmcp::Group::config(&group).name });
        quote! {
            {
                let group = #receiver.#method();
                let segment = #segment_expr;
                if let Some(rest) = name.strip_prefix(segment.as_str()) {
                    if let Some(rest) = rest.strip_prefix('.') {
                        return tmcp::GroupDispatch::call_tool(&group, context, rest, arguments)
                            .await;
                    }
                }
            }
        }
    });

    quote! {
        #(#checks)*
    }
}

/// Generate the ServerHandler::call_tool implementation for ToolSet-backed servers.
fn generate_toolset_call_tool(info: &ServerInfo, toolset_field: &syn::Ident) -> TokenStream {
    let receiver = quote! { handler };
    let tool_matches = info
        .tools
        .iter()
        .map(|tool| generate_tool_call_arm(tool, &receiver, &info.struct_name));
    let group_dispatch = generate_group_dispatch_chain(&info.groups, &receiver);

    quote! {
        async fn call_tool(
            &self,
            context: &tmcp::ServerCtx,
            name: String,
            arguments: Option<tmcp::Arguments>,
            _task: Option<tmcp::schema::TaskMetadata>,
        ) -> tmcp::Result<tmcp::schema::CallToolResult> {
            self.__ensure_tools_registered();
            self.#toolset_field
                .call_tool_with(self, context, &name, arguments, |handler, context, name, arguments| -> tmcp::ToolFuture<'_> {
                    Box::pin(async move {
                        match name {
                            #(#tool_matches)*
                            _ => {
                                #group_dispatch
                                handler
                                    .#toolset_field
                                    .call_dynamic_tool(context, name, arguments)
                                    .await
                            }
                        }
                    })
                })
                .await
        }
    }
}

/// Generate initialize for ToolSet-backed servers.
fn generate_toolset_initialize(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    custom_init_fn: Option<&syn::Ident>,
) -> TokenStream {
    if let Some(init_fn) = custom_init_fn {
        quote! {
            async fn initialize(
                &self,
                context: &tmcp::ServerCtx,
                protocol_version: String,
                capabilities: tmcp::schema::ClientCapabilities,
                client_info: tmcp::schema::Implementation,
            ) -> tmcp::Result<tmcp::schema::InitializeResult> {
                self.__ensure_tools_registered();
                self.#init_fn(context, protocol_version, capabilities, client_info).await
            }
        }
    } else {
        let prologue = quote! {
            self.__ensure_tools_registered();
        };
        generate_default_initialize_with_prologue(info, args, Some(true), &prologue)
    }
}

/// Generate the GroupDispatch implementation for a group impl block.
fn generate_group_dispatch_impl(info: &ServerInfo) -> TokenStream {
    let struct_ident = syn::Ident::new(&info.struct_name, proc_macro2::Span::call_site());

    let tool_registrations = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let name_expr = quote! { name.clone() };
        let schema = tool_schema_expr(tool, &info.struct_name);
        let tool_expr = build_tool_expr(tool, &name_expr, &schema);
        let always_visible = tool.attrs.always || name == "activate" || name == "deactivate";
        let visibility = if always_visible {
            quote! { tmcp::Visibility::Always }
        } else {
            quote! { tmcp::Visibility::Group(group_name.to_string()) }
        };
        quote! {
            {
                let name = tmcp::ToolSet::qualified_name(group_name, #name);
                let tool = #tool_expr;
                toolset.register_schema(&name, tool, #visibility)?;
            }
        }
    });

    let group_registrations = info.groups.iter().map(|group| {
        let method = &group.ident;
        let override_expr = group
            .segment_override
            .as_ref()
            .map(|name| quote! { Some(#name) })
            .unwrap_or_else(|| quote! { None });
        quote! {
            {
                let group = self.#method();
                tmcp::GroupRegistration::register_with_override(
                    &group,
                    toolset,
                    Some(group_name),
                    #override_expr,
                )?;
            }
        }
    });

    let receiver = quote! { self };
    let tool_matches = info
        .tools
        .iter()
        .map(|tool| generate_tool_call_arm(tool, &receiver, &info.struct_name));
    let group_dispatch = generate_group_dispatch_chain(&info.groups, &receiver);

    quote! {
        impl tmcp::GroupDispatch for #struct_ident {
            fn register_tools(&self, toolset: &tmcp::ToolSet, group_name: &str) -> tmcp::Result<()> {
                #(#tool_registrations)*
                #(#group_registrations)*
                Ok(())
            }

            fn call_tool<'a>(
                &'a self,
                context: &'a tmcp::ServerCtx,
                name: &'a str,
                arguments: Option<tmcp::Arguments>,
            ) -> tmcp::ToolFuture<'a> {
                Box::pin(async move {
                    match name {
                        #(#tool_matches)*
                        _ => {
                            #group_dispatch
                            Err(tmcp::Error::ToolNotFound(name.to_string()))
                        }
                    }
                })
            }
        }
    }
}

/// Generate the ServerHandler::list_resources implementation if requested.
fn generate_list_resources(args: &ServerMacroArgs) -> TokenStream {
    if let Some(resources_fn) = &args.resources_fn {
        quote! {
            async fn list_resources(
                &self,
                context: &tmcp::ServerCtx,
                cursor: Option<tmcp::schema::Cursor>,
            ) -> tmcp::Result<tmcp::schema::ListResourcesResult> {
                self.#resources_fn(context, cursor).await
            }
        }
    } else {
        quote! {}
    }
}

/// Generate the ServerHandler::read_resource implementation if requested.
fn generate_read_resource(args: &ServerMacroArgs) -> TokenStream {
    if let Some(read_resource_fn) = &args.read_resource_fn {
        quote! {
            async fn read_resource(
                &self,
                context: &tmcp::ServerCtx,
                uri: String,
            ) -> tmcp::Result<tmcp::schema::ReadResourceResult> {
                self.#read_resource_fn(context, uri).await
            }
        }
    } else {
        quote! {}
    }
}

/// Generate the ServerHandler::list_resource_templates implementation if requested.
fn generate_list_resource_templates(args: &ServerMacroArgs) -> TokenStream {
    if let Some(resource_templates_fn) = &args.resource_templates_fn {
        quote! {
            async fn list_resource_templates(
                &self,
                context: &tmcp::ServerCtx,
                cursor: Option<tmcp::schema::Cursor>,
            ) -> tmcp::Result<tmcp::schema::ListResourceTemplatesResult> {
                self.#resource_templates_fn(context, cursor).await
            }
        }
    } else {
        quote! {}
    }
}

/// Generate the ServerHandler::on_shutdown implementation if requested.
fn generate_on_shutdown(args: &ServerMacroArgs) -> TokenStream {
    if let Some(shutdown_fn) = &args.shutdown_fn {
        quote! {
            async fn on_shutdown(&self) -> tmcp::Result<()> {
                self.#shutdown_fn().await
            }
        }
    } else {
        quote! {}
    }
}

/// Generate all optional ServerHandler forwarding methods.
fn generate_server_forwarders(args: &ServerMacroArgs) -> TokenStream {
    let list_resources = generate_list_resources(args);
    let read_resource = generate_read_resource(args);
    let list_resource_templates = generate_list_resource_templates(args);
    let on_shutdown = generate_on_shutdown(args);

    quote! {
        #on_shutdown
        #list_resources
        #read_resource
        #list_resource_templates
    }
}

/// Expand a #[group] impl block into dispatch and registration logic.
fn expand_group_impl(input: &TokenStream) -> Result<TokenStream> {
    let impl_block = syn::parse2::<ItemImpl>(input.clone())?;
    if impl_block.trait_.is_some() {
        return Err(syn::Error::new(
            impl_block.impl_token.span(),
            "#[group] can only be used on inherent impl blocks",
        ));
    }

    let (impl_block, info) = parse_impl_block(input)?;
    let flat_structs = generate_flat_arg_structs_for(&info.struct_name, &info.tools);
    let dispatch_impl = generate_group_dispatch_impl(&info);

    Ok(quote! {
        #(#flat_structs)*
        #impl_block
        #dispatch_impl
    })
}

/// Find a named method in an impl block for a macro forwarding hook.
fn find_impl_method<'a>(
    impl_block: &'a ItemImpl,
    fn_name: &syn::Ident,
    role: &str,
) -> Result<&'a syn::ImplItemFn> {
    impl_block
        .items
        .iter()
        .find_map(|item| {
            if let ImplItem::Fn(method) = item
                && method.sig.ident == *fn_name
            {
                Some(method)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            syn::Error::new(
                fn_name.span(),
                format!("{role} function '{fn_name}' not found in impl block"),
            )
        })
}

/// Validate that the first callback parameter is a shared self receiver.
fn validate_shared_self_receiver(arg: &syn::FnArg) -> Result<()> {
    match arg {
        syn::FnArg::Receiver(receiver)
            if receiver.reference.is_some() && receiver.mutability.is_none() =>
        {
            Ok(())
        }
        _ => Err(syn::Error::new(arg.span(), "first parameter must be &self")),
    }
}

/// Validate common callback shape and return the parsed parameter list.
fn validate_callback_signature<'a>(
    method: &'a syn::ImplItemFn,
    role: &str,
    expected_param_count: usize,
) -> Result<Vec<&'a syn::FnArg>> {
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            format!("{role} function must be async"),
        ));
    }

    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.len() != expected_param_count {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            format!("{role} function must have exactly {expected_param_count} parameters"),
        ));
    }

    validate_shared_self_receiver(params[0])?;
    Ok(params)
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
fn type_path_ends_with(ty: &syn::Type, expected: &str) -> bool {
    type_path_last_ident(ty)
        .map(|ident| ident == expected)
        .unwrap_or(false)
}

/// Return true when a type is `String`.
fn is_string_type(ty: &syn::Type) -> bool {
    type_path_ends_with(ty, "String")
}

/// Return true when a type is `Option<Cursor>`.
fn is_option_cursor_type(ty: &syn::Type) -> bool {
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

    arguments.args.iter().any(|argument| {
        matches!(
            argument,
            syn::GenericArgument::Type(ty) if type_path_ends_with(ty, "Cursor")
        )
    })
}

/// Validate a callback parameter's type.
fn validate_callback_arg_type(
    arg: &syn::FnArg,
    role: &str,
    expected: &str,
    is_expected: fn(&syn::Type) -> bool,
) -> Result<()> {
    let syn::FnArg::Typed(pat_type) = arg else {
        return Err(syn::Error::new(
            arg.span(),
            format!("{role} parameter must be {expected}"),
        ));
    };
    if is_expected(pat_type.ty.as_ref()) {
        Ok(())
    } else {
        Err(syn::Error::new(
            pat_type.ty.span(),
            format!("{role} parameter must be {expected}"),
        ))
    }
}

/// Return the successful payload type from a `Result<T>` return type.
fn result_inner_type<'a>(output: &'a syn::ReturnType, role: &str) -> Result<&'a syn::Type> {
    let syn::ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new(
            output.span(),
            format!("{role} function must return Result<T>"),
        ));
    };
    let syn::Type::Path(type_path) = ty.as_ref() else {
        return Err(syn::Error::new(
            ty.span(),
            format!("{role} function must return Result<T>"),
        ));
    };
    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new(
            ty.span(),
            format!("{role} function must return Result<T>"),
        ));
    };
    if segment.ident != "Result" {
        return Err(syn::Error::new(
            segment.ident.span(),
            format!("{role} function must return Result<T>"),
        ));
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new(
            segment.arguments.span(),
            format!("{role} function must return Result<T>"),
        ));
    };

    arguments
        .args
        .iter()
        .find_map(|argument| {
            if let syn::GenericArgument::Type(ty) = argument {
                Some(ty)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            syn::Error::new(
                segment.arguments.span(),
                format!("{role} function must return Result<T>"),
            )
        })
}

/// Validate a callback result payload type.
fn validate_result_payload(
    method: &syn::ImplItemFn,
    role: &str,
    expected_payload: &str,
) -> Result<()> {
    let inner = result_inner_type(&method.sig.output, role)?;
    if type_path_ends_with(inner, expected_payload) {
        Ok(())
    } else {
        Err(syn::Error::new(
            inner.span(),
            format!("{role} function must return Result<{expected_payload}>"),
        ))
    }
}

/// Validate that a callback returns `Result<()>`.
fn validate_unit_result(method: &syn::ImplItemFn, role: &str) -> Result<()> {
    let inner = result_inner_type(&method.sig.output, role)?;
    if is_unit_type(inner) {
        Ok(())
    } else {
        Err(syn::Error::new(
            inner.span(),
            format!("{role} function must return Result<()>"),
        ))
    }
}

/// Validate the signature of a custom initialize function.
fn validate_custom_initialize_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    let method = find_impl_method(impl_block, fn_name, "initialize_fn")?;

    let params = validate_callback_signature(method, "initialize_fn", 5)?;
    validate_callback_arg_type(params[1], "initialize_fn", "&ServerCtx", is_server_ctx_type)?;
    validate_callback_arg_type(params[2], "initialize_fn", "String", is_string_type)?;
    validate_callback_arg_type(params[3], "initialize_fn", "ClientCapabilities", |ty| {
        type_path_ends_with(ty, "ClientCapabilities")
    })?;
    validate_callback_arg_type(params[4], "initialize_fn", "Implementation", |ty| {
        type_path_ends_with(ty, "Implementation")
    })?;
    validate_result_payload(method, "initialize_fn", "InitializeResult")
}

/// Validate the signature of a list resources callback.
fn validate_resources_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    let method = find_impl_method(impl_block, fn_name, "resources_fn")?;
    let params = validate_callback_signature(method, "resources_fn", 3)?;
    validate_callback_arg_type(params[1], "resources_fn", "&ServerCtx", is_server_ctx_type)?;
    validate_callback_arg_type(
        params[2],
        "resources_fn",
        "Option<Cursor>",
        is_option_cursor_type,
    )?;
    validate_result_payload(method, "resources_fn", "ListResourcesResult")
}

/// Validate the signature of a read resource callback.
fn validate_read_resource_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    let method = find_impl_method(impl_block, fn_name, "read_resource_fn")?;
    let params = validate_callback_signature(method, "read_resource_fn", 3)?;
    validate_callback_arg_type(
        params[1],
        "read_resource_fn",
        "&ServerCtx",
        is_server_ctx_type,
    )?;
    validate_callback_arg_type(params[2], "read_resource_fn", "String", is_string_type)?;
    validate_result_payload(method, "read_resource_fn", "ReadResourceResult")
}

/// Validate the signature of a list resource templates callback.
fn validate_resource_templates_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    let method = find_impl_method(impl_block, fn_name, "resource_templates_fn")?;
    let params = validate_callback_signature(method, "resource_templates_fn", 3)?;
    validate_callback_arg_type(
        params[1],
        "resource_templates_fn",
        "&ServerCtx",
        is_server_ctx_type,
    )?;
    validate_callback_arg_type(
        params[2],
        "resource_templates_fn",
        "Option<Cursor>",
        is_option_cursor_type,
    )?;
    validate_result_payload(
        method,
        "resource_templates_fn",
        "ListResourceTemplatesResult",
    )
}

/// Validate the signature of a shutdown callback.
fn validate_shutdown_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    let method = find_impl_method(impl_block, fn_name, "shutdown_fn")?;
    validate_callback_signature(method, "shutdown_fn", 1)?;
    validate_unit_result(method, "shutdown_fn")
}

/// Validate all optional ServerHandler forwarding callbacks.
fn validate_server_forwarders(impl_block: &ItemImpl, args: &ServerMacroArgs) -> Result<()> {
    if let Some(ref resources_fn) = args.resources_fn {
        validate_resources_fn(impl_block, resources_fn)?;
    }
    if let Some(ref read_resource_fn) = args.read_resource_fn {
        validate_read_resource_fn(impl_block, read_resource_fn)?;
    }
    if let Some(ref resource_templates_fn) = args.resource_templates_fn {
        validate_resource_templates_fn(impl_block, resource_templates_fn)?;
    }
    if let Some(ref shutdown_fn) = args.shutdown_fn {
        validate_shutdown_fn(impl_block, shutdown_fn)?;
    }
    Ok(())
}

/// Parse the #[mcp_server] macro inputs and emit the expanded tokens.
fn inner_mcp_server(attr: TokenStream, input: &TokenStream) -> Result<TokenStream> {
    // Parse macro attributes
    let args = syn::parse2::<ServerMacroArgs>(attr)?;
    let (impl_block, info) = parse_impl_block(input)?;

    if args.toolset.is_none() && !info.groups.is_empty() {
        return Err(syn::Error::new(
            input.span(),
            "#[group] methods require #[mcp_server(toolset = \"field\")]",
        ));
    }

    if args.toolset.is_none() && info.tools.is_empty() && !args.has_resource_callbacks() {
        return Err(syn::Error::new(
            input.span(),
            "No tool methods or resource callbacks found. Use #[tool] or a resource callback argument",
        ));
    }

    // Validate custom initialize function if provided
    if let Some(ref init_fn) = args.initialize_fn {
        validate_custom_initialize_fn(&impl_block, init_fn)?;
    }
    validate_server_forwarders(&impl_block, &args)?;

    let struct_name = syn::Ident::new(&info.struct_name, proc_macro2::Span::call_site());
    let toolset_field = args.toolset.as_ref();
    let call_tool = if let Some(toolset_field) = toolset_field {
        generate_toolset_call_tool(&info, toolset_field)
    } else {
        generate_call_tool(&info)
    };
    let list_tools = if let Some(toolset_field) = toolset_field {
        generate_toolset_list_tools(toolset_field)
    } else {
        generate_list_tools(&info)
    };
    let initialize = if toolset_field.is_some() {
        generate_toolset_initialize(&info, &args, args.initialize_fn.as_ref())
    } else {
        generate_initialize(&info, &args, args.initialize_fn.as_ref())
    };
    let server_forwarders = generate_server_forwarders(&args);
    let flat_structs = generate_flat_arg_structs(&info);

    if let Some(toolset_field) = toolset_field {
        let ensure_registered = generate_toolset_ensure_registered(&info, toolset_field);
        Ok(quote! {
            #(#flat_structs)*
            #impl_block

            impl #struct_name {
                #ensure_registered
            }

            #[async_trait::async_trait]
            impl tmcp::ServerHandler for #struct_name {
                #initialize
                #server_forwarders
                #list_tools
                #call_tool
            }
        })
    } else {
        Ok(quote! {
            #(#flat_structs)*
            #impl_block

            #[async_trait::async_trait]
            impl tmcp::ServerHandler for #struct_name {
                #initialize
                #server_forwarders
                #list_tools
                #call_tool
            }
        })
    }
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

/// Mark a group impl block or group factory method.
///
/// Apply `#[group]` to a group impl block to generate ToolSet registration
/// and dispatch glue. Use `#[group]` on methods that return child groups.
/// When applied to a struct alongside `#[derive(Group)]`, the attribute
/// supplies group metadata.
#[proc_macro_attribute]
pub fn group(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr_tokens = TokenStream::from(attr);
    let input_tokens = TokenStream::from(input);

    if let Ok(item_impl) = syn::parse2::<ItemImpl>(input_tokens.clone()) {
        if !attr_tokens.is_empty() {
            return syn::Error::new(
                item_impl.impl_token.span(),
                "#[group] on impl blocks does not accept arguments",
            )
            .to_compile_error()
            .into();
        }
        let impl_tokens = quote! { #item_impl };
        return match expand_group_impl(&impl_tokens) {
            Ok(tokens) => tokens.into(),
            Err(err) => err.to_compile_error().into(),
        };
    }

    if let Ok(mut item) = syn::parse2::<syn::DeriveInput>(input_tokens.clone()) {
        if !attr_tokens.is_empty() {
            let meta_attr: syn::Attribute = syn::parse_quote! { #[tmcp_group_meta(#attr_tokens)] };
            item.attrs.push(meta_attr);
        }
        return quote!(#item).into();
    }

    if let Ok(method) = syn::parse2::<syn::ImplItemFn>(input_tokens.clone()) {
        return quote!(#method).into();
    }

    input_tokens.into()
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

/// Derive `Group` for ToolSet-backed tool groups.
#[proc_macro_derive(Group, attributes(tmcp_group_meta, group))]
pub fn derive_group(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    if !matches!(input.data, syn::Data::Struct(_)) {
        return syn::Error::new(input.ident.span(), "Group can only be derived for structs")
            .to_compile_error()
            .into();
    }
    let ident = &input.ident;

    let doc_description = extract_doc_comment(&input.attrs);
    let meta = match parse_group_meta(&input.attrs) {
        Ok(meta) => meta,
        Err(err) => return err.to_compile_error().into(),
    };

    let default_name = ident.to_string().to_snake_case();
    let name = meta.name.unwrap_or(default_name);
    if name.is_empty() || name.contains('.') {
        return syn::Error::new(
            ident.span(),
            "group name must be non-empty and must not contain '.'",
        )
        .to_compile_error()
        .into();
    }
    let description = meta.description.unwrap_or(doc_description);
    let show_deactivator = meta.show_deactivator.unwrap_or(true);

    let on_activate_method = meta.on_activate.clone();
    let on_deactivate_method = meta.on_deactivate;
    let requires_clone = on_activate_method.is_some() || on_deactivate_method.is_some();

    let on_activate = if let Some(method) = on_activate_method {
        quote! {
            Some(Box::new({
                let group = self.clone();
                move |ctx| {
                    let group = group.clone();
                    let ctx = ctx.clone();
                    Box::pin(async move { group.#method(&ctx).await })
                }
            }))
        }
    } else {
        quote! { None }
    };

    let on_deactivate = if let Some(method) = on_deactivate_method {
        quote! {
            Some(Box::new({
                let group = self.clone();
                move |ctx| {
                    let group = group.clone();
                    let ctx = ctx.clone();
                    Box::pin(async move { group.#method(&ctx).await })
                }
            }))
        }
    } else {
        quote! { None }
    };

    let mut generics = input.generics.clone();
    if requires_clone {
        let type_generics = {
            let (_, ty_generics, _) = generics.split_for_impl();
            quote! { #ty_generics }
        };
        let where_clause = generics.make_where_clause();
        where_clause.predicates.push(syn::parse_quote!(
            #ident #type_generics: Clone + Send + Sync + 'static
        ));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let expanded = quote! {
        impl #impl_generics tmcp::Group for #ident #ty_generics #where_clause {
            fn config(&self) -> tmcp::GroupConfig {
                tmcp::GroupConfig {
                    name: #name.to_string(),
                    description: #description.to_string(),
                    parent: None,
                    on_activate: #on_activate,
                    on_deactivate: #on_deactivate,
                    show_deactivator: #show_deactivator,
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
            groups: Vec::new(),
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
            groups: Vec::new(),
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
    fn test_full_macro_expansion_dynamic_resources() {
        let input = quote! {
            /// Test server
            impl TestServer {
                async fn docs(
                    &self,
                    context: &ServerCtx,
                    cursor: Option<schema::Cursor>,
                ) -> Result<schema::ListResourcesResult> {
                    Ok(schema::ListResourcesResult::new())
                }

                async fn doc(
                    &self,
                    context: &ServerCtx,
                    uri: String,
                ) -> Result<schema::ReadResourceResult> {
                    Ok(schema::ReadResourceResult::new())
                }

                async fn doc_templates(
                    &self,
                    context: &ServerCtx,
                    cursor: Option<schema::Cursor>,
                ) -> Result<schema::ListResourceTemplatesResult> {
                    Ok(schema::ListResourceTemplatesResult::new())
                }

                async fn shutdown(&self) -> Result<()> {
                    Ok(())
                }
            }
        };

        let attrs = quote! {
            resources_fn = docs,
            read_resource_fn = doc,
            resource_templates_fn = doc_templates,
            shutdown_fn = shutdown
        };
        let result = inner_mcp_server(attrs, &input).unwrap();
        let result_str = result.to_string();

        assert!(result_str.contains("async fn list_resources"));
        assert!(result_str.contains("self . docs (context , cursor) . await"));
        assert!(result_str.contains("async fn read_resource"));
        assert!(result_str.contains("self . doc (context , uri) . await"));
        assert!(result_str.contains("async fn list_resource_templates"));
        assert!(result_str.contains("self . doc_templates (context , cursor) . await"));
        assert!(result_str.contains("async fn on_shutdown"));
        assert!(result_str.contains("self . shutdown () . await"));
        assert!(result_str.contains("with_resources"));
        assert!(!result_str.contains("with_tools"));
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
                .contains("No tool methods or resource callbacks found")
        );
    }

    #[test]
    fn test_missing_resource_callback_error() {
        let input = quote! {
            impl TestServer {
                async fn helper(&self) -> Result<()> {
                    Ok(())
                }
            }
        };

        let result = inner_mcp_server(quote! { resources_fn = docs }, &input);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("resources_fn function 'docs' not found")
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
