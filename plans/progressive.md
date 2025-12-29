# Progressive Discovery for tmcp

## Executive Summary

Progressive discovery allows MCP servers to expose tools incrementally, reducing context
overhead for LLM clients. Instead of presenting all tools upfront, servers expose
activation tools that unlock additional capabilities on demand. This is particularly
valuable for complex servers with many specialized tools.

## Motivation

### The Problem

MCP servers with many tools create context overhead for LLM clients:

1. **Context pollution**: All tool descriptions consume tokens, even when most tools are
   irrelevant to the current task
2. **Decision fatigue**: Large tool sets make it harder for models to select appropriate
   tools
3. **Specialization cost**: Servers that do many things must expose everything, even when
   users only need a subset

### The Solution

Progressive discovery provides a mechanism for servers to:

1. Expose minimal "activation" tools by default
2. Reveal additional tools when the model activates specific capabilities
3. Support hierarchical tool organization for complex servers
4. Maintain type safety and ergonomics through derive macros

### Use Cases

**Simple Activation**: A complex code analysis server exposes only an `activate` tool
initially. The activation tool's description explains what the full server can do. When
the model calls `activate`, the complete tool set becomes available.

**Menu-Based Discovery**: A multi-purpose automation server exposes category activators:
`activate_file_ops`, `activate_network`, `activate_database`. The model can activate only
the categories it needs for the current task.

**Dynamic Tool Trees**: An AI agent framework dynamically generates tools based on loaded
plugins, current user permissions, or runtime state.

## Design Principles

1. **Progressive enhancement**: Static tools remain the default. Progressive discovery is
   opt-in and layered on top.

2. **Type-safe by default**: Derive macros provide compile-time guarantees. Dynamic
   registration is available for advanced use cases.

3. **Zero cost when unused**: Servers that don't use progressive discovery have no
   additional overhead.

4. **Composable**: Static and dynamic tools can be mixed. Multiple progressive patterns
   can coexist.

5. **Notification-driven**: Uses MCP's existing `notifications/tools/list_changed`
   mechanism—no protocol extensions required.

## Architecture

### Core Concepts

**Tool Visibility**: Each tool has a visibility state:
- `Always`: Tool is always visible (activation tools, menus)
- `Group(name)`: Tool is visible when the named group is active
- `Conditional(fn)`: Tool visibility determined by a closure (dynamic only)

**Tool Groups**: Named collections of tools that can be activated together. Groups support:
- Binary activation (active/inactive)
- Mutual exclusion (activating one group deactivates others)
- Hierarchical nesting (child groups require parent activation)

**Activation Flow**:
1. Client calls `tools/list`, receives only visible tools
2. Model calls an activation tool (e.g., `activate_file_ops`)
3. Server updates group state, sends `notifications/tools/list_changed`
4. Client calls `tools/list` again, receives updated tool set
5. Model can now use the newly visible tools

### Three-Tier API

The progressive discovery API is structured in three tiers, from simplest to most
flexible:

#### Tier 1: Simple Activation (Derive Macro)

For servers with a single activation gate—tools are either all hidden or all visible.

```rust
use tmcp::{mcp_server, tool, ServerCtx, Result, schema::CallToolResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Default)]
struct CodeAnalyzer {
    activated: AtomicBool,
}

/// Comprehensive code analysis server for examining, refactoring, and documenting code.
/// Call `activate` to access the full suite of analysis tools.
#[mcp_server(activation = "activated")]
impl CodeAnalyzer {
    /// Activate the code analyzer. Enables: analyze, refactor, document, find_issues.
    /// Call this when you need to perform code analysis tasks.
    #[tool(activator)]
    async fn activate(&self, _ctx: &ServerCtx) -> Result<CallToolResult> {
        self.activated.store(true, Ordering::SeqCst);
        Ok(CallToolResult::new().with_text_content(
            "Code analyzer activated. Tools available: analyze, refactor, document, find_issues"
        ))
    }

    /// Analyze code structure and dependencies.
    #[tool]
    async fn analyze(&self, _ctx: &ServerCtx, params: AnalyzeParams) -> Result<CallToolResult> {
        // Implementation...
        Ok(CallToolResult::new())
    }

    /// Suggest refactoring improvements.
    #[tool]
    async fn refactor(&self, _ctx: &ServerCtx, params: RefactorParams) -> Result<CallToolResult> {
        // Implementation...
        Ok(CallToolResult::new())
    }
}
```

