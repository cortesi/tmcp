# Progressive Discovery for tmcp

## Executive Summary

Progressive discovery lets MCP servers expose tools incrementally without changing the MCP
protocol. A single `ToolSet` pipeline owns registration, visibility, listing, and dispatch. The
`ToolSet` includes an implicit root group that is always active; tools registered into root are
always visible, so a static tool list stays effortless. When group activation changes visibility,
the server emits `notifications/tools/list_changed` so clients refresh their tool list. Hidden
tools are indistinguishable from unknown tools (`ToolNotFound`) to avoid leaking capabilities.

## Motivation

### The Problem

MCP servers with many tools create context overhead for LLM clients:

1. **Context pollution**: Tool descriptions consume tokens even when most tools are
   irrelevant.
2. **Decision fatigue**: Large tool sets make it harder for models to select appropriate
   tools.
3. **Specialization cost**: Servers that do many things must expose everything even when
   users only need a subset.

### The Solution

Progressive discovery provides a mechanism for servers to:

1. Expose minimal "activation" tools by default.
2. Reveal additional tools when capabilities are activated.
3. Organize tools into groups without protocol changes.
4. Keep list and call behavior consistent across static and dynamic tools.

### Use Cases

**Default Behavior**: A server registers tools into the root group and behaves like a
normal MCP server with no progressive discovery configuration.

**Simple Activation**: A complex code analysis server exposes a `foo.activate` tool
initially. Calling it unlocks the full suite.

**Menu-Based Discovery**: An automation server exposes category activators like
`file_ops.activate` and `network.activate`.

**Dynamic Tool Trees**: An agent framework registers tools at runtime based on plugins,
permissions, or state.

## Design Principles

1. **Single pipeline**: All tools, static or dynamic, are registered into one `ToolSet`.
2. **Root by default**: Tools register into an always-active root group unless specified.
3. **Opaque discovery**: Hidden tools are indistinguishable from unknown tools.
4. **Type-safe ergonomics**: Derive macros provide compile-time guarantees.
5. **Composable**: Groups, conditional visibility, and dynamic registration can coexist.
6. **Notification-driven**: Uses `notifications/tools/list_changed`; no protocol changes.

## Architecture

### The ToolSet Pipeline

`ToolSet` is the canonical source of truth for tools. It owns:

- Tool registration (static or dynamic)
- Tool visibility rules
- `list_tools` assembly
- `call_tool` dispatch with visibility enforcement

Servers delegate both `list_tools` and `call_tool` to the `ToolSet`, which ensures
consistent behavior regardless of how a tool was introduced.

### Model and Invariants

Progressive discovery separates three concerns:

- **Registration**: A tool exists in the system and has a handler.
- **Visibility**: A tool appears in `list_tools`.
- **Dispatchability**: A tool can be invoked successfully.

Invariants:

- Dispatchability implies visibility; hidden tools must return `ToolNotFound` even when
  registered.
- Root is implicit, always active, cannot be deactivated, and does not appear in
  `list_groups`.
- `list_tools` is a pure function of registered tools + current visibility rules at a
  single point in time.

### Core Concepts

**ToolSet**: Central registry and dispatcher. All tools flow through it.

**Root Group**: An implicit group that is always active. Tools registered without an
explicit group or visibility use root, so they are always visible. This preserves the
default MCP behavior and removes the need to specify groups for common servers.

**Visibility**: Each tool has a visibility rule:

- `Always`: Tool is always visible (activators, menus, introspection tools).
- `Group(name)`: Tool is visible only when the named group is active.
- `When(fn)`: Tool is visible when a predicate returns true. The predicate receives
  a read-only `ToolSetView` for inspecting group state or other runtime conditions.
  Predicates must be side-effect free and fast; they are evaluated during `list_tools`.

Visibility evaluation is snapshot-based: `list_tools` evaluates all predicates against a
consistent read-only view of group state captured at the start of the call.

`Visibility::When` is evaluated per call to `list_tools`; there are no cross-call caching
guarantees.

**Group Structs**: A group is a struct annotated with `#[derive(Group)]`. The group name is
derived from the struct name (snake-cased) unless overridden with `#[group(name = "...")]`.
Tool methods inside the group use `#[tool]`, and subgroup methods use `#[group]` to return
another group instance. `#[group]` methods on the server impl define root groups. Groups can
optionally declare activation hooks and deactivator visibility via `#[group(...)]` metadata.

