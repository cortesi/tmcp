//! Expansion of plain (non-toolset) `#[mcp_server]` impl blocks.

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::ext::IdentExt;

use crate::{
    codegen::{
        build_tool_expr, delegated_descriptor_path, generate_tool_call_arm, tool_schema_expr,
    },
    model::{ForwarderParam, ServerInfo, ServerMacroArgs, ToolTaskSupport},
    parse::{generic_param_idents, tokens_mention_ident},
};

/// Generate the ServerHandler::call_tool implementation.
pub fn generate_call_tool(info: &ServerInfo, args: &ServerMacroArgs) -> TokenStream {
    let receiver = quote! { self };
    let tool_matches = info
        .tools
        .iter()
        .map(|tool| generate_tool_call_arm(tool, &receiver, &info.struct_name));
    let resolver = args.tool_state_fn.as_ref();
    let delegated_matches = args.tools.iter().map(|path| {
        let name = path
            .segments
            .last()
            .expect("tool paths are non-empty")
            .ident
            .unraw()
            .to_string();
        let descriptor = delegated_descriptor_path(path);
        quote! {
            #name => {
                let state = match self.#resolver(arguments.as_ref()).await {
                    Ok(state) => state,
                    Err(error) => {
                        let result: ::tmcp::schema::CallToolResult = error.into();
                        return Ok(::tmcp::schema::CallToolResponse::Result(result));
                    }
                };
                #descriptor::call(state, context, arguments, task).await
            }
        }
    });

    quote! {
        async fn call_tool(
            &self,
            context: &::tmcp::ServerCtx,
            name: String,
            arguments: Option<::tmcp::Arguments>,
            task: Option<::tmcp::schema::TaskMetadata>,
        ) -> ::tmcp::Result<::tmcp::schema::CallToolResponse> {
            let _ = &task;
            match name.as_str() {
                #(#tool_matches)*
                #(#delegated_matches)*
                _ => Err(::tmcp::Error::ToolNotFound(name))
            }
        }
    }
}