**Generated behavior**:
- `list_tools` returns only `activate` when `activated` is false
- `list_tools` returns all tools when `activated` is true
- `call_tool("activate", ...)` sets `activated = true` and sends `ToolListChanged`
- Calling a hidden tool returns `ToolNotFound` error

**Macro attributes**:
- `#[mcp_server(activation = "field_name")]`: Enables simple activation mode, reading
  state from the named `AtomicBool` field
- `#[tool(activator)]`: Marks the tool as always visible and responsible for activation

#### Tier 2: Grouped Tools (Derive Macro)

For servers with multiple independent feature sets that can be activated separately.

```rust
use tmcp::{mcp_server, tool, ServerCtx, Result, schema::CallToolResult, ToolGroups};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
struct AutomationHub {
    groups: ToolGroups,
}

impl Default for AutomationHub {
    fn default() -> Self {
        Self {
            groups: ToolGroups::new()
                .with_group("file_ops", "File operations: read, write, delete, move")
                .with_group("network", "Network operations: fetch, download, upload")
                .with_group("database", "Database operations: query, insert, update")
        }
    }
}

/// Multi-purpose automation hub. Activate specific capabilities as needed.
#[mcp_server(groups = "groups")]
impl AutomationHub {
    // Menu tools - always visible

    /// Activate file operations. Enables: read_file, write_file, delete_file, move_file.
    #[tool(activator = "file_ops")]
    async fn activate_file_ops(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.groups.activate("file_ops", ctx)?;
        Ok(CallToolResult::new().with_text_content("File operations activated"))
    }

    /// Activate network operations. Enables: fetch_url, download, upload.
    #[tool(activator = "network")]
    async fn activate_network(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.groups.activate("network", ctx)?;
        Ok(CallToolResult::new().with_text_content("Network operations activated"))
    }

    /// Deactivate a capability group to reduce available tools.
    #[tool(activator)]  // activator without group = always visible
    async fn deactivate(&self, ctx: &ServerCtx, params: DeactivateParams) -> Result<CallToolResult> {
        self.groups.deactivate(&params.group, ctx)?;
        Ok(CallToolResult::new().with_text_content(format!("{} deactivated", params.group)))
    }

    // File ops group

    /// Read file contents.
    #[tool(group = "file_ops")]
    async fn read_file(&self, _ctx: &ServerCtx, params: ReadFileParams) -> Result<CallToolResult> {
        // Implementation...
        Ok(CallToolResult::new())
    }

    /// Write content to file.
    #[tool(group = "file_ops")]
    async fn write_file(&self, _ctx: &ServerCtx, params: WriteFileParams) -> Result<CallToolResult> {
        // Implementation...
        Ok(CallToolResult::new())
    }

    // Network group

    /// Fetch URL contents.
    #[tool(group = "network")]
    async fn fetch_url(&self, _ctx: &ServerCtx, params: FetchParams) -> Result<CallToolResult> {
        // Implementation...
        Ok(CallToolResult::new())
    }
}
```

**ToolGroups API**:

```rust
/// Manages tool group activation state and notifications.
pub struct ToolGroups {
    groups: HashMap<String, GroupState>,
}

struct GroupState {
    active: AtomicBool,
    description: String,
    parent: Option<String>,
    mutually_exclusive_with: Vec<String>,
}

impl ToolGroups {
    /// Create a new empty ToolGroups instance.
    pub fn new() -> Self;

    /// Add a group with its description.
    pub fn with_group(self, name: impl Into<String>, description: impl Into<String>) -> Self;

    /// Add a group that's a child of another (requires parent to be active).
    pub fn with_child_group(
        self,
        name: impl Into<String>,
        parent: impl Into<String>,
        description: impl Into<String>
    ) -> Self;

    /// Mark groups as mutually exclusive (activating one deactivates others).
    pub fn with_exclusion(self, groups: &[&str]) -> Self;

    /// Activate a group and send notification.
    pub fn activate(&self, name: &str, ctx: &ServerCtx) -> Result<()>;

    /// Deactivate a group and send notification.
    pub fn deactivate(&self, name: &str, ctx: &ServerCtx) -> Result<()>;

    /// Check if a group is currently active.
    pub fn is_active(&self, name: &str) -> bool;

    /// Get descriptions of all groups for help text.
    pub fn describe(&self) -> String;
}
```

**Generated behavior**:
- `list_tools` returns tools based on group activation state
- Activator tools for inactive groups are always visible
- Regular tools in a group are only visible when the group is active
- Tools in child groups require both parent and child groups to be active

