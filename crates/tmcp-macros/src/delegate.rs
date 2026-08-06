//! `ServerHandler` delegation code generation.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Expr, ImplItem, ItemImpl, Result, spanned::Spanned};

/// Add forwarding methods for each `ServerHandler` method that the impl omits.
pub fn expand(attr: TokenStream, input: TokenStream) -> Result<TokenStream> {
    let delegate = syn::parse2::<Expr>(attr)?;
    let mut impl_block = syn::parse2::<ItemImpl>(input)?;
    let Some((_, trait_path, _)) = &impl_block.trait_ else {
        return Err(Error::new(
            impl_block.impl_token.span(),
            "delegate_server_handler requires a trait impl",
        ));
    };
    if !trait_path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "ServerHandler")
    {
        return Err(Error::new(
            trait_path.span(),
            "delegate_server_handler requires a ServerHandler impl",
        ));
    }

    let present: HashSet<_> = impl_block
        .items
        .iter()
        .filter_map(|item| match item {
            ImplItem::Fn(method) => Some(method.sig.ident.to_string()),
            _ => None,
        })
        .collect();
    for (name, method) in methods(&delegate) {
        if !present.contains(name) {
            impl_block.items.push(ImplItem::Fn(syn::parse2(method)?));
        }
    }
    Ok(quote! { #impl_block })
}

/// Build the complete forwarding method set.
fn methods(delegate: &Expr) -> Vec<(&'static str, TokenStream)> {
    vec![
        (
            "on_connect",
            quote! {
                async fn on_connect(
                    &self,
                    context: &::tmcp::ServerCtx,
                    remote_addr: &str,
                ) -> ::tmcp::Result<()> {
                    (#delegate).on_connect(context, remote_addr).await
                }
            },
        ),
        (
            "on_shutdown",
            quote! {
                async fn on_shutdown(&self) -> ::tmcp::Result<()> {
                    (#delegate).on_shutdown().await
                }
            },
        ),
        (
            "initialize",
            quote! {
                async fn initialize(
                    &self,
                    context: &::tmcp::ServerCtx,
                    protocol_version: ::tmcp::schema::ProtocolVersion,
                    capabilities: ::tmcp::schema::ClientCapabilities,
                    client_info: ::tmcp::schema::Implementation,
                ) -> ::tmcp::Result<::tmcp::schema::InitializeResult> {
                    (#delegate)
                        .initialize(context, protocol_version, capabilities, client_info)
                        .await
                }
            },
        ),
        (
            "pong",
            quote! {
                async fn pong(&self, context: &::tmcp::ServerCtx) -> ::tmcp::Result<()> {
                    (#delegate).pong(context).await
                }
            },
        ),
        (
            "list_tools",
            quote! {
                async fn list_tools(
                    &self,
                    context: &::tmcp::ServerCtx,
                    cursor: Option<::tmcp::schema::Cursor>,
                ) -> ::tmcp::Result<::tmcp::schema::ListToolsResult> {
                    (#delegate).list_tools(context, cursor).await
                }
            },
        ),
        (
            "call_tool",
            quote! {
                async fn call_tool(
                    &self,
                    context: &::tmcp::ServerCtx,
                    name: String,
                    arguments: Option<::tmcp::Arguments>,
                    task: Option<::tmcp::schema::TaskMetadata>,
                ) -> ::tmcp::Result<::tmcp::schema::CallToolResponse> {
                    (#delegate).call_tool(context, name, arguments, task).await
                }
            },
        ),
        (
            "list_resources",
            quote! {
                async fn list_resources(
                    &self,
                    context: &::tmcp::ServerCtx,
                    cursor: Option<::tmcp::schema::Cursor>,
                ) -> ::tmcp::Result<::tmcp::schema::ListResourcesResult> {
                    (#delegate).list_resources(context, cursor).await
                }
            },
        ),
        (
            "list_resource_templates",
            quote! {
                async fn list_resource_templates(
                    &self,
                    context: &::tmcp::ServerCtx,
                    cursor: Option<::tmcp::schema::Cursor>,
                ) -> ::tmcp::Result<::tmcp::schema::ListResourceTemplatesResult> {
                    (#delegate).list_resource_templates(context, cursor).await
                }
            },
        ),
        (
            "read_resource",
            quote! {
                async fn read_resource(
                    &self,
                    context: &::tmcp::ServerCtx,
                    uri: String,
                ) -> ::tmcp::Result<::tmcp::schema::ReadResourceResult> {
                    (#delegate).read_resource(context, uri).await
                }
            },
        ),
        (
            "subscribe_resource",
            quote! {
                async fn subscribe_resource(
                    &self,
                    context: &::tmcp::ServerCtx,
                    uri: String,
                ) -> ::tmcp::Result<()> {
                    (#delegate).subscribe_resource(context, uri).await
                }
            },
        ),
        (
            "unsubscribe_resource",
            quote! {
                async fn unsubscribe_resource(
                    &self,
                    context: &::tmcp::ServerCtx,
                    uri: String,
                ) -> ::tmcp::Result<()> {
                    (#delegate).unsubscribe_resource(context, uri).await
                }
            },
        ),
        (
            "list_prompts",
            quote! {
                async fn list_prompts(
                    &self,
                    context: &::tmcp::ServerCtx,
                    cursor: Option<::tmcp::schema::Cursor>,
                ) -> ::tmcp::Result<::tmcp::schema::ListPromptsResult> {
                    (#delegate).list_prompts(context, cursor).await
                }
            },
        ),
        (
            "get_prompt",
            quote! {
                async fn get_prompt(
                    &self,
                    context: &::tmcp::ServerCtx,
                    name: String,
                    arguments: Option<::std::collections::HashMap<String, String>>,
                ) -> ::tmcp::Result<::tmcp::schema::GetPromptResult> {
                    (#delegate).get_prompt(context, name, arguments).await
                }
            },
        ),
        (
            "complete",
            quote! {
                async fn complete(
                    &self,
                    context: &::tmcp::ServerCtx,
                    reference: ::tmcp::schema::Reference,
                    argument: ::tmcp::schema::ArgumentInfo,
                    context_info: Option<::tmcp::schema::CompleteContext>,
                ) -> ::tmcp::Result<::tmcp::schema::CompleteResult> {
                    (#delegate)
                        .complete(context, reference, argument, context_info)
                        .await
                }
            },
        ),
        (
            "set_level",
            quote! {
                async fn set_level(
                    &self,
                    context: &::tmcp::ServerCtx,
                    level: ::tmcp::schema::LoggingLevel,
                ) -> ::tmcp::Result<()> {
                    (#delegate).set_level(context, level).await
                }
            },
        ),
        (
            "list_roots",
            quote! {
                async fn list_roots(
                    &self,
                    context: &::tmcp::ServerCtx,
                ) -> ::tmcp::Result<::tmcp::schema::ListRootsResult> {
                    (#delegate).list_roots(context).await
                }
            },
        ),
        (
            "create_message",
            quote! {
                async fn create_message(
                    &self,
                    context: &::tmcp::ServerCtx,
                    params: ::tmcp::schema::CreateMessageParams,
                ) -> ::tmcp::Result<::tmcp::schema::CreateMessageResult> {
                    (#delegate).create_message(context, params).await
                }
            },
        ),
        (
            "get_task",
            quote! {
                async fn get_task(
                    &self,
                    context: &::tmcp::ServerCtx,
                    task_id: String,
                ) -> ::tmcp::Result<::tmcp::schema::GetTaskResult> {
                    (#delegate).get_task(context, task_id).await
                }
            },
        ),
        (
            "get_task_payload",
            quote! {
                async fn get_task_payload(
                    &self,
                    context: &::tmcp::ServerCtx,
                    task_id: String,
                ) -> ::tmcp::Result<::tmcp::schema::GetTaskPayloadResult> {
                    (#delegate).get_task_payload(context, task_id).await
                }
            },
        ),
        (
            "list_tasks",
            quote! {
                async fn list_tasks(
                    &self,
                    context: &::tmcp::ServerCtx,
                    cursor: Option<::tmcp::schema::Cursor>,
                ) -> ::tmcp::Result<::tmcp::schema::ListTasksResult> {
                    (#delegate).list_tasks(context, cursor).await
                }
            },
        ),
        (
            "cancel_task",
            quote! {
                async fn cancel_task(
                    &self,
                    context: &::tmcp::ServerCtx,
                    task_id: String,
                ) -> ::tmcp::Result<::tmcp::schema::CancelTaskResult> {
                    (#delegate).cancel_task(context, task_id).await
                }
            },
        ),
        (
            "notification",
            quote! {
                async fn notification(
                    &self,
                    context: &::tmcp::ServerCtx,
                    notification: ::tmcp::schema::ClientNotification,
                ) -> ::tmcp::Result<()> {
                    (#delegate).notification(context, notification).await
                }
            },
        ),
    ]
}