**ToolGroups**: Optional activation state for named groups. Supports:

- Binary activation (active/inactive)
- Mutual exclusion (activating one deactivates others)
- Hierarchical nesting (child groups require parents)

Group semantics:

- Group names are stable, unique identifiers used in tool visibility rules and client UIs.
- A child group is only active if its parent is active.
- Activating a group in an exclusion set deactivates the other groups in that set (and their
  descendants).
- Activating a child group fails unless the parent is already active; require explicit parent
  activation.

Automatic activators:

- Each group has an always-visible activator tool named `group.activate`.
- Each group may have a deactivator tool named `group.deactivate`.
- Deactivators are shown by default; `#[group(show_deactivator = false)]` hides them.
- If a group defines a tool method named `activate` or `deactivate`, it overrides
  auto-generation for that tool name.

Tool naming policy:

- Tool names are hierarchical identifiers derived from the group path plus a local base name.
- Full tool name = `{group_path}.{base}` when a tool belongs to a group; root tools use `base`.
- Group paths are dot-separated segments (e.g., `database.write`).
- Group path segments default to group struct names (snake-cased) unless overridden.
- Base names default to tool method names (snake-cased).
- `.` is reserved as the namespace separator; group names and base names MUST NOT contain `.`.
- `ToolSet` MAY validate this and return an error on registration if violated.
- Group membership is reflected only in the name and visibility rules; the tool handler and
  schema remain unchanged.
- Base names `activate` and `deactivate` are reserved for auto-generated tools unless explicitly
  overridden by user-defined tools of the same name.

Naming conventions enabled by this policy:

- A group can manage tools without global name coordination: two groups can both have `read`.
- Automatic activators live in the same namespace as the group: `foo.activate` and
  `foo.deactivate`.

Activation hooks:

- Groups can define `on_activate` and `on_deactivate` hooks for setup/teardown.
- Hooks are declared on the group struct via `#[group(on_activate = "...")]` and
  `#[group(on_deactivate = "...")]` pointing at async methods on the group.
- Hooks run for both tool-driven activation and programmatic activation.
- If a hook returns an error, the activation/deactivation is aborted and state is unchanged.
- Hooks run before state changes are applied so teardown runs while the group is still active.

### Activation Flow

1. Client calls `tools/list` and receives visible tools.
2. Model calls an activation tool (e.g., `file_ops.activate`).
3. Server runs activation hooks, activates the group, and if visibility changed emits
   `notifications/tools/list_changed`.
4. Client calls `tools/list` again and receives the updated tool set.
5. Newly visible tools can now be used.

### Capability Signaling

Servers that may change tool visibility at runtime MUST advertise tools capability with
`list_changed = true`. Static servers (root-only tools, no dynamic registration) may advertise
`list_changed = false` or omit it, but should still tolerate clients refreshing.

## API Design

### Public Types

```rust
use std::sync::Arc;

use futures::future::BoxFuture;

use tmcp::{
    Arguments, Result, ServerCtx,
    schema::{CallToolResult, Cursor, ListToolsResult, Tool},
};

/// Read-only view used by visibility predicates.
pub struct ToolSetView<'a> { /* private */ }

impl ToolSetView<'_> {
    /// Check whether a group is active in the current snapshot.
    pub fn is_group_active(&self, name: &str) -> bool;
}

/// Controls when a tool appears in `list_tools`.
pub enum Visibility {
    /// Tool is always visible (default for root group).
    Always,
    /// Tool is visible only when the named group is active.
    Group(String),
    /// Tool is visible when the predicate returns true. The predicate receives
    /// a read-only ToolSetView for state inspection.
    When(Arc<dyn Fn(&ToolSetView) -> bool + Send + Sync>),
}

/// Information about a tool group for introspection.
pub struct GroupInfo {
    /// Group identifier.
    pub name: String,
    /// Human-readable description of the group's purpose.
    pub description: String,
    /// Whether the group is currently active.
    pub active: bool,
    /// Parent group name, if this is a child group.
    pub parent: Option<String>,
    /// Number of tools registered to this group (best-effort snapshot).
    pub tool_count: usize,
}

/// Configuration for group registration, hooks, and deactivator visibility.
pub struct GroupConfig {
    /// Group identifier (path segment for this level).
    pub name: String,
    /// Human-readable description for UI/discovery.
    pub description: String,
    /// Optional parent group name to form a hierarchy.
    pub parent: Option<String>,
    /// Optional setup hook invoked before activation.
    pub on_activate: Option<ActivationHook>,
    /// Optional teardown hook invoked before deactivation.
    pub on_deactivate: Option<ActivationHook>,
    /// Whether to expose the `group.deactivate` tool (default: true).
    pub show_deactivator: bool,
}

/// Hook invoked during activation/deactivation.
pub type ActivationHook = Box<
    dyn Fn(&ServerCtx) -> BoxFuture<'static, Result<()>> + Send + Sync,
>;

/// Trait implemented by group structs (usually via #[derive(Group)]).
pub trait Group {
    /// Group configuration for this node.
    fn config(&self) -> GroupConfig;

    /// Register tools and child groups for this node.
    fn register(&self, toolset: &ToolSet, parent: Option<&str>) -> Result<()>;
}

/// Central registry and dispatcher.
pub struct ToolSet {
    /* private */
}
```