#### Tier 3: Dynamic Registration (Trait Implementation)

For fully dynamic tool management—tools created at runtime, conditional visibility, or
complex state-dependent tool sets.

```rust
use tmcp::{
    ServerHandler, ServerCtx, Result,
    schema::{CallToolResult, ListToolsResult, Tool, ToolSchema, Cursor},
    registry::{ToolRegistry, ToolEntry, DynamicTool},
    Arguments,
};
use async_trait::async_trait;
use std::sync::Arc;

struct PluginServer {
    registry: ToolRegistry,
}

impl PluginServer {
    pub fn new() -> Self {
        let mut registry = ToolRegistry::new();

        // Register a dynamic tool that checks conditions at call time
        registry.register(
            "load_plugin",
            Tool::from_schema::<LoadPluginParams>("load_plugin")
                .with_description("Load a plugin by name. This will add the plugin's tools."),
            |ctx, args| Box::pin(async move {
                let params: LoadPluginParams = args.deserialize()?;
                // Load plugin and register its tools...
                Ok(CallToolResult::new().with_text_content("Plugin loaded"))
            }),
        );

        Self { registry }
    }

    pub async fn load_plugin(&self, name: &str, ctx: &ServerCtx) -> Result<()> {
        // Dynamically add tools from the loaded plugin
        let plugin_tools = discover_plugin_tools(name)?;
        for tool_def in plugin_tools {
            self.registry.register(
                &tool_def.name,
                tool_def.tool,
                tool_def.handler,
            );
        }

        // Notify client of tool list change
        ctx.notify(ServerNotification::tool_list_changed())?;
        Ok(())
    }
}

#[async_trait]
impl ServerHandler for PluginServer {
    async fn initialize(
        &self,
        _ctx: &ServerCtx,
        _protocol_version: String,
        _capabilities: ClientCapabilities,
        _client_info: Implementation,
    ) -> Result<InitializeResult> {
        Ok(InitializeResult::new("plugin-server")
            .with_version("1.0.0")
            .with_tools(true))  // Enable list_changed notifications
    }

    async fn list_tools(
        &self,
        _ctx: &ServerCtx,
        cursor: Option<Cursor>,
    ) -> Result<ListToolsResult> {
        self.registry.list_tools(cursor)
    }

    async fn call_tool(
        &self,
        ctx: &ServerCtx,
        name: String,
        arguments: Option<Arguments>,
        _task: Option<TaskMetadata>,
    ) -> Result<CallToolResult> {
        self.registry.call_tool(ctx, &name, arguments).await
    }
}
```

**ToolRegistry API**:

```rust
/// Type alias for tool handler functions.
pub type ToolHandler = Box<
    dyn Fn(&ServerCtx, Arguments) -> BoxFuture<'static, Result<CallToolResult>>
        + Send
        + Sync
>;

/// A registry for managing dynamic tool registration and dispatch.
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, ToolEntry>>,
}

struct ToolEntry {
    tool: Tool,
    handler: Arc<ToolHandler>,
    group: Option<String>,
    visible: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ToolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self;

    /// Register a tool with its handler.
    pub fn register<F, Fut>(&mut self, name: &str, tool: Tool, handler: F)
    where
        F: Fn(&ServerCtx, Arguments) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CallToolResult>> + Send + 'static;

    /// Register a tool that belongs to a group.
    pub fn register_in_group<F, Fut>(
        &mut self,
        name: &str,
        tool: Tool,
        group: &str,
        handler: F
    ) where
        F: Fn(&ServerCtx, Arguments) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CallToolResult>> + Send + 'static;

    /// Register a tool with conditional visibility.
    pub fn register_conditional<F, Fut, V>(
        &mut self,
        name: &str,
        tool: Tool,
        visible: V,
        handler: F,
    ) where
        F: Fn(&ServerCtx, Arguments) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CallToolResult>> + Send + 'static,
        V: Fn() -> bool + Send + Sync + 'static;

    /// Remove a tool from the registry.
    pub fn unregister(&mut self, name: &str) -> Option<Tool>;

    /// List all currently visible tools.
    pub fn list_tools(&self, cursor: Option<Cursor>) -> Result<ListToolsResult>;

    /// Call a tool by name.
    pub async fn call_tool(
        &self,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
    ) -> Result<CallToolResult>;

    /// Check if a tool exists and is visible.
    pub fn has_tool(&self, name: &str) -> bool;
}
```

