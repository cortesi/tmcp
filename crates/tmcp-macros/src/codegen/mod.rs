//! Code generation for the tmcp macros.
//!
//! All generated code references the host crate through absolute `::tmcp::`
//! paths, and reaches serde/schemars/async-trait through the hidden
//! `::tmcp::__private` re-exports so consumers do not need direct dependencies
//! on those crates.

pub mod group;
pub mod server;
pub mod tool_group;
pub mod toolset;

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Result, ext::IdentExt, spanned::Spanned};

use crate::{
    model::{
        ParamsKind, ServerInfo, ServerMacroArgs, TaskParamKind, ToolMethod, ToolReturnKind,
        ToolTaskSupport,
    },
    parse::{is_call_tool_result_type, parse_free_tool_function, parse_impl_block},
    validate::{validate_custom_initialize_fn, validate_server_forwarders, validate_tool_state_fn},
};

/// Add the declared state parameter to one delegated tool schema.
fn with_tool_state_param(tool: TokenStream, args: &ServerMacroArgs) -> TokenStream {
    let Some(param) = &args.tool_state_param else {
        return tool;
    };
    let name = param.name.unraw().to_string();
    let ty = &param.ty;
    let description = &param.description;
    quote! {
        {
            let mut tool = #tool;
            tool.input_schema = tool.input_schema
                .with_required_property::<#ty>(#name, #description);
            tool
        }
    }
}

/// Return the hidden descriptor type path for a delegated tool function path.
fn delegated_descriptor_path(path: &syn::Path) -> syn::Path {
    let mut path = path.clone();
    let last = path.segments.last_mut().expect("tool paths are non-empty");
    last.ident = format_ident!("__TmcpTool_{}", last.ident.unraw());
    path
}