### Internal Types (not exported)

```rust
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, RwLock};

use futures::future::BoxFuture;

/// Handler function for a tool. Receives context and optional arguments.
pub type ToolHandler = Box<
    dyn Fn(&ServerCtx, Option<Arguments>) -> BoxFuture<'static, Result<CallToolResult>>
        + Send
        + Sync,
>;

/// A registered tool with its metadata, visibility rule, and handler.
pub struct ToolEntry {
    pub tool: Tool,
    pub visibility: Visibility,
    pub handler: ToolHandler,
}

/// Manages tool group state including activation, hierarchy, and exclusion.
pub struct ToolGroups {
    groups: RwLock<HashMap<String, GroupState>>,
    exclusions: Vec<Vec<String>>,
}

struct GroupState {
    description: String,
    active: bool,
    parent: Option<String>,
    show_deactivator: bool,
    on_activate: Option<ActivationHook>,
    on_deactivate: Option<ActivationHook>,
}
```

### ToolSet

```rust
pub struct ToolSet {
    /* private */
}

impl ToolSet {
    /// Create an empty tool set with an always-active root group.
    pub fn new() -> Self;

    /// Register a group definition (used by group structs and dynamic registration).
    /// Also registers auto activator/deactivator tools per group configuration.
    pub fn register_group(&self, config: GroupConfig) -> Result<()>;

    /// Register a mutual exclusion set.
    pub fn register_exclusion(&self, groups: &[&str]) -> Result<()>;

    /// Construct a fully-qualified tool name from a group path and base name.
    /// Returns `base` when group is empty.
    pub fn qualified_name(group: &str, base: &str) -> String;

    /// Activate a group and emit `notifications/tools/list_changed` if visibility changed.
    pub fn activate_group(&self, name: &str, ctx: &ServerCtx) -> Result<bool>;

    /// Deactivate a group and emit `notifications/tools/list_changed` if visibility changed.
    pub fn deactivate_group(&self, name: &str, ctx: &ServerCtx) -> Result<bool>;

    /// Check if a group is currently active.
    pub fn is_group_active(&self, name: &str) -> bool;

    /// List all groups with their current state.
    pub fn list_groups(&self) -> Vec<GroupInfo>;

    /// Register a tool into the root group (always visible).
    pub fn register<F, Fut>(&self, name: &str, tool: Tool, handler: F) -> Result<()>
    where
        F: Fn(&ServerCtx, Option<Arguments>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CallToolResult>> + Send + 'static;

    /// Register a tool with explicit visibility.
    pub fn register_with_visibility<F, Fut>(
        &self,
        name: &str,
        tool: Tool,
        visibility: Visibility,
        handler: F,
    ) -> Result<()>
    where
        F: Fn(&ServerCtx, Option<Arguments>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<CallToolResult>> + Send + 'static;

    /// Remove a tool by name. Returns the tool definition if it existed.
    pub fn unregister(&self, name: &str) -> Option<Tool>;

    /// Emit `notifications/tools/list_changed` explicitly.
    pub fn notify_list_changed(&self, ctx: &ServerCtx) -> Result<()>;

    /// List currently visible tools.
    ///
    /// Tools are returned in deterministic name-sorted order (ascending); cursors
    /// are interpreted against that ordering.
    pub fn list_tools(&self, cursor: Option<Cursor>) -> Result<ListToolsResult>;

    /// Call a tool by name. Returns `ToolNotFound` if the tool doesn't exist
    /// or is not currently visible.
    pub async fn call_tool(
        &self,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
    ) -> Result<CallToolResult>;
}
```

### Derive Macro Integration