### Combining Static and Dynamic Tools

The derive macro and dynamic registration can be combined for hybrid use cases:

```rust
#[derive(Debug)]
struct HybridServer {
    registry: ToolRegistry,
    groups: ToolGroups,
}

/// Hybrid server with both static and dynamic tools.
#[mcp_server(groups = "groups", registry = "registry")]
impl HybridServer {
    // Static activator - always visible
    #[tool(activator = "core")]
    async fn activate_core(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.groups.activate("core", ctx)?;
        Ok(CallToolResult::new())
    }

    // Static tool in a group
    #[tool(group = "core")]
    async fn core_function(&self, _ctx: &ServerCtx, params: CoreParams) -> Result<CallToolResult> {
        // ...
        Ok(CallToolResult::new())
    }

    // Dynamic tools are handled by the registry
    // The macro generates code to check registry after static tools
}
```

When both are present, the generated `list_tools`:
1. Collects visible static tools based on group state
2. Appends visible tools from the registry
3. Returns the combined list

The generated `call_tool`:
1. Checks static tools first
2. Falls back to registry if not found in static tools
3. Returns `ToolNotFound` only if neither has the tool

## Implementation Details

### Derive Macro Changes

The `#[mcp_server]` macro gains new attributes:

| Attribute | Type | Description |
|-----------|------|-------------|
| `activation` | String | Field name of `AtomicBool` for simple activation |
| `groups` | String | Field name of `ToolGroups` for grouped activation |
| `registry` | String | Field name of `ToolRegistry` for dynamic tools |

The `#[tool]` attribute gains:

| Attribute | Type | Description |
|-----------|------|-------------|
| `activator` | Optional String | Marks tool as always visible; optional value names the group it activates |
| `group` | String | Names the group this tool belongs to |

### Generated Code Structure

For a grouped server, the macro generates:

```rust
#[async_trait]
impl ServerHandler for AutomationHub {
    async fn initialize(...) -> Result<InitializeResult> {
        Ok(InitializeResult::new("automation_hub")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_tools(true))  // Always enable list_changed for progressive servers
    }

    async fn list_tools(
        &self,
        _ctx: &ServerCtx,
        _cursor: Option<Cursor>,
    ) -> Result<ListToolsResult> {
        let mut tools = Vec::new();

        // Activators are always visible
        tools.push(Tool::new("activate_file_ops", ...)
            .with_description("Activate file operations..."));
        tools.push(Tool::new("activate_network", ...)
            .with_description("Activate network operations..."));
        tools.push(Tool::new("deactivate", ...)
            .with_description("Deactivate a capability group..."));

        // Group tools are conditional
        if self.groups.is_active("file_ops") {
            tools.push(Tool::new("read_file", ...)
                .with_description("Read file contents."));
            tools.push(Tool::new("write_file", ...)
                .with_description("Write content to file."));
        }

        if self.groups.is_active("network") {
            tools.push(Tool::new("fetch_url", ...)
                .with_description("Fetch URL contents."));
        }

        Ok(ListToolsResult { tools, next_cursor: None })
    }

    async fn call_tool(
        &self,
        ctx: &ServerCtx,
        name: String,
        arguments: Option<Arguments>,
        _task: Option<TaskMetadata>,
    ) -> Result<CallToolResult> {
        match name.as_str() {
            // Activators always callable
            "activate_file_ops" => self.activate_file_ops(ctx).await,
            "activate_network" => self.activate_network(ctx).await,
            "deactivate" => {
                let args = arguments.ok_or_else(|| Error::InvalidParams("..."))?;
                let params = args.deserialize()?;
                self.deactivate(ctx, params).await
            }

            // Group tools require activation
            "read_file" if self.groups.is_active("file_ops") => {
                let args = arguments.ok_or_else(|| Error::InvalidParams("..."))?;
                let params = args.deserialize()?;
                self.read_file(ctx, params).await
            }
            "write_file" if self.groups.is_active("file_ops") => {
                // ...
            }
            "fetch_url" if self.groups.is_active("network") => {
                // ...
            }

            _ => Err(Error::ToolNotFound(name))
        }
    }
}
```

### Error Handling

When a client tries to call a tool that exists but isn't currently visible:

1. **Strict mode** (default): Return `ToolNotFound` error—the tool doesn't exist from the
   client's perspective
2. **Helpful mode** (opt-in): Return a specialized error explaining which group needs to
   be activated

