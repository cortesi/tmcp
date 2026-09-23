//! Inspection of MCP APIs from server handlers and connected clients.

use std::collections::HashSet;

use tokio::sync::mpsc;

use crate::{
    Client, ClientHandler, McpApi, McpApiOptions, Result, ServerCtx, ServerHandler,
    schema::{
        ClientRequest, Cursor, InitializeResult, ListPromptsResult, ListResourceTemplatesResult,
        ListResourcesResult, ListToolsResult, Prompt, Resource, ResourceTemplate,
        ServerNotification, Tool,
    },
};

/// Notification buffer used by the synthetic inspection context.
///
/// Notifications emitted during inspection are buffered, never drained, so the
/// buffer bounds how often a handler can notify while being inspected.
const INSPECTION_NOTIFICATION_BUFFER: usize = 256;

/// Inspect a server handler using the default MCP inspection client identity.
///
/// # Errors
///
/// Returns any error raised by the handler's initialize or listing methods.
pub async fn inspect_server(handler: &(impl ServerHandler + ?Sized)) -> Result<McpApi> {
    inspect_server_with(handler, McpApiOptions::default()).await
}

/// Inspect a server handler using explicit MCP inspection settings.
///
/// # Errors
///
/// Returns any error raised by the handler's initialize or listing methods.
pub async fn inspect_server_with(
    handler: &(impl ServerHandler + ?Sized),
    options: McpApiOptions,
) -> Result<McpApi> {
    // The receiver must stay alive for the whole inspection so handlers that
    // notify during initialize or listing do not fail on a closed channel.
    let (ctx, _notification_rx) = inspection_context();
    let protocol_version = options.protocol_version;
    let mut initialize = handler
        .initialize(
            &ctx,
            protocol_version.clone(),
            options.client_capabilities,
            options.client_info,
        )
        .await?;
    initialize.protocol_version = protocol_version;
    let tools = if initialize.capabilities.tools.is_some() {
        collect_tools(handler, &ctx).await?
    } else {
        Vec::new()
    };
    let resources = if initialize.capabilities.resources.is_some() {
        collect_resources(handler, &ctx).await?
    } else {
        Vec::new()
    };
    let resource_templates = if initialize.capabilities.resources.is_some() {
        collect_resource_templates(handler, &ctx).await?
    } else {
        Vec::new()
    };
    let prompts = if initialize.capabilities.prompts.is_some() {
        collect_prompts(handler, &ctx).await?
    } else {
        Vec::new()
    };

    Ok(McpApi {
        initialize,
        tools,
        resources,
        resource_templates,
        prompts,
    })
}

/// Inspect a connected MCP client using an already captured initialize result.
///
/// # Errors
///
/// Returns any error raised by the remote server's listing methods.
pub async fn inspect_client<C>(client: &Client<C>, initialize: InitializeResult) -> Result<McpApi>
where
    C: ClientHandler + Send + 'static,
{
    let tools = if initialize.capabilities.tools.is_some() {
        collect_client_tools(client).await?
    } else {
        Vec::new()
    };
    let resources = if initialize.capabilities.resources.is_some() {
        collect_client_resources(client).await?
    } else {
        Vec::new()
    };
    let resource_templates = if initialize.capabilities.resources.is_some() {
        collect_client_resource_templates(client).await?
    } else {
        Vec::new()
    };
    let prompts = if initialize.capabilities.prompts.is_some() {
        collect_client_prompts(client).await?
    } else {
        Vec::new()
    };

    Ok(McpApi {
        initialize,
        tools,
        resources,
        resource_templates,
        prompts,
    })
}

/// Build a request context suitable for direct handler inspection.
///
/// Returns the context together with its notification receiver; the caller
/// must keep the receiver alive so `ServerCtx::notify` succeeds during
/// inspection.
fn inspection_context() -> (ServerCtx, mpsc::Receiver<ServerNotification>) {
    let (notification_tx, notification_rx) = mpsc::channel(INSPECTION_NOTIFICATION_BUFFER);
    (ServerCtx::new(notification_tx, None), notification_rx)
}