`#[mcp_server]` without a `toolset` attribute behaves exactly as it does today: static
`list_tools` and `call_tool` generation with no runtime registration. Progressive discovery
is opt-in by adding `toolset = "field_name"`, which switches the macro to ToolSet-backed
registration and visibility.

Root tools are still supported: tool methods defined directly on the server impl register into
the root group and remain always visible. Progressive discovery becomes structural when you
define groups as structs with `#[derive(Group)]` and expose them via `#[group]` methods.

```rust
use tmcp::{mcp_server, tool, Group, ToolSet, ToolResult};

#[derive(Debug, Group)]
struct FileOps {
    // group name defaults to "file_ops"
}

impl FileOps {
    #[tool]
    async fn read_file(&self, params: ReadFileParams) -> ToolResult<FileContent> {
        Ok(FileContent { content: std::fs::read_to_string(&params.path)? })
    }
}

#[derive(Debug, Group)]
struct Network;

impl Network {
    #[tool]
    async fn fetch_url(&self, params: FetchParams) -> ToolResult<FetchResponse> {
        Ok(FetchResponse { body: fetch(&params.url).await? })
    }
}

#[derive(Debug)]
struct AutomationHub {
    tools: ToolSet,
}

#[mcp_server(toolset = "tools")]
impl AutomationHub {
    #[group]
    fn file_ops(&self) -> FileOps {
        FileOps {}
    }

    #[group]
    fn network(&self) -> Network {
        Network
    }

    #[tool(always)]
    async fn list_capabilities(&self) -> ToolResult<CapabilitiesResponse> {
        Ok(CapabilitiesResponse { groups: self.tools.list_groups() })
    }
}
```

Tool names derived from this block:

- `file_ops.activate` (auto)
- `file_ops.deactivate` (auto)
- `file_ops.read_file`
- `network.activate` (auto)
- `network.deactivate` (auto)
- `network.fetch_url`

Deeper hierarchy example:

```rust
use tmcp::{group, tool, Group, ToolResult};

#[derive(Group)]
struct Database;

impl Database {
    #[group]
    fn read(&self) -> DatabaseRead {
        DatabaseRead
    }

    #[group]
    fn write(&self) -> DatabaseWrite {
        DatabaseWrite
    }
}

#[derive(Group)]
struct DatabaseRead;

impl DatabaseRead {
    #[tool]
    async fn query(&self, params: QueryParams) -> ToolResult<QueryResult> {
        Ok(QueryResult::default())
    }
}

#[derive(Group)]
struct DatabaseWrite;

impl DatabaseWrite {
    #[tool]
    async fn insert(&self, params: InsertParams) -> ToolResult<InsertResult> {
        Ok(InsertResult::default())
    }
}
```

Generated tool names:

- `database.read.query`
- `database.write.insert`

Group customization example:

```rust
use tmcp::{group, Group, ServerCtx};

#[derive(Group)]
#[group(
    name = "fs",
    description = "Filesystem operations",
    show_deactivator = false,
    on_activate = "mount",
    on_deactivate = "unmount"
)]
struct FileSystem;

impl FileSystem {
    async fn mount(&self, _ctx: &ServerCtx) -> Result<()> {
        Ok(())
    }

    async fn unmount(&self, _ctx: &ServerCtx) -> Result<()> {
        Ok(())
    }
}
```

**Macro attributes**:

- `#[mcp_server(toolset = "field_name")]`: Delegate `list_tools` and `call_tool` to
  the named `ToolSet` field.
- `#[derive(Group)]`: Declare a group struct. Name defaults to snake-cased struct name.
- `#[group(...)]` on a group struct: Override group metadata (name, description,
  show_deactivator, on_activate, on_deactivate).
- `#[group]` on a method returning a group instance: Register a child group. When used on
  a server impl, it registers a root group.
- `#[group(name = "...")]` on a method: Override the child group name for this edge.
- `#[tool]`: Declare a tool method inside a group (or root tool on the server).
- `#[tool(always)]`: Explicitly mark as always visible. Use for menu tools or
  capability introspection.

### Dynamic Registration

Dynamic tools register into the same `ToolSet` as macro-defined tools. This enables
runtime plugin loading, permission-based tool exposure, and state-dependent tooling.