```rust
#[mcp_server(groups = "groups", hidden_tool_hint = true)]
impl MyServer {
    // ...
}

// With hidden_tool_hint = true, calling "read_file" when file_ops is inactive returns:
// Error: Tool 'read_file' requires activation. Call 'activate_file_ops' first.
```

### Thread Safety

- `ToolGroups` uses `AtomicBool` for activation state—no locks needed for reading
- `ToolRegistry` uses `RwLock<HashMap<...>>` for thread-safe dynamic registration
- Notifications use the existing `broadcast` channel infrastructure

## Migration Path

### Existing Servers

Existing `#[mcp_server]` implementations continue to work unchanged. Progressive discovery
is entirely opt-in.

### Adding Progressive Discovery

1. **Simple activation**: Add `AtomicBool` field, use `activation` attribute, mark one
   tool as `activator`
2. **Grouped activation**: Add `ToolGroups` field, use `groups` attribute, add `group`
   attributes to tools
3. **Dynamic tools**: Add `ToolRegistry` field, implement registration logic, use
   `registry` attribute

### From Manual to Macro

Servers currently implementing `ServerHandler` manually can migrate to derive macros while
keeping dynamic capabilities via the `registry` attribute.

## Examples

### Example 1: LLM-Friendly Complex Server

```rust
/// Advanced code intelligence server. Call 'activate' to access analysis tools.
///
/// Available capabilities after activation:
/// - analyze: Deep code structure analysis
/// - refactor: Automated refactoring suggestions
/// - document: Generate documentation
/// - security_scan: Find security vulnerabilities
/// - dependency_audit: Analyze dependencies
#[derive(Debug, Default)]
struct CodeIntelligence {
    activated: AtomicBool,
}

#[mcp_server(activation = "activated")]
impl CodeIntelligence {
    /// Activate the code intelligence suite.
    /// After calling this, you'll have access to: analyze, refactor, document,
    /// security_scan, and dependency_audit tools.
    #[tool(activator)]
    async fn activate(&self, _ctx: &ServerCtx) -> Result<CallToolResult> {
        self.activated.store(true, Ordering::SeqCst);
        Ok(CallToolResult::new().with_text_content(
            "Code intelligence activated. Available tools:\n\
             - analyze: Deep code structure analysis\n\
             - refactor: Automated refactoring suggestions\n\
             - document: Generate documentation\n\
             - security_scan: Find security vulnerabilities\n\
             - dependency_audit: Analyze dependencies"
        ))
    }

    // ... tool implementations
}
```

### Example 2: Hierarchical Menu System

```rust
#[derive(Debug)]
struct DevOpsHub {
    groups: ToolGroups,
}

impl Default for DevOpsHub {
    fn default() -> Self {
        Self {
            groups: ToolGroups::new()
                // Top-level categories
                .with_group("infra", "Infrastructure management")
                .with_group("deploy", "Deployment operations")
                .with_group("monitor", "Monitoring and observability")
                // Sub-categories under infra
                .with_child_group("aws", "infra", "AWS-specific operations")
                .with_child_group("gcp", "infra", "GCP-specific operations")
                .with_child_group("k8s", "infra", "Kubernetes operations")
                // Make cloud providers mutually exclusive
                .with_exclusion(&["aws", "gcp"])
        }
    }
}

#[mcp_server(groups = "groups")]
impl DevOpsHub {
    #[tool(activator = "infra")]
    async fn activate_infra(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.groups.activate("infra", ctx)?;
        Ok(CallToolResult::new().with_text_content(
            "Infrastructure tools activated. Sub-categories available:\n\
             - Call activate_aws for AWS operations\n\
             - Call activate_gcp for GCP operations\n\
             - Call activate_k8s for Kubernetes operations"
        ))
    }

    #[tool(activator = "aws")]  // Requires infra to be active
    async fn activate_aws(&self, ctx: &ServerCtx) -> Result<CallToolResult> {
        self.groups.activate("aws", ctx)?;  // Also deactivates "gcp" due to exclusion
        Ok(CallToolResult::new().with_text_content("AWS tools activated"))
    }

    #[tool(group = "aws")]
    async fn ec2_list(&self, _ctx: &ServerCtx) -> Result<CallToolResult> {
        // Only visible when both "infra" AND "aws" are active
        Ok(CallToolResult::new())
    }
}
```

### Example 3: Permission-Based Visibility