/// Expand a free tool function and its schema and dispatch descriptor.
pub fn expand_free_tool(attr: &TokenStream, input: &TokenStream) -> Result<TokenStream> {
    let function = syn::parse2::<syn::ItemFn>(input.clone())?;
    let info = parse_free_tool_function(attr, &function)?;
    let tool = &info.tool;
    let function_name = &tool.ident;
    let descriptor_name = format_ident!("__TmcpTool_{}", function_name.unraw());
    let visibility = &info.visibility;
    let state_ty = &info.state_ty;
    let name = &tool.name;
    let name_expr = quote! { Self::NAME };
    let schema = tool_schema_expr(tool, &tool.name);
    let tool_expr = build_tool_expr(tool, &name_expr, &schema);
    let supports_tasks = matches!(
        tool.attrs.task_support,
        Some(ToolTaskSupport::Optional | ToolTaskSupport::Required)
    );
    let defaults = tool.attrs.defaults;
    let ctx_arg = tool.has_ctx.then(|| quote! { context, });
    let task_prelude = match tool.task_param {
        TaskParamKind::None => {
            let message = format!("tool '{name}' does not accept task-augmented calls");
            quote! {
                if task.is_some() {
                    return Err(::tmcp::Error::InvalidParams(#message.to_string()));
                }
            }
        }
        TaskParamKind::Required => quote! {
            let task = task.ok_or_else(|| {
                ::tmcp::Error::InvalidParams("tool requires task metadata".to_string())
            })?;
        },
        TaskParamKind::Optional => quote! {},
    };
    let task_arg = match tool.task_param {
        TaskParamKind::None => quote! {},
        TaskParamKind::Required | TaskParamKind::Optional => quote! { task, },
    };
    let flat_struct = match &tool.params_kind {
        ParamsKind::Flat(params) => {
            let struct_ident = flat_args_struct_ident(&tool.name, &tool.name);
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
                #[allow(non_camel_case_types)]
                #[derive(
                    ::tmcp::__private::serde::Deserialize,
                    ::tmcp::__private::schemars::JsonSchema,
                )]
                #[serde(crate = "::tmcp::__private::serde")]
                #[schemars(crate = "::tmcp::__private::schemars")]
                struct #struct_ident {
                    #(#fields)*
                }
            }
        }
        _ => quote! {},
    };
    let (args_prelude, params_arg) = match &tool.params_kind {
        ParamsKind::None => (quote! { let _ = arguments; }, quote! {}),
        ParamsKind::Unit => (quote! { let _ = arguments; }, quote! { (), }),
        ParamsKind::Typed(params_type) => {
            let params_type = params_type.as_ref();
            (
                quote! {
                    let params: #params_type = match ::tmcp::Arguments::into_tool_params(
                        arguments,
                        #defaults,
                    ) {
                        Ok(params) => params,
                        Err(err) => {
                            let result: ::tmcp::schema::CallToolResult = err.into();
                            return Ok(::tmcp::schema::CallToolResponse::Result(result));
                        }
                    };
                },
                quote! { params, },
            )
        }
        ParamsKind::Flat(params) => {
            let struct_ident = flat_args_struct_ident(&tool.name, &tool.name);
            let param_idents: Vec<_> = params.iter().map(|param| &param.ident).collect();
            (
                quote! {
                    let params: #struct_ident = match ::tmcp::Arguments::into_tool_params(
                        arguments,
                        #defaults,
                    ) {
                        Ok(params) => params,
                        Err(err) => {
                            let result: ::tmcp::schema::CallToolResult = err.into();
                            return Ok(::tmcp::schema::CallToolResponse::Result(result));
                        }
                    };
                    let #struct_ident { #(#param_idents),* } = params;
                },
                quote! { #(#param_idents),*, },
            )
        }
    };
    let call_expr = quote! {
        #function_name(
            ::std::borrow::Borrow::borrow(&state),
            #ctx_arg
            #task_arg
            #params_arg
        ).await
    };
    let call = match &tool.return_kind {
        ToolReturnKind::CallResult => quote! {
            #task_prelude
            #args_prelude
            #call_expr.map(::tmcp::schema::CallToolResponse::Result)
        },
        ToolReturnKind::TaskResult => quote! {
            #task_prelude
            #args_prelude
            #call_expr.map(::tmcp::schema::CallToolResponse::Task)
        },
        ToolReturnKind::CallResponse => quote! {
            #task_prelude
            #args_prelude
            #call_expr
        },
        ToolReturnKind::ToolResult { .. } => quote! {
            #task_prelude
            #args_prelude
            let result = #call_expr;
            Ok(match result {
                Ok(value) => {
                    let result: ::tmcp::schema::CallToolResult = value.into();
                    ::tmcp::schema::CallToolResponse::Result(result)
                }
                Err(err) => {
                    let result: ::tmcp::schema::CallToolResult = err.into();
                    ::tmcp::schema::CallToolResponse::Result(result)
                }
            })
        },
    };

    Ok(quote! {
        #function

        #flat_struct

        #[doc(hidden)]
        #[allow(non_camel_case_types)]
        #visibility struct #descriptor_name;

        impl #descriptor_name {
            #[doc(hidden)]
            pub const NAME: &str = #name;
            #[doc(hidden)]
            pub const SUPPORTS_TASKS: bool = #supports_tasks;

            #[doc(hidden)]
            pub fn schema() -> ::tmcp::schema::Tool {
                #tool_expr
            }

            #[doc(hidden)]
            pub fn call<'a, S>(
                state: S,
                context: &'a ::tmcp::ServerCtx,
                arguments: Option<::tmcp::Arguments>,
                task: Option<::tmcp::schema::TaskMetadata>,
            ) -> ::tmcp::ToolCallFuture<'a>
            where
                S: ::std::borrow::Borrow<#state_ty> + Send + 'a,
                #state_ty: Sync,
            {
                ::std::boxed::Box::pin(async move { #call })
            }
        }
    })
}

/// Generate a unique struct identifier for flat tool arguments.
fn flat_args_struct_ident(server_name: &str, tool_name: &str) -> syn::Ident {
    let server = server_name.trim_start_matches("r#");
    let tool = tool_name.trim_start_matches("r#");
    format_ident!("__TmcpToolArgs_{}_{}", server, tool)
}

