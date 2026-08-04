//! Helpers for inspecting a server's MCP API.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::{
    Client, ClientHandler, Result, ServerCtx, ServerHandler,
    schema::{
        ClientCapabilities, ClientRequest, Implementation, InitializeResult, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, Prompt, ProtocolVersion,
        Resource, ResourceTemplate, ServerNotification, SupportedProtocolVersions, Tool,
    },
};

/// Notification buffer used by the synthetic inspection context.
///
/// Notifications emitted during inspection are buffered, never drained, so the
/// buffer bounds how often a handler can notify while being inspected.
const INSPECTION_NOTIFICATION_BUFFER: usize = 256;

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
    /// Return a digest for cache invalidation over the advertised surface.
    ///
    /// The digest canonicalizes ordering by tool name, resource URI, resource
    /// template URI template, and prompt name before hashing the full serialized
    /// [`McpApi`], including the initialize response and advertised schemas.
    /// Treat the value as stable for one tmcp release series; downstream caches
    /// should tolerate invalidation when tmcp changes the protocol model.
    pub fn surface_digest(&self) -> String {
        let mut tools: Vec<&Tool> = self.tools.iter().collect();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        let mut resources: Vec<&Resource> = self.resources.iter().collect();
        resources.sort_by(|left, right| left.uri.cmp(&right.uri));
        let mut resource_templates: Vec<&ResourceTemplate> =
            self.resource_templates.iter().collect();
        resource_templates.sort_by(|left, right| left.uri_template.cmp(&right.uri_template));
        let mut prompts: Vec<&Prompt> = self.prompts.iter().collect();
        prompts.sort_by(|left, right| left.name.cmp(&right.name));

        let surface = McpApiSurface {
            initialize: &self.initialize,
            tools,
            resources,
            resource_templates,
            prompts,
        };
        let bytes = serde_json::to_vec(&surface).expect("McpApi can be serialized to JSON");
        hex_encode(&Sha256::digest(bytes))
    }
}

/// Borrowed, canonically ordered view of [`McpApi`] hashed by `surface_digest`.
///
/// Serializes byte-identically to a sorted [`McpApi`], so digests stay stable
/// without cloning the snapshot.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpApiSurface<'a> {
    /// The server's initialize response.
    initialize: &'a InitializeResult,
    /// Tools sorted by name.
    tools: Vec<&'a Tool>,
    /// Resources sorted by URI.
    resources: Vec<&'a Resource>,
    /// Resource templates sorted by URI template.
    resource_templates: Vec<&'a ResourceTemplate>,
    /// Prompts sorted by name.
    prompts: Vec<&'a Prompt>,
}

/// Encode bytes as lowercase hex into one pre-allocated string.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        hex.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    hex
}

/// Coalesced MCP API refresh flags from server list-change notifications.
#[derive(Default)]
pub struct McpApiRefreshState {
    /// Whether the server's tool list changed.
    tools: AtomicBool,
    /// Whether the server's resource or resource-template lists changed.
    resources: AtomicBool,
    /// Whether the server's prompt list changed.
    prompts: AtomicBool,
}

impl McpApiRefreshState {
    /// Create shared refresh state for one connected server.
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Mark the server's tool list as changed.
    pub fn mark_tools(&self) {
        self.tools.store(true, Ordering::Release);
    }

    /// Mark the server's resource and resource-template lists as changed.
    pub fn mark_resources(&self) {
        self.resources.store(true, Ordering::Release);
    }

    /// Mark the server's prompt list as changed.
    pub fn mark_prompts(&self) {
        self.prompts.store(true, Ordering::Release);
    }

    /// Mark refresh state implied by one server notification.
    pub fn observe_server_notification(&self, notification: &ServerNotification) {
        match notification {
            ServerNotification::ToolListChanged { .. } => self.mark_tools(),
            ServerNotification::ResourceListChanged { .. } => self.mark_resources(),
            ServerNotification::PromptListChanged { .. } => self.mark_prompts(),
            _ => {}
        }
    }

    /// Return the current coalesced dirty state.
    pub fn snapshot(&self) -> McpApiRefreshSnapshot {
        McpApiRefreshSnapshot {
            tools: self.tools.load(Ordering::Acquire),
            resources: self.resources.load(Ordering::Acquire),
            prompts: self.prompts.load(Ordering::Acquire),
        }
    }

    /// Take and clear the current coalesced dirty state.
    pub fn take(&self) -> McpApiRefreshSnapshot {
        McpApiRefreshSnapshot {
            tools: self.tools.swap(false, Ordering::AcqRel),
            resources: self.resources.swap(false, Ordering::AcqRel),
            prompts: self.prompts.swap(false, Ordering::AcqRel),
        }
    }
}

/// Snapshot of pending MCP API refresh work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpApiRefreshSnapshot {
    /// Whether the server's tool list changed.
    pub tools: bool,
    /// Whether the server's resource or resource-template lists changed.
    pub resources: bool,
    /// Whether the server's prompt list changed.
    pub prompts: bool,
}

impl McpApiRefreshSnapshot {
    /// Return whether no API section is marked dirty.
    pub fn is_empty(&self) -> bool {
        !(self.tools || self.resources || self.prompts)
    }
}

/// Client identity and protocol settings used when inspecting a server handler.
#[derive(Debug, Clone)]
pub struct McpApiOptions {
    /// MCP protocol version to request during initialization.
    pub protocol_version: ProtocolVersion,
    /// Client capabilities to send during initialization.
    pub client_capabilities: ClientCapabilities,
    /// Client implementation metadata to send during initialization.
    pub client_info: Implementation,
}

impl Default for McpApiOptions {
    fn default() -> Self {
        Self {
            protocol_version: SupportedProtocolVersions::default().preferred().clone(),
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
pub async fn collect_client_tools<C>(client: &Client<C>) -> Result<Vec<Tool>>
where
    C: ClientHandler + Send + 'static,
{
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
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `resources/list`.
pub async fn collect_client_resources<C>(client: &Client<C>) -> Result<Vec<Resource>>
where
    C: ClientHandler + Send + 'static,
{
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
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `resources/templates/list`.
pub async fn collect_client_resource_templates<C>(
    client: &Client<C>,
) -> Result<Vec<ResourceTemplate>>
where
    C: ClientHandler + Send + 'static,
{
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
        cursor = Some(next_cursor);
    }
}

/// Collect every page returned by a remote `prompts/list`.
pub async fn collect_client_prompts<C>(client: &Client<C>) -> Result<Vec<Prompt>>
where
    C: ClientHandler + Send + 'static,
{
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
        cursor = Some(next_cursor);
    }
}