```rust
struct SecureServer {
    registry: ToolRegistry,
    user_permissions: RwLock<HashSet<String>>,
}

impl SecureServer {
    fn new() -> Self {
        let mut registry = ToolRegistry::new();
        let server = Self {
            registry,
            user_permissions: RwLock::new(HashSet::new()),
        };

        // Register tools with permission-based visibility
        let perms = server.user_permissions.clone();
        server.registry.register_conditional(
            "admin_reset",
            Tool::from_schema::<EmptyParams>("admin_reset")
                .with_description("Reset system state (admin only)"),
            move || perms.read().unwrap().contains("admin"),
            |ctx, _args| Box::pin(async move {
                Ok(CallToolResult::new().with_text_content("System reset"))
            }),
        );

        server
    }

    async fn grant_permission(&self, perm: &str, ctx: &ServerCtx) -> Result<()> {
        self.user_permissions.write().unwrap().insert(perm.to_string());
        ctx.notify(ServerNotification::tool_list_changed())?;
        Ok(())
    }
}
```

## Testing Considerations

### Unit Testing Progressive Servers

```rust
#[tokio::test]
async fn test_progressive_activation() {
    let server = CodeIntelligence::default();
    let ctx = test_server_ctx();

    // Before activation
    let tools = server.list_tools(&ctx, None).await.unwrap();
    assert_eq!(tools.tools.len(), 1);
    assert_eq!(tools.tools[0].name, "activate");

    // Calling hidden tool fails
    let result = server.call_tool(&ctx, "analyze".into(), None, None).await;
    assert!(matches!(result, Err(Error::ToolNotFound(_))));

    // Activate
    server.call_tool(&ctx, "activate".into(), None, None).await.unwrap();

    // After activation
    let tools = server.list_tools(&ctx, None).await.unwrap();
    assert!(tools.tools.len() > 1);
    assert!(tools.tools.iter().any(|t| t.name == "analyze"));

    // Hidden tool now works
    let result = server.call_tool(&ctx, "analyze".into(), Some(args), None).await;
    assert!(result.is_ok());
}
```

### Integration Testing

Test that `ToolListChanged` notifications are properly sent:

```rust
#[tokio::test]
async fn test_tool_list_changed_notification() {
    let (client, server) = create_test_pair();

    // Set up notification capture
    let notifications = Arc::new(Mutex::new(Vec::new()));
    client.on_notification({
        let notifs = notifications.clone();
        move |n| notifs.lock().unwrap().push(n)
    });

    // Activate
    client.call_tool("activate", json!({})).await.unwrap();

    // Check notification was received
    let notifs = notifications.lock().unwrap();
    assert!(notifs.iter().any(|n| matches!(n, ServerNotification::ToolListChanged { .. })));
}
```

## Future Considerations

### Potential Extensions

1. **Tool tagging**: Allow tools to have multiple tags for cross-cutting organization
2. **Activation persistence**: Option to persist activation state across reconnections
3. **Activation quotas**: Limit how many groups can be active simultaneously
4. **Activation dependencies**: More complex dependency graphs between groups
5. **Tool versioning**: Different tool versions for different activation states

### Not In Scope

1. **Protocol changes**: This design uses existing MCP notifications
2. **Client-side caching**: Clients are responsible for their own tool list management
3. **Automatic deactivation**: No timeout-based deactivation (servers can implement this)

## Appendix: API Surface

### New Types

```rust
// In tmcp::progressive (or tmcp::discovery)

/// Manages tool group activation state.
pub struct ToolGroups { ... }

/// Manages dynamic tool registration.
pub struct ToolRegistry { ... }

/// Entry in the tool registry.
pub struct ToolEntry { ... }

/// Type alias for dynamic tool handlers.
pub type ToolHandler = Box<dyn Fn(&ServerCtx, Arguments) -> BoxFuture<'_, Result<CallToolResult>> + Send + Sync>;
```

### New Macro Attributes

```rust
// #[mcp_server] attributes
#[mcp_server(activation = "field")]           // Simple activation
#[mcp_server(groups = "field")]               // Grouped activation
#[mcp_server(registry = "field")]             // Dynamic registration
#[mcp_server(hidden_tool_hint = true)]        // Helpful errors for hidden tools

// #[tool] attributes
#[tool(activator)]                            // Always visible, activates server
#[tool(activator = "group_name")]             // Always visible, activates named group
#[tool(group = "group_name")]                 // Belongs to named group
```

### Re-exports

```rust
// In tmcp prelude or main module
pub use progressive::{ToolGroups, ToolRegistry};
```