```rust
use tmcp::{
    Arguments, GroupConfig, Result, ServerCtx, ToolSet, Visibility,
    schema::{CallToolResult, Tool, ToolSchema},
};

struct PluginServer {
    tools: ToolSet,
}

impl PluginServer {
    pub fn new() -> Self {
        let tools = ToolSet::new();
        tools
            .register_group(GroupConfig {
                name: "plugins".into(),
                description: "Dynamically loaded plugin tools".into(),
                parent: None,
                on_activate: None,
                on_deactivate: None,
                show_deactivator: true,
            })
            .expect("group registration failed");
        Self { tools }
    }

    /// Load a plugin and register its tools into the "plugins" group.
    pub async fn load_plugin(&self, name: &str, ctx: &ServerCtx) -> Result<()> {
        let plugin = load_plugin_library(name)?;

        for tool_def in plugin.tools() {
            let name = ToolSet::qualified_name("plugins", &tool_def.name);
            self.tools.register_with_visibility(
                &name,
                tool_def.schema.clone(),
                Visibility::Group("plugins".into()),
                move |ctx, args| {
                    let handler = tool_def.handler.clone();
                    Box::pin(async move { handler.call(ctx, args).await })
                },
            )?;
        }

        // Notify clients that the tool list has changed
        self.tools.notify_list_changed(ctx)?;
        Ok(())
    }

    /// Unload a plugin and remove its tools.
    pub async fn unload_plugin(&self, name: &str, ctx: &ServerCtx) -> Result<()> {
        let plugin = get_loaded_plugin(name)?;

        for tool_name in plugin.tool_names() {
            let name = ToolSet::qualified_name("plugins", tool_name);
            self.tools.unregister(&name);
        }

        self.tools.notify_list_changed(ctx)?;
        Ok(())
    }
}
```

**Note**: When dynamically registering tools, the server is responsible for emitting
`notifications/tools/list_changed`. `activate_group` and `deactivate_group` handle this
automatically; for direct `register` and `unregister` calls, use
`ToolSet::notify_list_changed(ctx)` after changes.

## Implementation Details

### Macro Tool Registration

When `#[mcp_server(toolset = "tools")]` is specified, the macro generates both the
`ServerHandler` trait implementation and a private initialization method that registers
root tools on the server and traverses `#[group]` methods to register group structs and
their tools into the `ToolSet`.

The macro generates registration at first access via `std::sync::Once`:

```rust
impl AutomationHub {
    fn __ensure_tools_registered(&self) {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            // Register group: file_ops
            self.tools.register_group(GroupConfig {
                name: "file_ops".into(),
                description: "File operations: read, write, delete, move".into(),
                parent: None,
                on_activate: None,
                on_deactivate: None,
                show_deactivator: true,
            }).expect("group registration failed");

            // Register file_ops.read_file (group FileOps)
            self.tools.register_schema(
                "file_ops.read_file",
                Tool::new("file_ops.read_file", ToolSchema::from_json_schema::<ReadFileParams>())
                    .with_description("Read a file from the filesystem."),
                Visibility::Group("file_ops".into()),
            ).expect("tool registration failed");

            // ... more tools
        });
    }
}
```

Macro-generated tools use a dispatch table rather than closures, so they do not require the
server to be wrapped in `Arc`. Tool metadata is stored in `ToolSet`; dispatch is generated in
the macro.

`ToolSet` registers auto activators/deactivators when groups are configured. These tools are
handled by `ToolSet::call_dynamic_tool` and do not require macro-generated methods.

Internal helpers used by macro expansion:

```rust
impl ToolSet {
    /// Register tool metadata without a handler (macro-only).
    pub(crate) fn register_schema(
        &self,
        name: &str,
        tool: Tool,
        visibility: Visibility,
    ) -> Result<()>;

    /// Check visibility by name using the current snapshot.
    pub(crate) fn is_tool_visible(&self, name: &str) -> bool;

    /// Dispatch dynamic tools registered with handlers (macro fallback).
    pub(crate) async fn call_dynamic_tool(
        &self,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
    ) -> Result<CallToolResult>;
}
```

The dispatch table approach is the canonical implementation:

```rust
impl ToolSet {
    /// Call a tool, delegating to a handler method on the server.
    pub async fn call_tool_with<H, F, Fut>(
        &self,
        handler: &H,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
        dispatch: F,
    ) -> Result<CallToolResult>
    where
        F: Fn(&H, &ServerCtx, &str, Option<Arguments>) -> Fut,
        Fut: Future<Output = Result<CallToolResult>>,
    {
        // Check visibility first
        if !self.is_tool_visible(name) {
            return Err(Error::ToolNotFound(name.to_string()));
        }
        dispatch(handler, ctx, name, arguments).await
    }
}
```