/// Generate the ServerHandler::list_tools implementation.
///
/// The tool list (including schemars schema generation) is memoized in a
/// `LazyLock` when the impl block has no type or const generics; otherwise the
/// schema expressions may reference generic parameters and must be evaluated
/// per call.
pub fn generate_list_tools(info: &ServerInfo, args: &ServerMacroArgs) -> TokenStream {
    let mut tools: Vec<_> = info
        .tools
        .iter()
        .map(|tool| {
            let name = &tool.name;
            let name_expr = quote! { #name };
            let schema = tool_schema_expr(tool, &info.struct_name);
            build_tool_expr(tool, &name_expr, &schema)
        })
        .collect();
    tools.extend(args.tools.iter().map(|path| {
        let descriptor = delegated_descriptor_path(path);
        quote! { #descriptor::schema() }
    }));

    let tools_expr = quote! { vec![#(#tools),*] };
    let generic_idents = generic_param_idents(&info.generics);
    let body = if !tokens_mention_ident(tools_expr.clone(), &generic_idents) {
        quote! {
            static TOOLS: ::std::sync::LazyLock<Vec<::tmcp::schema::Tool>> =
                ::std::sync::LazyLock::new(|| #tools_expr);
            let tools = TOOLS.clone();
        }
    } else {
        quote! {
            let tools = #tools_expr;
        }
    };

    quote! {
        async fn list_tools(
            &self,
            _context: &::tmcp::ServerCtx,
            _cursor: Option<::tmcp::schema::Cursor>,
        ) -> ::tmcp::Result<::tmcp::schema::ListToolsResult> {
            #body
            Ok(::tmcp::schema::ListToolsResult {
                tools,
                next_cursor: None,
                _meta: None,
                _extra: Default::default(),
            })
        }
    }
}

/// Determine if task-augmented tool calls should be advertised.
fn local_tools_support_tasks(info: &ServerInfo) -> bool {
    info.tools.iter().any(|tool| {
        matches!(
            tool.attrs.task_support,
            Some(ToolTaskSupport::Optional | ToolTaskSupport::Required)
        )
    })
}

/// How the generated initializer should advertise tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolCapability {
    /// Do not advertise tools.
    Omit,
    /// Advertise a static tool list without list change notifications.
    Static,
    /// Advertise a dynamic tool list with list change notifications.
    Dynamic,
}

/// Generate the ServerHandler::initialize implementation.
///
/// Toolset-backed servers run the registration guard before initializing and
/// advertise a dynamic tool list. A custom `initialize_fn` replaces the
/// default initializer entirely.
pub fn generate_initialize(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    toolset: bool,
) -> TokenStream {
    let prologue = if toolset {
        quote! { self.__ensure_tools_registered()?; }
    } else {
        quote! {}
    };

    if let Some(init_fn) = &args.initialize_fn {
        return quote! {
            async fn initialize(
                &self,
                context: &::tmcp::ServerCtx,
                protocol_version: ::tmcp::schema::ProtocolVersion,
                capabilities: ::tmcp::schema::ClientCapabilities,
                client_info: ::tmcp::schema::Implementation,
            ) -> ::tmcp::Result<::tmcp::schema::InitializeResult> {
                #prologue
                self.#init_fn(context, protocol_version, capabilities, client_info).await
            }
        };
    }

    let tools_capability = if toolset {
        ToolCapability::Dynamic
    } else if info.tools.is_empty() && args.tools.is_empty() {
        ToolCapability::Omit
    } else {
        ToolCapability::Static
    };
    generate_default_initialize(info, args, tools_capability, &prologue)
}

/// Generate the default initialize implementation with a custom prologue.
fn generate_default_initialize(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    tools_capability: ToolCapability,
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

    let tools_capability_setter = match tools_capability {
        ToolCapability::Omit => quote! {},
        ToolCapability::Static => quote! { init = init.with_tools(None); },
        ToolCapability::Dynamic => quote! { init = init.with_tools(Some(true)); },
    };

    let resources_capability_setter = if args.has_resource_callbacks() {
        let list_changed = args.resources_list_changed();
        quote! { init = init.with_resources(Some(false), Some(#list_changed)); }
    } else {
        quote! {}
    };

    let delegated_task_support = args.tools.iter().map(|path| {
        let descriptor = delegated_descriptor_path(path);
        quote! { #descriptor::SUPPORTS_TASKS }
    });
    let local_task_support = local_tools_support_tasks(info);
    let task_tools_call_setter = if args.tools.is_empty() {
        if local_task_support {
            quote! { init = init.with_task_tools_call(); }
        } else {
            quote! {}
        }
    } else {
        quote! {
            if #local_task_support #( || #delegated_task_support )* {
                init = init.with_task_tools_call();
            }
        }
    };

    let task_list_setter = if args.tasks_list() {
        quote! { init = init.with_tasks_list(); }
    } else {
        quote! {}
    };

    let task_cancel_setter = if args.tasks_cancel() {
        quote! { init = init.with_tasks_cancel(); }
    } else {
        quote! {}
    };

    quote! {
        async fn initialize(
            &self,
            _context: &::tmcp::ServerCtx,
            protocol_version: ::tmcp::schema::ProtocolVersion,
            _capabilities: ::tmcp::schema::ClientCapabilities,
            _client_info: ::tmcp::schema::Implementation,
        ) -> ::tmcp::Result<::tmcp::schema::InitializeResult> {
            #prologue
            let mut init = ::tmcp::schema::InitializeResult::new(#name_expr)
                .with_version(#version_expr);
            #tools_capability_setter
            #resources_capability_setter
            #task_tools_call_setter
            #task_list_setter
            #task_cancel_setter
            #instructions_setter
            init = init.with_mcp_version(protocol_version);
            Ok(init)
        }
    }
}

/// Generate all configured ServerHandler forwarding methods.
///
/// Each forwarder's trait method signature and forwarding call are derived
/// from its [`crate::model::ForwarderSpec`] table entry.
pub fn generate_server_forwarders(args: &ServerMacroArgs) -> TokenStream {
    let methods = args.forwarders.iter().map(|binding| {
        let spec = binding.spec;
        let fn_name = &binding.fn_name;
        let trait_method = format_ident!("{}", spec.trait_method);

        let mut param_decls = Vec::new();
        let mut forwarded = Vec::new();
        for shape in spec.params {
            match shape {
                ForwarderParam::Ctx => {
                    param_decls.push(quote! { context: &::tmcp::ServerCtx });
                    forwarded.push(quote! { context });
                }
                ForwarderParam::Str(name) => {
                    let ident = format_ident!("{}", name);
                    param_decls.push(quote! { #ident: String });
                    forwarded.push(quote! { #ident });
                }
                ForwarderParam::Cursor => {
                    param_decls.push(quote! { cursor: Option<::tmcp::schema::Cursor> });
                    forwarded.push(quote! { cursor });
                }
            }
        }

        let return_type = match spec.payload {
            Some(payload) => {
                let payload = format_ident!("{}", payload);
                quote! { ::tmcp::Result<::tmcp::schema::#payload> }
            }
            None => quote! { ::tmcp::Result<()> },
        };

        quote! {
            async fn #trait_method(&self #(, #param_decls)*) -> #return_type {
                self.#fn_name(#(#forwarded),*).await
            }
        }
    });

    quote! {
        #(#methods)*
    }
}
