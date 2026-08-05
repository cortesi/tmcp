//! Expansion of ToolSet-backed `#[mcp_server]` impl blocks.

use proc_macro2::TokenStream;
use quote::quote;
use syn::ext::IdentExt;

use crate::{
    codegen::{
        build_tool_expr, delegated_descriptor_path, generate_tool_call_arm,
        group::generate_group_dispatch_chain, tool_schema_expr,
    },
    model::{ServerInfo, ServerMacroArgs},
};

/// Generate tool registration statements for ToolSet-backed servers.
fn generate_toolset_registration(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    toolset_field: &syn::Ident,
) -> TokenStream {
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
                ::tmcp::GroupRegistration::register_with_override(
                    &group,
                    &self.#toolset_field,
                    None,
                    #override_expr,
                )?;
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
                    .register_schema(#name, tool, ::tmcp::Visibility::Always)?;
            }
        }
    });
    let delegated_registrations = args.tools.iter().map(|path| {
        let descriptor = delegated_descriptor_path(path);
        quote! {
            {
                let tool = #descriptor::schema();
                self.#toolset_field.register_schema(
                    #descriptor::NAME,
                    tool,
                    ::tmcp::Visibility::Always,
                )?;
            }
        }
    });

    quote! {
        #(#group_registrations)*
        #(#tool_registrations)*
        #(#delegated_registrations)*
    }
}

/// Generate a registration guard for ToolSet-backed servers.
pub fn generate_toolset_ensure_registered(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    toolset_field: &syn::Ident,
) -> TokenStream {
    let registrations = generate_toolset_registration(info, args, toolset_field);
    quote! {
        #[doc(hidden)]
        fn __ensure_tools_registered(&self) -> ::tmcp::Result<()> {
            self.#toolset_field.ensure_registered(|| {
                #registrations
                Ok(())
            })
        }
    }
}

/// Generate the ServerHandler::list_tools implementation for ToolSet-backed servers.
pub fn generate_toolset_list_tools(toolset_field: &syn::Ident) -> TokenStream {
    quote! {
        async fn list_tools(
            &self,
            _context: &::tmcp::ServerCtx,
            cursor: Option<::tmcp::schema::Cursor>,
        ) -> ::tmcp::Result<::tmcp::schema::ListToolsResult> {
            self.__ensure_tools_registered()?;
            self.#toolset_field.list_tools(cursor)
        }
    }
}

/// Generate the ServerHandler::call_tool implementation for ToolSet-backed servers.
pub fn generate_toolset_call_tool(
    info: &ServerInfo,
    args: &ServerMacroArgs,
    toolset_field: &syn::Ident,
) -> TokenStream {
    let receiver = quote! { handler };
    let tool_matches = info
        .tools
        .iter()
        .map(|tool| generate_tool_call_arm(tool, &receiver, &info.struct_name));
    let group_dispatch = generate_group_dispatch_chain(&info.groups, &receiver);
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
                let state = match handler.#resolver(arguments.as_ref()).await {
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
            self.__ensure_tools_registered()?;
            self.#toolset_field
                .call_tool_with(self, context, &name, arguments, task, |handler, context, name, arguments, task| -> ::tmcp::ToolCallFuture<'_> {
                    Box::pin(async move {
                        match name {
                            #(#tool_matches)*
                            #(#delegated_matches)*
                            _ => {
                                #group_dispatch
                                handler
                                    .#toolset_field
                                    .call_dynamic_tool(context, name, arguments)
                                    .await
                                    .map(Into::into)
                            }
                        }
                    })
                })
                .await
        }
    }
}