The macro then generates:

```rust
async fn call_tool(
    &self,
    ctx: &ServerCtx,
    name: String,
    arguments: Option<Arguments>,
    _task: Option<TaskMetadata>,
) -> Result<CallToolResult> {
    self.__ensure_tools_registered();
    self.tools.call_tool_with(self, ctx, &name, arguments, |s, ctx, name, args| {
        Box::pin(async move {
            match name {
                "file_ops.read_file" => { /* dispatch */ }
                _ => s.tools.call_dynamic_tool(ctx, name, args).await
            }
        })
    }).await
}
```

### Generated ServerHandler

The macro generates `ServerHandler` implementations that delegate to `ToolSet`:

```rust
#[async_trait]
impl ServerHandler for AutomationHub {
    async fn initialize(&self, _ctx: &ServerCtx, ...) -> Result<InitializeResult> {
        self.__ensure_tools_registered();
        Ok(InitializeResult::new("automation_hub")
            .with_version(env!("CARGO_PKG_VERSION"))
            .with_tools(true))
    }

    async fn list_tools(
        &self,
        _ctx: &ServerCtx,
        cursor: Option<Cursor>,
    ) -> Result<ListToolsResult> {
        self.__ensure_tools_registered();
        self.tools.list_tools(cursor)
    }

    async fn call_tool(
        &self,
        ctx: &ServerCtx,
        name: String,
        arguments: Option<Arguments>,
        _task: Option<TaskMetadata>,
    ) -> Result<CallToolResult> {
        self.__ensure_tools_registered();
        // Dispatch via generated match (see above)
    }
}
```

### Error Handling

When a client calls a tool that exists but is not currently visible, return
`ToolNotFound` to match MCP semantics. This prevents clients from discovering
which tools exist but are inactive.

Activation and registration errors should be explicit and actionable:

- Unknown group: return a specific error (e.g., `Error::GroupNotFound`) rather than
  silently creating groups.
- Parent inactive: return a clear error indicating the parent must be activated first.
- Exclusion conflict: activation succeeds but deactivates conflicting groups; this should
  be observable via `list_groups`.
- Hook failures: bubble the hook error and leave group state unchanged.

## Migration Path

1. **Static servers**: Add a `ToolSet` field and `#[mcp_server(toolset = "tools")]`.
   Existing tools register into root automatically.
2. **Grouped activation**: Define group structs with `#[derive(Group)]` and expose them
   via `#[group]` methods. Grouped tools are namespaced as `group.<base>`.
3. **Dynamic tools**: Replace ad-hoc dynamic registration with `ToolSet::register(...)` or
   `ToolSet::register_with_visibility(...)`.

Existing servers without progressive discovery remain unchanged.

## Testing Considerations

### Unit Tests

