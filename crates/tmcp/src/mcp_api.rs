//! Helpers for inspecting a server's MCP API.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::{
    Client, ClientHandler, Result, ServerCtx, ServerHandler,
    schema::{
        ClientCapabilities, Implementation, InitializeResult, LATEST_PROTOCOL_VERSION, Prompt,
        Resource, ResourceTemplate, Tool,
    },
};

/// Notification buffer used by the synthetic inspection context.
const INSPECTION_NOTIFICATION_BUFFER: usize = 16;

/// A serializable snapshot of the static MCP API exposed by a server handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpApi {
    /// The server's initialize response for the inspection client.
    pub initialize: InitializeResult,
    /// Tools advertised by `tools/list`.
    pub tools: Vec<Tool>,
    /// Resources advertised by `resources/list`.
    pub resources: Vec<Resource>,
    /// Resource templates advertised by `resources/templates/list`.
    pub resource_templates: Vec<ResourceTemplate>,
    /// Prompts advertised by `prompts/list`.
    pub prompts: Vec<Prompt>,
}

impl McpApi {
    /// Return a stable digest for cache invalidation over the advertised surface.
    pub fn surface_digest(&self) -> String {
        let mut api = self.clone();
        api.tools.sort_by(|left, right| left.name.cmp(&right.name));
        api.resources
            .sort_by(|left, right| left.uri.cmp(&right.uri));
        api.resource_templates
            .sort_by(|left, right| left.uri_template.cmp(&right.uri_template));
        api.prompts
            .sort_by(|left, right| left.name.cmp(&right.name));

        let bytes = serde_json::to_vec(&api).expect("McpApi can be serialized to JSON");
        let digest = Sha256::digest(bytes);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// Client identity and protocol settings used when inspecting a server handler.
#[derive(Debug, Clone)]
pub struct McpApiOptions {
    /// MCP protocol version to request during initialization.
    pub protocol_version: String,
    /// Client capabilities to send during initialization.
    pub client_capabilities: ClientCapabilities,
    /// Client implementation metadata to send during initialization.
    pub client_info: Implementation,
}

impl Default for McpApiOptions {
    fn default() -> Self {
        Self {
            protocol_version: LATEST_PROTOCOL_VERSION.to_owned(),
            client_capabilities: ClientCapabilities::default(),
            client_info: Implementation::new("tmcp-inspector", env!("CARGO_PKG_VERSION")),
        }
    }
}

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
    let ctx = inspection_context();
    let initialize = handler
        .initialize(
            &ctx,
            options.protocol_version,
            options.client_capabilities,
            options.client_info,
        )
        .await?;
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
pub async fn inspect_client<C>(
    client: &mut Client<C>,
    initialize: InitializeResult,
) -> Result<McpApi>
where
    C: ClientHandler + Send + Sync + 'static,
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
fn inspection_context() -> ServerCtx {
    let (notification_tx, _notification_rx) = mpsc::channel(INSPECTION_NOTIFICATION_BUFFER);
    ServerCtx::new(notification_tx, None)
}

/// Collect every page returned by `tools/list`.
async fn collect_tools(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<Tool>> {
    let mut cursor = None;
    let mut tools = Vec::new();
    loop {
        let page = handler.list_tools(ctx, cursor).await?;
        tools.extend(page.tools);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(tools);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by `resources/list`.
async fn collect_resources(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<Resource>> {
    let mut cursor = None;
    let mut resources = Vec::new();
    loop {
        let page = handler.list_resources(ctx, cursor).await?;
        resources.extend(page.resources);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resources);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by `resources/templates/list`.
async fn collect_resource_templates(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<ResourceTemplate>> {
    let mut cursor = None;
    let mut resource_templates = Vec::new();
    loop {
        let page = handler.list_resource_templates(ctx, cursor).await?;
        resource_templates.extend(page.resource_templates);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resource_templates);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by `prompts/list`.
async fn collect_prompts(
    handler: &(impl ServerHandler + ?Sized),
    ctx: &ServerCtx,
) -> Result<Vec<Prompt>> {
    let mut cursor = None;
    let mut prompts = Vec::new();
    loop {
        let page = handler.list_prompts(ctx, cursor).await?;
        prompts.extend(page.prompts);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(prompts);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `tools/list`.
async fn collect_client_tools<C>(client: &mut Client<C>) -> Result<Vec<Tool>>
where
    C: ClientHandler + Send + Sync + 'static,
{
    let mut cursor = None;
    let mut tools = Vec::new();
    loop {
        let page = client.list_tools(cursor).await?;
        tools.extend(page.tools);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(tools);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `resources/list`.
async fn collect_client_resources<C>(client: &mut Client<C>) -> Result<Vec<Resource>>
where
    C: ClientHandler + Send + Sync + 'static,
{
    let mut cursor = None;
    let mut resources = Vec::new();
    loop {
        let page = client.list_resources(cursor).await?;
        resources.extend(page.resources);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resources);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `resources/templates/list`.
async fn collect_client_resource_templates<C>(
    client: &mut Client<C>,
) -> Result<Vec<ResourceTemplate>>
where
    C: ClientHandler + Send + Sync + 'static,
{
    let mut cursor = None;
    let mut resource_templates = Vec::new();
    loop {
        let page = client.list_resource_templates(cursor).await?;
        resource_templates.extend(page.resource_templates);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(resource_templates);
        };
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `prompts/list`.
async fn collect_client_prompts<C>(client: &mut Client<C>) -> Result<Vec<Prompt>>
where
    C: ClientHandler + Send + Sync + 'static,
{
    let mut cursor = None;
    let mut prompts = Vec::new();
    loop {
        let page = client.list_prompts(cursor).await?;
        prompts.extend(page.prompts);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(prompts);
        };
        cursor = Some(next_cursor);
    }
}