/// Retain one pagination cursor and reject a cycle before the next request.
fn retain_next_cursor(
    seen: &mut HashSet<Cursor>,
    cursor: Cursor,
    operation: &str,
) -> Result<Cursor> {
    if seen.insert(cursor.clone()) {
        Ok(cursor)
    } else {
        Err(crate::Error::Protocol(format!(
            "{operation} returned repeated pagination cursor `{cursor}`"
        )))
    }
}

/// Collect every page returned by `tools/list`.
async fn collect_tools(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<Tool>> {
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut tools = Vec::new();
    loop {
        let page = handler.list_tools(ctx, cursor).await?;
        tools.extend(page.tools);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(tools);
        };
        cursor = Some(retain_next_cursor(&mut seen, next_cursor, "tools/list")?);
    }
}

/// Collect every page returned by `resources/list`.
async fn collect_resources(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<Resource>> {
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut resources = Vec::new();
    loop {
        let page = handler.list_resources(ctx, cursor).await?;
        resources.extend(page.resources);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resources);
        };
        cursor = Some(retain_next_cursor(
            &mut seen,
            next_cursor,
            "resources/list",
        )?);
    }
}

/// Collect every page returned by `resources/templates/list`.
async fn collect_resource_templates(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<ResourceTemplate>> {
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut resource_templates = Vec::new();
    loop {
        let page = handler.list_resource_templates(ctx, cursor).await?;
        resource_templates.extend(page.resource_templates);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resource_templates);
        };
        cursor = Some(retain_next_cursor(
            &mut seen,
            next_cursor,
            "resources/templates/list",
        )?);
    }
}

/// Collect every page returned by `prompts/list`.
async fn collect_prompts(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<Prompt>> {
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut prompts = Vec::new();
    loop {
        let page = handler.list_prompts(ctx, cursor).await?;
        prompts.extend(page.prompts);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(prompts);
        };
        cursor = Some(retain_next_cursor(&mut seen, next_cursor, "prompts/list")?);
    }
}

/// Collect every page returned by a remote `tools/list`.
pub async fn collect_client_tools<C>(client: &Client<C>) -> Result<Vec<Tool>>
where
    C: ClientHandler + Send + 'static,
{
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut tools = Vec::new();
    loop {
        let (_, pending) = client
            .request::<ListToolsResult>(ClientRequest::list_tools(cursor))
            .await?;
        let page = pending.await?;
        tools.extend(page.tools);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(tools);
        };
        cursor = Some(retain_next_cursor(&mut seen, next_cursor, "tools/list")?);
    }
}

/// Collect every page returned by a remote `resources/list`.
pub async fn collect_client_resources<C>(client: &Client<C>) -> Result<Vec<Resource>>
where
    C: ClientHandler + Send + 'static,
{
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut resources = Vec::new();
    loop {
        let (_, pending) = client
            .request::<ListResourcesResult>(ClientRequest::list_resources(cursor))
            .await?;
        let page = pending.await?;
        resources.extend(page.resources);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resources);
        };
        cursor = Some(retain_next_cursor(
            &mut seen,
            next_cursor,
            "resources/list",
        )?);
    }
}

/// Collect every page returned by a remote `resources/templates/list`.
pub async fn collect_client_resource_templates<C>(
    client: &Client<C>,
) -> Result<Vec<ResourceTemplate>>
where
    C: ClientHandler + Send + 'static,
{
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut resource_templates = Vec::new();
    loop {
        let (_, pending) = client
            .request::<ListResourceTemplatesResult>(ClientRequest::list_resource_templates(cursor))
            .await?;
        let page = pending.await?;
        resource_templates.extend(page.resource_templates);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resource_templates);
        };
        cursor = Some(retain_next_cursor(
            &mut seen,
            next_cursor,
            "resources/templates/list",
        )?);
    }
}

/// Collect every page returned by a remote `prompts/list`.
pub async fn collect_client_prompts<C>(client: &Client<C>) -> Result<Vec<Prompt>>
where
    C: ClientHandler + Send + 'static,
{
    let mut seen = HashSet::new();
    let mut cursor = None;
    let mut prompts = Vec::new();
    loop {
        let (_, pending) = client
            .request::<ListPromptsResult>(ClientRequest::list_prompts(cursor))
            .await?;
        let page = pending.await?;
        prompts.extend(page.prompts);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(prompts);
        };
        cursor = Some(retain_next_cursor(&mut seen, next_cursor, "prompts/list")?);
    }
}