/// Generate a tool call match arm for a receiver expression.
fn generate_tool_call_arm(
    tool: &ToolMethod,
    receiver: &TokenStream,
    owner_name: &str,
) -> TokenStream {
    let name = &tool.name;
    let method = &tool.ident;
    let defaults = tool.attrs.defaults;
    let ctx_arg = if tool.has_ctx {
        quote! { context, }
    } else {
        quote! {}
    };
    let task_prelude = match tool.task_param {
        TaskParamKind::None => {
            let message = format!("tool '{name}' does not accept task-augmented calls");
            quote! {
                if task.is_some() {
                    return Err(::tmcp::Error::InvalidParams(#message.to_string()));
                }
            }
        }
        TaskParamKind::Required => quote! {
            let task = task.ok_or_else(|| {
                ::tmcp::Error::InvalidParams("tool requires task metadata".to_string())
            })?;
        },
        TaskParamKind::Optional => quote! {},
    };
    let task_arg = match tool.task_param {
        TaskParamKind::None => quote! {},
        TaskParamKind::Required | TaskParamKind::Optional => quote! { task, },
    };
    let (args_prelude, call_expr) = match &tool.params_kind {
        ParamsKind::None => (
            quote! { let _ = arguments; },
            quote! { #receiver.#method(#ctx_arg #task_arg).await },
        ),
        ParamsKind::Unit => (
            quote! { let _ = arguments; },
            quote! { #receiver.#method(#ctx_arg #task_arg ()).await },
        ),
        ParamsKind::Typed(params_type) => {
            let params_type = params_type.as_ref();
            (
                quote! {
                    let params: #params_type = match ::tmcp::Arguments::into_tool_params(
                        arguments,
                        #defaults,
                    ) {
                        Ok(params) => params,
                        Err(err) => {
                            let result: ::tmcp::schema::CallToolResult = err.into();
                            return Ok(::tmcp::schema::CallToolResponse::Result(result));
                        }
                    };
                },
                quote! { #receiver.#method(#ctx_arg #task_arg params).await },
            )
        }
        ParamsKind::Flat(params) => {
            let struct_ident = flat_args_struct_ident(owner_name, name);
            let param_idents: Vec<_> = params.iter().map(|param| &param.ident).collect();

            (
                quote! {
                    let params: #struct_ident = match ::tmcp::Arguments::into_tool_params(
                        arguments,
                        #defaults,
                    ) {
                        Ok(params) => params,
                        Err(err) => {
                            let result: ::tmcp::schema::CallToolResult = err.into();
                            return Ok(::tmcp::schema::CallToolResponse::Result(result));
                        }
                    };
                    let #struct_ident { #(#param_idents),* } = params;
                },
                quote! { #receiver.#method(#ctx_arg #task_arg #(#param_idents),*).await },
            )
        }
    };

    let call = match &tool.return_kind {
        ToolReturnKind::CallResult => quote! {
            #task_prelude
            #args_prelude
            #call_expr.map(::tmcp::schema::CallToolResponse::Result)
        },
        ToolReturnKind::TaskResult => quote! {
            #task_prelude
            #args_prelude
            #call_expr.map(::tmcp::schema::CallToolResponse::Task)
        },
        ToolReturnKind::CallResponse => quote! {
            #task_prelude
            #args_prelude
            #call_expr
        },
        ToolReturnKind::ToolResult { .. } => quote! {
            #task_prelude
            #args_prelude
            let result = #call_expr;
            Ok(match result {
                Ok(value) => {
                    let result: ::tmcp::schema::CallToolResult = value.into();
                    ::tmcp::schema::CallToolResponse::Result(result)
                }
                Err(err) => {
                    let result: ::tmcp::schema::CallToolResult = err.into();
                    ::tmcp::schema::CallToolResponse::Result(result)
                }
            })
        },
    };

    quote! {
        #name => {
            #call
        }
    }
}

/// Build the input schema expression for a tool.
fn tool_schema_expr(tool: &ToolMethod, owner_name: &str) -> TokenStream {
    match tool.params_kind {
        ParamsKind::None | ParamsKind::Unit => {
            quote! { ::tmcp::schema::ToolSchema::default() }
        }
        ParamsKind::Typed(ref params_type) => {
            let params_type = params_type.as_ref();
            quote! { ::tmcp::schema::ToolSchema::from_json_schema::<#params_type>() }
        }
        ParamsKind::Flat(_) => {
            let struct_ident = flat_args_struct_ident(owner_name, &tool.name);
            quote! { ::tmcp::schema::ToolSchema::from_json_schema::<#struct_ident>() }
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
                    quote! { ::tmcp::schema::ToolTaskSupport::Forbidden }
                }
                ToolTaskSupport::Optional => {
                    quote! { ::tmcp::schema::ToolTaskSupport::Optional }
                }
                ToolTaskSupport::Required => {
                    quote! { ::tmcp::schema::ToolTaskSupport::Required }
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
            quote! { tool = tool.with_output_schema(::tmcp::schema::ToolSchema::from_json_schema::<#ty>()); }
        })
        .unwrap_or_default();

    let icons_setter = if tool.attrs.icons.is_empty() {
        quote! {}
    } else {
        let icons = tool
            .attrs
            .icons
            .iter()
            .map(|icon| quote! { ::tmcp::schema::Icon::new(#icon) });
        quote! {
            tool = tool.with_icons(vec![#(#icons),*]);
        }
    };

    quote! {
        {
            let mut tool = ::tmcp::schema::Tool::new(#name_expr, #schema_expr);
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
                #[derive(
                    ::tmcp::__private::serde::Deserialize,
                    ::tmcp::__private::schemars::JsonSchema,
                )]
                #[serde(crate = "::tmcp::__private::serde")]
                #[schemars(crate = "::tmcp::__private::schemars")]
                struct #struct_ident {
                    #(#fields)*
                }
            }
        })
        .collect()
}

/// Parse the #[mcp_server] macro inputs and emit the expanded tokens.
pub fn expand_mcp_server(attr: TokenStream, input: &TokenStream) -> Result<TokenStream> {
    // Parse macro attributes
    let args = syn::parse2::<ServerMacroArgs>(attr)?;
    let (impl_block, info) = parse_impl_block(input)?;

    if args.tools.is_empty() && args.tool_groups.is_empty() && args.tool_state_fn.is_some() {
        return Err(syn::Error::new(
            input.span(),
            "tool_state_fn requires at least one entry in tools or tool_groups",
        ));
    }
    if args.tool_state_param.is_some() && args.tool_state_fn.is_none() {
        return Err(syn::Error::new(
            input.span(),
            "tool_state_param requires tool_state_fn",
        ));
    }
    if (!args.tools.is_empty() || !args.tool_groups.is_empty()) && args.tool_state_fn.is_none() {
        return Err(syn::Error::new(
            input.span(),
            "delegated tools and tool groups require tool_state_fn",
        ));
    }
    let mut names = HashSet::new();
    for tool in &info.tools {
        names.insert(tool.name.clone());
    }
    for path in &args.tools {
        let segment = path.segments.last().expect("tool paths are non-empty");
        let name = segment.ident.unraw().to_string();
        if !names.insert(name.clone()) {
            return Err(syn::Error::new(
                segment.ident.span(),
                format!("duplicate tool name `{name}`"),
            ));
        }
    }

    if args.toolset.is_none() && !info.groups.is_empty() {
        return Err(syn::Error::new(
            input.span(),
            "#[group] methods require #[mcp_server(toolset = \"field\")]",
        ));
    }

    if args.toolset.is_none()
        && info.tools.is_empty()
        && args.tools.is_empty()
        && args.tool_groups.is_empty()
        && !args.has_resource_callbacks()
    {
        return Err(syn::Error::new(
            input.span(),
            "No tool methods, delegated tools, tool groups, or resource callbacks found. Use #[tool], tools, tool_groups, or a resource callback argument",
        ));
    }

    // Validate custom initialize function if provided
    if let Some(ref init_fn) = args.initialize_fn {
        validate_custom_initialize_fn(&impl_block, init_fn)?;
    }
    validate_server_forwarders(&impl_block, &args)?;
    if let Some(tool_state_fn) = &args.tool_state_fn {
        validate_tool_state_fn(&impl_block, tool_state_fn)?;
    }

    let self_ty = &info.self_ty;
    let (impl_generics, _, where_clause) = info.generics.split_for_impl();
    let toolset_field = args.toolset.as_ref();
    let call_tool = if let Some(toolset_field) = toolset_field {
        toolset::generate_toolset_call_tool(&info, &args, toolset_field)
    } else {
        server::generate_call_tool(&info, &args)
    };
    let list_tools = if let Some(toolset_field) = toolset_field {
        toolset::generate_toolset_list_tools(toolset_field)
    } else {
        server::generate_list_tools(&info, &args)
    };
    let initialize = server::generate_initialize(&info, &args, toolset_field.is_some());
    let server_forwarders = server::generate_server_forwarders(&args);
    let flat_structs = generate_flat_arg_structs(&info);
    let inherent_names = info.tools.iter().map(|tool| &tool.name);

    let names_impl = quote! {
        impl #impl_generics #self_ty #where_clause {
            /// Exact names of tools declared as methods on this server.
            pub const NAMES: &'static [&'static str] = &[#(#inherent_names),*];
        }
    };

    let ensure_registered_impl = toolset_field
        .map(|toolset_field| {
            let ensure_registered =
                toolset::generate_toolset_ensure_registered(&info, &args, toolset_field);
            quote! {
                impl #impl_generics #self_ty #where_clause {
                    #ensure_registered
                }
            }
        })
        .unwrap_or_default();

    Ok(quote! {
        #(#flat_structs)*
        #impl_block

        #names_impl

        #ensure_registered_impl

        #[::tmcp::__private::async_trait::async_trait]
        impl #impl_generics ::tmcp::ServerHandler for #self_ty #where_clause {
            #initialize
            #server_forwarders
            #list_tools
            #call_tool
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expand `#[mcp_server(#attr)]` over `input` and return the pretty-parsed
    /// output.
    fn expand(attr: TokenStream, input: &TokenStream) -> String {
        let expanded = expand_mcp_server(attr, input).unwrap();
        // Parsing the expansion as a file proves the output is structurally valid Rust.
        let file = syn::parse2::<syn::File>(expanded.clone()).expect("expansion parses");
        let _ = file;
        expanded.to_string()
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

        let result_str = expand(TokenStream::new(), &input);

        // The original impl block is preserved and a handler impl is generated.
        assert!(result_str.contains("impl TestServer"));
        assert!(result_str.contains(":: tmcp :: ServerHandler for TestServer"));
        // The server name is the snake_case struct name.
        assert!(result_str.contains("\"test_server\""));
    }

    #[test]
    fn test_snake_case_server_names() {
        let test_cases = vec![
            ("TestServer", "test_server"),
            ("MyMCPServer", "my_mcp_server"),
            ("HTTPServer", "http_server"),
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

            let result_str = expand(TokenStream::new(), &input);
            let expected = format!("\"{expected_snake_case}\"");
            assert!(
                result_str.contains(&expected),
                "Expected server name '{expected_snake_case}' for struct '{struct_name}'"
            );
        }
    }

    #[test]
    fn test_list_tools_memoized_unless_schema_mentions_generics() {
        let plain = quote! {
            impl TestServer {
                #[tool]
                async fn echo(&self, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };
        assert!(expand(TokenStream::new(), &plain).contains("LazyLock"));

        // The params schema references the impl generic, so the tool list
        // cannot be hoisted into a static.
        let generic = quote! {
            impl<T: Send + Sync + 'static> GenericServer<T> {
                #[tool]
                async fn echo(&self, params: Wrapper<T>) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };
        assert!(!expand(TokenStream::new(), &generic).contains("LazyLock"));
    }

    #[test]
    fn test_generic_impl_rejects_generic_flat_params() {
        let input = quote! {
            impl<T: Send + Sync + 'static> GenericServer<T> {
                #[tool]
                async fn echo(&self, a: T, b: i32) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let err = expand_mcp_server(TokenStream::new(), &input).unwrap_err();
        assert!(
            err.to_string()
                .contains("flat tool parameters cannot use the impl block's generic parameters")
        );
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

        let result = expand_mcp_server(TokenStream::new(), &input);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains(
            "No tool methods, delegated tools, tool groups, or resource callbacks found"
        ));
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

        let result = expand_mcp_server(quote! { resources_fn = docs }, &input);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("resources_fn function 'docs' not found")
        );
    }

    #[test]
    fn test_duplicate_forwarder_argument_rejected() {
        let input = quote! {
            impl TestServer {
                async fn docs(
                    &self,
                    context: &ServerCtx,
                    cursor: Option<schema::Cursor>,
                ) -> Result<schema::ListResourcesResult> {
                    unimplemented!()
                }
            }
        };

        let result = expand_mcp_server(quote! { resources_fn = docs, resources_fn = docs }, &input);
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("duplicate argument: resources_fn")
        );
    }

    #[test]
    fn test_duplicate_delegated_tool_name_rejected() {
        let input = quote! {
            impl TestServer {
                async fn state(
                    &self,
                    arguments: Option<&Arguments>,
                ) -> ToolResult<String> {
                    unimplemented!()
                }

                #[tool]
                async fn echo(&self) -> ToolResult<()> {
                    Ok(())
                }
            }
        };

        let result = expand_mcp_server(
            quote! { tools = [concern::echo], tool_state_fn = state },
            &input,
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("duplicate tool name `echo`")
        );
    }

    #[test]
    fn test_forwarder_expansion_generates_trait_methods() {
        let input = quote! {
            /// Test server
            impl TestServer {
                async fn docs(
                    &self,
                    context: &ServerCtx,
                    cursor: Option<schema::Cursor>,
                ) -> Result<schema::ListResourcesResult> {
                    unimplemented!()
                }

                async fn doc(
                    &self,
                    context: &ServerCtx,
                    uri: String,
                ) -> Result<schema::ReadResourceResult> {
                    unimplemented!()
                }

                async fn shutdown(&self) -> Result<()> {
                    Ok(())
                }
            }
        };

        let attrs = quote! {
            resources_fn = docs,
            read_resource_fn = doc,
            shutdown_fn = shutdown
        };
        let result_str = expand(attrs, &input);

        assert!(result_str.contains("async fn list_resources"));
        assert!(result_str.contains("async fn read_resource"));
        assert!(result_str.contains("async fn on_shutdown"));
        assert!(result_str.contains("with_resources"));
        assert!(!result_str.contains("with_tools"));
    }
}