```rust
fn group(name: &str, description: &str) -> GroupConfig {
    GroupConfig {
        name: name.into(),
        description: description.into(),
        parent: None,
        on_activate: None,
        on_deactivate: None,
        show_deactivator: true,
    }
}

#[tokio::test]
async fn test_progressive_activation() {
    let server = AutomationHub::default();
    let ctx = TestServerContext::new();

    // Initially only activators are visible
    let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
    assert!(tools.tools.iter().any(|t| t.name == "file_ops.activate"));
    assert!(!tools.tools.iter().any(|t| t.name == "file_ops.read_file"));

    // Calling hidden tool returns ToolNotFound
    let result = server
        .call_tool(ctx.ctx(), "file_ops.read_file".into(), None, None)
        .await;
    assert!(matches!(result, Err(Error::ToolNotFound(_))));

    // Activate the group
    server.call_tool(ctx.ctx(), "file_ops.activate".into(), None, None)
        .await
        .unwrap();

    // Now the tool is visible
    let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
    assert!(tools.tools.iter().any(|t| t.name == "file_ops.read_file"));
}

#[tokio::test]
async fn test_mutual_exclusion() {
    let tools = ToolSet::new();
    tools.register_group(group("mode_a", "Mode A")).unwrap();
    tools.register_group(group("mode_b", "Mode B")).unwrap();
    tools.register_exclusion(&["mode_a", "mode_b"]).unwrap();

    let ctx = TestServerContext::new();

    tools.activate_group("mode_a", ctx.ctx()).unwrap();
    assert!(tools.is_group_active("mode_a"));
    assert!(!tools.is_group_active("mode_b"));

    // Activating mode_b should deactivate mode_a
    tools.activate_group("mode_b", ctx.ctx()).unwrap();
    assert!(!tools.is_group_active("mode_a"));
    assert!(tools.is_group_active("mode_b"));
}

#[tokio::test]
async fn test_hierarchical_groups() {
    let tools = ToolSet::new();
    tools.register_group(group("database", "Database operations")).unwrap();
    tools
        .register_group(GroupConfig {
            name: "database.write".into(),
            description: "Write operations".into(),
            parent: Some("database".into()),
            on_activate: None,
            on_deactivate: None,
            show_deactivator: true,
        })
        .unwrap();

    let ctx = TestServerContext::new();

    // Cannot activate child without parent
    let result = tools.activate_group("database.write", ctx.ctx());
    assert!(result.is_err());

    // Activate parent first
    tools.activate_group("database", ctx.ctx()).unwrap();
    tools.activate_group("database.write", ctx.ctx()).unwrap();

    assert!(tools.is_group_active("database"));
    assert!(tools.is_group_active("database.write"));

    // Deactivating parent deactivates children
    tools.deactivate_group("database", ctx.ctx()).unwrap();
    assert!(!tools.is_group_active("database"));
    assert!(!tools.is_group_active("database.write"));
}

#[tokio::test]
async fn test_when_visibility() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let flag = Arc::new(AtomicBool::new(false));
    let flag_clone = flag.clone();

    let tools = ToolSet::new();
    tools.register_with_visibility(
        "conditional_tool",
        Tool::new("conditional_tool", ToolSchema::empty()),
        Visibility::When(Arc::new(move |_view| flag_clone.load(Ordering::Relaxed))),
        |_, _| Box::pin(async { Ok(CallToolResult::new()) }),
    ).unwrap();

    // Tool not visible when flag is false
    let visible = tools.list_tools(None).unwrap();
    assert!(!visible.tools.iter().any(|t| t.name == "conditional_tool"));

    // Set flag to true
    flag.store(true, Ordering::Relaxed);

    // Now tool is visible
    let visible = tools.list_tools(None).unwrap();
    assert!(visible.tools.iter().any(|t| t.name == "conditional_tool"));
}

#[tokio::test]
async fn test_deactivation() {
    let server = AutomationHub::default();
    let ctx = TestServerContext::new();

    // Activate then deactivate
    server.call_tool(ctx.ctx(), "file_ops.activate".into(), None, None)
        .await.unwrap();
    assert!(server.tools.is_group_active("file_ops"));

    server.call_tool(ctx.ctx(), "file_ops.deactivate".into(), None, None)
        .await.unwrap();
    assert!(!server.tools.is_group_active("file_ops"));

    // Tool should be hidden again
    let tools = server.list_tools(ctx.ctx(), None).await.unwrap();
    assert!(!tools.tools.iter().any(|t| t.name == "file_ops.read_file"));
}

#[tokio::test]
async fn test_activation_hooks() {
    let tools = ToolSet::new();
    let ctx = TestServerContext::new();

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let activated = Arc::new(AtomicBool::new(false));
    let flag = activated.clone();

    tools
        .register_group(GroupConfig {
            name: "file_ops".into(),
            description: "File operations".into(),
            parent: None,
            on_activate: Some(Box::new(move |_ctx| {
                let flag = flag.clone();
                Box::pin(async move {
                    flag.store(true, Ordering::Relaxed);
                    Ok(())
                })
            })),
            on_deactivate: None,
            show_deactivator: true,
        })
        .unwrap();

    tools.activate_group("file_ops", ctx.ctx()).unwrap();
    assert!(activated.load(Ordering::Relaxed));
}

#[tokio::test]
async fn test_hidden_deactivator() {
    let tools = ToolSet::new();
    let ctx = TestServerContext::new();

    tools
        .register_group(GroupConfig {
            name: "file_ops".into(),
            description: "File operations".into(),
            parent: None,
            on_activate: None,
            on_deactivate: None,
            show_deactivator: false,
        })
        .unwrap();

    let list = tools.list_tools(None).unwrap();
    assert!(list.tools.iter().any(|t| t.name == "file_ops.activate"));
    assert!(!list.tools.iter().any(|t| t.name == "file_ops.deactivate"));

    tools.activate_group("file_ops", ctx.ctx()).unwrap();
    tools.deactivate_group("file_ops", ctx.ctx()).unwrap();
}

#[tokio::test]
async fn test_list_groups() {
    let tools = ToolSet::new();
    tools.register_group(group("file_ops", "File operations")).unwrap();
    tools.register_group(group("network", "Network operations")).unwrap();

    let ctx = TestServerContext::new();
    tools.activate_group("file_ops", ctx.ctx()).unwrap();

    let groups = tools.list_groups();
    assert_eq!(groups.len(), 2);

    let file_ops = groups.iter().find(|g| g.name == "file_ops").unwrap();
    assert!(file_ops.active);
    assert_eq!(file_ops.description, "File operations");

    let network = groups.iter().find(|g| g.name == "network").unwrap();
    assert!(!network.active);
}

#[tokio::test]
async fn test_root_not_listed() {
    let tools = ToolSet::new();
    tools.register_group(group("file_ops", "File operations")).unwrap();
    let groups = tools.list_groups();
    assert!(groups.iter().all(|g| g.name != "root"));
}

#[tokio::test]
async fn test_list_tools_ordering() {
    let tools = ToolSet::new();
    let handler = |_, _| Box::pin(async { Ok(CallToolResult::new()) });
    tools.register("b_tool", Tool::new("b_tool", ToolSchema::empty()), handler)?;
    tools.register("a_tool", Tool::new("a_tool", ToolSchema::empty()), handler)?;
    let list = tools.list_tools(None).unwrap();
    assert_eq!(list.tools[0].name, "a_tool");
    assert_eq!(list.tools[1].name, "b_tool");
}
```

