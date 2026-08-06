//! Macros to make defining MCP servers easier
//!
//! Using the `#[mcp_server]` macro on an impl block, this crate picks up all methods
//! marked with `#[tool]` and derives the necessary `ServerHandler::call_tool`,
//! `ServerHandler::list_tools` and `ServerHandler::initialize` methods. Resource callbacks
//! can be wired into the same generated `ServerHandler` implementation for servers whose
//! resources are discovered dynamically. The name of the server is derived from the name of
//! the struct converted to snake_case (e.g., MyServer becomes my_server), and the
//! description is derived from the doc comment on the impl block. The version defaults to
//! the consuming crate's `CARGO_PKG_VERSION`.
//!
//! The macro supports customization through attributes:
//! - `initialize_fn`: Specify a custom initialize function instead of using the default
//! - `name`: Override the server name used in initialization
//! - `version`: Override the server version used in initialization
//! - `instructions`: Override the server instructions used in initialization
//! - `toolset`: Use a ToolSet field for progressive discovery
//! - `tools`: Delegate a static list of tools to annotated free functions
//! - `tool_groups`: Delegate generated groups of annotated free functions
//! - `tool_state_fn`: Resolve state for the delegated free functions
//! - `tool_state_param`: Add one raw state-selector argument to delegated tool schemas
//! - `resources_fn`: Forward `resources/list` to an async method
//! - `read_resource_fn`: Forward `resources/read` to an async method
//! - `resource_templates_fn`: Forward `resources/templates/list` to an async method
//! - `shutdown_fn`: Forward shutdown handling to an async method
//! - `get_task_fn`: Forward `tasks/get` to an async method
//! - `get_task_payload_fn`: Forward `tasks/result` to an async method
//! - `list_tasks_fn`: Forward `tasks/list` to an async method
//! - `cancel_task_fn`: Forward `tasks/cancel` to an async method
//!
//! All tool methods must be async and have one of the following signatures:
//!
//! ```ignore
//! async fn tool_name(&self, context: &ServerCtx, params: ToolParams) -> Result<schema::CallToolResult>
//! async fn task_tool(&self, context: &ServerCtx, task: schema::TaskMetadata, params: ToolParams) -> Result<schema::CreateTaskResult>
//! async fn maybe_task_tool(&self, context: &ServerCtx, task: Option<schema::TaskMetadata>, params: ToolParams) -> Result<schema::CallToolResponse>
//! async fn tool_name(&self, context: &ServerCtx, params: ToolParams) -> ToolResult<T>
//! async fn tool_name(&self, context: &ServerCtx) -> ToolResult<T>
//! async fn tool_name(&self, params: ToolParams) -> ToolResult<T>
//! async fn tool_name(&self) -> ToolResult<T>
//! async fn tool_name(&self, context: &ServerCtx, a: Type1, b: Type2) -> ToolResult<T>
//! async fn tool_name(&self, a: Type1, b: Type2) -> ToolResult<T>
//! async fn delegated(state: &State, context: &ServerCtx, params: ToolParams) -> ToolResult<T>
//! ```
//!
//! The `context: &ServerCtx` parameter is optional. Tools may also omit parameters or
//! accept `()` for parameter-less tools.
//! Single-argument tools still default to struct-style parameters; use `#[tool(flat)]`
//! to force flat argument handling for one-argument tools.
//!
//! A delegated function uses the same optional context, task, parameter, and return shapes.
//! Its first parameter is a shared state reference. The server resolver receives
//! unchanged `Option<&Arguments>` before typed decoding and returns `ToolResult<State>`.
//! `tool_state_param = (name: Type, "Description.")` adds one required argument to each
//! delegated tool schema.
//!
//! The parameter struct (ToolParams in this example) must implement `schemars::JsonSchema`
//! and `serde::Deserialize`.
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
//! The `#[tool]` attribute can also accept metadata that feeds into tool annotations and
//! execution hints. Supported arguments:
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
//!         protocol_version: schema::ProtocolVersion,
//!         capabilities: schema::ClientCapabilities,
//!         client_info: schema::Implementation,
//!     ) -> Result<schema::InitializeResult> {
//!         Ok(schema::InitializeResult {
//!             protocol_version,
//!             capabilities: schema::ServerCapabilities {
//!                 tools: Some(schema::ToolsCapability {
//!                     list_changed: Some(true),
//!                 }),
//!                 ..Default::default()
//!             },
//!             server_info: schema::Implementation::new("my_custom_server", "2.0.0"),
//!             instructions: Some("Custom server with advanced features".to_string()),
//!             _meta: None,
//!             _extra: Default::default(),
//!         })
//!     }
//!
//!     #[tool]
//!     async fn my_tool(&self, context: &ServerCtx, params: MyParams) -> Result<schema::CallToolResult> {
//!         // Tool implementation
//!     }
//! }
//! ```
//!
//! # Progressive discovery with ToolSet and groups
//!
//! Servers that expose many tools can register them in a `tmcp::ToolSet` field and reveal
//! them progressively. Pass the field name via `toolset`:
//!
//! ```ignore
//! #[derive(Default)]
//! struct Workbench {
//!     tools: tmcp::ToolSet,
//! }
//!
//! #[mcp_server(toolset = tools)]
//! impl Workbench {
//!     /// Tools marked `always` stay visible; others appear when their group activates.
//!     #[tool(always)]
//!     async fn status(&self) -> ToolResult<Status> { /* ... */ }
//!
//!     /// Factory methods marked #[group] register a child group of tools.
//!     #[group]
//!     fn math(&self) -> MathGroup {
//!         MathGroup::default()
//!     }
//! }
//! ```
//!
//! A group is a struct that derives `Group` and carries its tools in a `#[group]` impl
//! block. The group name defaults to the snake_case struct name and the description to the
//! struct's doc comment; both can be overridden, along with activation hooks, through the
//! `#[group(...)]` attribute on the struct:
//!
//! ```ignore
//! /// Arithmetic helpers.
//! #[derive(Clone, Default, Group)]
//! #[group(name = "math", on_activate = setup)]
//! struct MathGroup;
//!
//! #[group]
//! impl MathGroup {
//!     async fn setup(&self, ctx: &ServerCtx) -> Result<()> { Ok(()) }
//!
//!     #[tool]
//!     async fn add(&self, a: f64, b: f64) -> ToolResult<Sum> { /* ... */ }
//! }
//! ```
//!
//! On a `#[group]` factory method, the attribute accepts only `name = "..."` to override
//! the path segment for that edge. Group tools are addressed as `group.tool` (nested groups
//! as `parent.child.tool`).