### Integration Tests

```rust
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn test_tool_list_changed_notification() {
    let (client, server) = create_test_pair::<AutomationHub>();

    let notifications = Arc::new(Mutex::new(Vec::new()));
    client.on_notification({
        let notifs = notifications.clone();
        move |n| notifs.lock().unwrap().push(n)
    });

    client.call_tool("file_ops.activate", json!({})).await.unwrap();

    let notifs = notifications.lock().unwrap();
    assert!(notifs.iter().any(|n| matches!(n, ServerNotification::ToolListChanged { .. })));
}

#[tokio::test]
async fn test_concurrent_activation() {
    let server = Arc::new(AutomationHub::default());
    let ctx = TestServerContext::new();

    // Spawn multiple concurrent activations
    let handles: Vec<_> = (0..10).map(|i| {
        let server = server.clone();
        let ctx = ctx.ctx().clone();
        tokio::spawn(async move {
            let group = if i % 2 == 0 { "file_ops" } else { "network" };
            server.tools.activate_group(group, &ctx)
        })
    }).collect();

    // All should complete without panic
    for handle in handles {
        let _ = handle.await.unwrap();
    }

    // State should be consistent (at least one group active)
    let groups = server.tools.list_groups();
    assert!(groups.iter().any(|g| g.active));
}
```

## Future Considerations

1. **Tool tagging**: Multi-tag organization across groups.
2. **Activation persistence**: Persist group state across reconnects.
3. **Activation quotas**: Limit how many groups can be active simultaneously.
4. **Tool versioning**: Different tool versions based on activation state.

## Appendix: API Surface

```rust
// In tmcp::toolset

pub struct ToolSet { ... }
pub struct GroupInfo { ... }
pub struct ToolSetView<'a> { ... }
pub struct GroupConfig { ... }
pub type ActivationHook = Box<
    dyn Fn(&ServerCtx) -> BoxFuture<'static, Result<()>> + Send + Sync,
>;
pub trait Group { ... }

pub enum Visibility {
    Always,
    Group(String),
    When(Arc<dyn Fn(&ToolSetView) -> bool + Send + Sync>),
}
```

### Re-exports

```rust
// In tmcp lib.rs
pub use toolset::{
    ActivationHook, Group, GroupConfig, GroupInfo, ToolSet, ToolSetView, Visibility,
};
```

### New Macro Attributes

| Attribute | Visibility | Behavior |
|-----------|------------|----------|
| `#[derive(Group)]` | n/a | Declares a group struct (name defaults to struct name) |
| `#[group(...)]` on struct | n/a | Group metadata: name, description, hooks, visibility |
| `#[group]` on method | n/a | Registers a child group (root if on server) |
| `#[group(name = "...")]` on method | n/a | Overrides child group name for this edge |
| `#[tool]` | Root/Group | Declares a tool method in the current group |
| `#[tool(always)]` | Always | Always visible tool in the current group |