#![warn(missing_docs)]

use proc_macro2::TokenStream;

/// Code generation for servers, toolsets, and groups.
mod codegen;
/// Code generation for delegating server-handler impls.
mod delegate;
/// Derive expansion for tool response types and derive-merging attributes.
mod derives;
/// Data model shared between parsing, validation, and code generation.
mod model;
/// Parsing of annotated impl blocks, attributes, and tool signatures.
mod parse;
/// Field-injection macros used internally by tmcp's schema module.
mod schema_ext;
/// Validation of forwarder-callback signatures.
mod validate;

/// Derive the ServerHandler methods from an impl block.
#[proc_macro_attribute]
pub fn mcp_server(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr_tokens = TokenStream::from(attr);
    let input_tokens = TokenStream::from(input);

    match codegen::expand_mcp_server(attr_tokens, &input_tokens) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Delegate each omitted `ServerHandler` method to one inner handler.
#[proc_macro_attribute]
pub fn delegate_server_handler(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match delegate::expand(TokenStream::from(attr), TokenStream::from(input)) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Mark a server method or delegated free function as an MCP tool.
#[proc_macro_attribute]
pub fn tool(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr = TokenStream::from(attr);
    let input = TokenStream::from(input);
    if syn::parse2::<syn::ItemFn>(input.clone())
        .is_ok_and(|function| matches!(function.sig.inputs.first(), Some(syn::FnArg::Receiver(_))))
    {
        return input.into();
    }
    match codegen::expand_free_tool(&attr, &input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// Generate a static group of delegated free tools.
///
/// The `state` type must match the shared state returned by the enclosing
/// server's `tool_state_fn`. Every path in `tools` must name a free function
/// annotated with [`tool`].
#[proc_macro_attribute]
pub fn tool_group(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    match codegen::tool_group::expand_tool_group(
        &TokenStream::from(attr),
        &TokenStream::from(input),
    ) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
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
    codegen::group::expand_group_attribute(&TokenStream::from(attr), &TokenStream::from(input))
        .into()
}

/// Add serde + schemars derives for tool parameter structs.
#[proc_macro_attribute]
pub fn tool_params(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as syn::DeriveInput);
    derives::expand_tool_params(item).into()
}

/// Add serde + schemars derives plus ToolResponse for tool result structs.
#[proc_macro_attribute]
pub fn tool_result(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as syn::DeriveInput);
    derives::expand_tool_result(item).into()
}

/// Derive `ToolResponse` by encoding the type as structured content.
#[proc_macro_derive(ToolResponse)]
pub fn derive_tool_response(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    derives::expand_derive_tool_response(&input).into()
}

/// Derive `Group` for ToolSet-backed tool groups.
#[proc_macro_derive(Group, attributes(tmcp_group_meta, group))]
pub fn derive_group(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    codegen::group::expand_derive_group(&input).into()
}

/// Adds the MCP `_meta` field to a struct with proper serde attributes and builder methods.
///
/// Internal use only: this macro expands inside the tmcp crate's schema module and is not
/// part of the public macro API.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn with_meta(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as syn::DeriveInput);
    schema_ext::expand_meta(item, false).into()
}

/// Adds MCP `_meta` plus flattened extension fields for protocol objects that inherit
/// MCP `Result` or are otherwise open in the TypeScript schema.
///
/// Internal use only: this macro expands inside the tmcp crate's schema module and is not
/// part of the public macro API.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn with_open_meta(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as syn::DeriveInput);
    schema_ext::expand_meta(item, true).into()
}

/// Adds name and title fields to a struct with proper serde attributes, documentation, and
/// builder methods.
///
/// Internal use only: this macro expands inside the tmcp crate's schema module and is not
/// part of the public macro API.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn with_basename(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(input as syn::DeriveInput);
    schema_ext::expand_basename(item).into()
}
