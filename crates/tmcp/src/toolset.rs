use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, OnceLock, RwLock},
};

use futures::future::BoxFuture;
use tokio::sync::Mutex;

use crate::{
    Arguments, Error, Result, ServerCtx,
    schema::{CallToolResult, Cursor, ListToolsResult, ServerNotification, Tool, ToolSchema},
};

/// Read-only view used by visibility predicates.
pub struct ToolSetView<'a> {
    /// Snapshot of group state keyed by group name.
    groups: &'a HashMap<String, GroupSnapshot>,
}

/// Shared boxed future type used by tool dispatch.
#[doc(hidden)]
pub type ToolFuture<'a, T> = BoxFuture<'a, T>;

impl ToolSetView<'_> {
    /// Check whether a group is active in the current snapshot.
    pub fn is_group_active(&self, name: &str) -> bool {
        is_group_active_snapshot(self.groups, name)
    }
}

/// Controls when a tool appears in `list_tools`.
#[derive(Clone)]
pub enum Visibility {
    /// Tool is always visible.
    Always,
    /// Tool is visible only when the named group is active.
    Group(String),
    /// Tool is visible when the predicate returns true.
    When(Arc<dyn Fn(&ToolSetView) -> bool + Send + Sync>),
}

impl Visibility {
    /// Evaluate visibility for the current snapshot.
    fn is_visible(&self, view: &ToolSetView) -> bool {
        match self {
            Self::Always => true,
            Self::Group(name) => view.is_group_active(name),
            Self::When(predicate) => predicate(view),
        }
    }
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
    /// Number of tools registered to this group.
    pub tool_count: usize,
}

/// Configuration for group registration, hooks, and deactivator visibility.
pub struct GroupConfig {
    /// Group identifier.
    ///
    /// When a parent is provided, the name must be fully qualified under that parent.
    pub name: String,
    /// Human-readable description for UI/discovery.
    pub description: String,
    /// Optional parent group name to form a hierarchy.
    pub parent: Option<String>,
    /// Optional setup hook invoked before activation.
    pub on_activate: Option<ActivationHook>,
    /// Optional teardown hook invoked before deactivation.
    pub on_deactivate: Option<ActivationHook>,
    /// Whether to expose the `group.deactivate` tool.
    pub show_deactivator: bool,
}

/// Hook invoked during activation/deactivation.
pub type ActivationHook = Box<dyn Fn(&ServerCtx) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Trait implemented by group structs (usually via `#[derive(Group)]`).
pub trait Group: GroupDispatch {
    /// Group configuration for this node.
    ///
    /// For derive-based groups, this should return the local path segment; registration
    /// will qualify it with parent groups.
    fn config(&self) -> GroupConfig;

    /// Register tools and child groups for this node.
    fn register(&self, toolset: &ToolSet, parent: Option<&str>) -> Result<()>
    where
        Self: Sized,
    {
        GroupRegistration::register_with_override(self, toolset, parent, None)
    }
}

/// Internal dispatch and tool registration hooks for groups.
#[doc(hidden)]
pub trait GroupDispatch {
    /// Register tools and child groups for this node.
    fn register_tools(&self, toolset: &ToolSet, group_name: &str) -> Result<()>;

    /// Dispatch a tool call relative to this group.
    fn call_tool<'a>(
        &'a self,
        ctx: &'a ServerCtx,
        name: &'a str,
        arguments: Option<Arguments>,
    ) -> ToolFuture<'a, Result<CallToolResult>>;
}

/// Internal helper for overriding group path segments during registration.
#[doc(hidden)]
pub trait GroupRegistration: Group + GroupDispatch {
    /// Register a group, overriding the path segment if provided.
    fn register_with_override(
        &self,
        toolset: &ToolSet,
        parent: Option<&str>,
        segment_override: Option<&str>,
    ) -> Result<()>;
}

impl<T> GroupRegistration for T
where
    T: Group + GroupDispatch,
{
    fn register_with_override(
        &self,
        toolset: &ToolSet,
        parent: Option<&str>,
        segment_override: Option<&str>,
    ) -> Result<()> {
        let mut config = self.config();
        let segment = segment_override.unwrap_or(config.name.as_str());
        validate_group_segment(segment)?;
        let group_name = if let Some(parent) = parent {
            format!("{parent}.{segment}")
        } else {
            segment.to_string()
        };
        config.parent = parent.map(|parent| parent.to_string());
        config.name = group_name.clone();
        toolset.register_group(config)?;
        self.register_tools(toolset, &group_name)?;
        Ok(())
    }
}

/// Central registry and dispatcher.
#[derive(Clone)]
pub struct ToolSet {
    /// Registered tools and handlers.
    tools: Arc<RwLock<HashMap<String, ToolEntry>>>,
    /// Tool group state and configuration.
    groups: ToolGroups,
    /// Ensures macro-generated registration runs once per toolset instance.
    registration: Arc<OnceLock<()>>,
    /// Serialize activation and deactivation to preserve group invariants.
    activation_lock: Arc<Mutex<()>>,
}

impl Default for ToolSet {
    fn default() -> Self {
        Self {
            tools: Arc::new(RwLock::new(HashMap::new())),
            groups: ToolGroups::default(),
            registration: Arc::new(OnceLock::new()),
            activation_lock: Arc::new(Mutex::new(())),
        }
    }
}

impl ToolSet {
    /// Create an empty tool set with an always-active root group.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a group definition.
    ///
    /// This registers the group configuration and auto activation tools.
    pub fn register_group(&self, config: GroupConfig) -> Result<()> {
        validate_group_path(&config.name)?;
        let auto_config = AutoGroupConfig::from(&config);
        self.groups.register_group(config)?;
        if let Err(error) = self.register_auto_tools(&auto_config) {
            self.groups.unregister_group(&auto_config.name);
            return Err(error);
        }
        Ok(())
    }

    /// Register a mutual exclusion set.
    pub fn register_exclusion(&self, groups: &[&str]) -> Result<()> {
        self.groups.register_exclusion(groups)
    }

    /// Construct a fully-qualified tool name from a group path and base name.
    pub fn qualified_name(group: &str, base: &str) -> String {
        if group.is_empty() {
            base.to_string()
        } else {
            format!("{group}.{base}")
        }
    }

    /// Activate a group and emit `notifications/tools/list_changed` if visibility changed.
    pub async fn activate_group(&self, name: &str, ctx: &ServerCtx) -> Result<bool> {
        let _guard = self.activation_lock.lock().await;
        let plan = self.groups.plan_activation(name)?;
        run_activation_hooks(&plan.hooks, ctx).await?;
        let changed = self.groups.apply_activation(plan)?;
        if changed {
            self.notify_list_changed(ctx)?;
        }
        Ok(changed)
    }

    /// Deactivate a group and emit `notifications/tools/list_changed` if visibility changed.
    pub async fn deactivate_group(&self, name: &str, ctx: &ServerCtx) -> Result<bool> {
        let _guard = self.activation_lock.lock().await;
        let plan = self.groups.plan_deactivation(name)?;
        run_activation_hooks(&plan.hooks, ctx).await?;
        let changed = self.groups.apply_deactivation(plan)?;
        if changed {
            self.notify_list_changed(ctx)?;
        }
        Ok(changed)
    }

    /// Check if a group is currently active.
    pub fn is_group_active(&self, name: &str) -> bool {
        self.groups.is_active(name)
    }

    /// List all groups with their current state.
    pub fn list_groups(&self) -> Vec<GroupInfo> {
        let tool_counts = self.group_tool_counts();
        let mut groups = self.groups.list_groups(&tool_counts);
        groups.sort_by(|a, b| a.name.cmp(&b.name));
        groups
    }

    /// Register a tool into the root group (always visible).
    pub fn register<F>(&self, name: &str, tool: Tool, handler: F) -> Result<()>
    where
        F: for<'a> Fn(&'a ServerCtx, Option<Arguments>) -> ToolFuture<'a, Result<CallToolResult>>
            + Send
            + Sync
            + 'static,
    {
        self.register_with_visibility(name, tool, Visibility::Always, handler)
    }

    /// Register a tool with explicit visibility.
    pub fn register_with_visibility<F>(
        &self,
        name: &str,
        tool: Tool,
        visibility: Visibility,
        handler: F,
    ) -> Result<()>
    where
        F: for<'a> Fn(&'a ServerCtx, Option<Arguments>) -> ToolFuture<'a, Result<CallToolResult>>
            + Send
            + Sync
            + 'static,
    {
        let handler: ToolHandler = Arc::new(handler);
        self.register_entry(name, tool, visibility, Some(handler), ToolOrigin::Explicit)
    }

    /// Remove a tool by name. Returns the tool definition if it existed.
    pub fn unregister(&self, name: &str) -> Option<Tool> {
        self.tools
            .write()
            .unwrap_or_else(|err| err.into_inner())
            .remove(name)
            .map(|entry| entry.tool)
    }

    /// Emit `notifications/tools/list_changed` explicitly.
    pub fn notify_list_changed(&self, ctx: &ServerCtx) -> Result<()> {
        ctx.notify(ServerNotification::tool_list_changed())
    }

    /// List currently visible tools.
    ///
    /// Tools are returned in deterministic name-sorted order (ascending); cursors
    /// are interpreted as offsets into that ordering.
    pub fn list_tools(&self, cursor: Option<Cursor>) -> Result<ListToolsResult> {
        let snapshot = self.groups.snapshot();
        let view = ToolSetView { groups: &snapshot };
        let entries = self
            .tools
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut tools = entries
            .into_iter()
            .filter(|entry| entry.visibility.is_visible(&view))
            .map(|entry| entry.tool)
            .collect::<Vec<_>>();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let offset = cursor.map(parse_cursor_offset).transpose()?.unwrap_or(0);
        if offset > tools.len() {
            return Err(Error::InvalidParams("cursor out of range".to_string()));
        }
        let tools = tools.into_iter().skip(offset).collect();
        Ok(ListToolsResult {
            tools,
            next_cursor: None,
        })
    }

    /// Call a tool by name.
    ///
    /// Returns `ToolNotFound` if the tool doesn't exist or is not currently visible.
    pub async fn call_tool(
        &self,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
    ) -> Result<CallToolResult> {
        if !self.is_tool_visible(name) {
            return Err(Error::ToolNotFound(name.to_string()));
        }
        let handler = self.tool_handler(name);
        let handler = handler.ok_or_else(|| Error::ToolNotFound(name.to_string()))?;
        handler(ctx, arguments).await
    }

    /// Call a tool, delegating to a handler method on the server.
    pub async fn call_tool_with<H, F>(
        &self,
        handler: &H,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
        dispatch: F,
    ) -> Result<CallToolResult>
    where
        F: for<'a> Fn(
            &'a H,
            &'a ServerCtx,
            &'a str,
            Option<Arguments>,
        ) -> ToolFuture<'a, Result<CallToolResult>>,
    {
        if !self.is_tool_visible(name) {
            return Err(Error::ToolNotFound(name.to_string()));
        }
        if let Some(tool_handler) = self.tool_handler(name) {
            return tool_handler(ctx, arguments).await;
        }
        dispatch(handler, ctx, name, arguments).await
    }

    /// Register tool metadata without a handler (macro-only).
    #[doc(hidden)]
    pub fn register_schema(&self, name: &str, tool: Tool, visibility: Visibility) -> Result<()> {
        self.register_entry(name, tool, visibility, None, ToolOrigin::Explicit)
    }

    /// Ensure macro-generated registration runs once per ToolSet instance.
    #[doc(hidden)]
    pub fn ensure_registered<F>(&self, register: F)
    where
        F: FnOnce(),
    {
        let _ = self.registration.get_or_init(|| {
            register();
        });
    }

    /// Check visibility by name using the current snapshot.
    #[doc(hidden)]
    pub fn is_tool_visible(&self, name: &str) -> bool {
        let Some(entry) = self.tool_entry(name) else {
            return false;
        };
        let snapshot = self.groups.snapshot();
        let view = ToolSetView { groups: &snapshot };
        entry.visibility.is_visible(&view)
    }

    /// Dispatch dynamic tools registered with handlers (macro fallback).
    #[doc(hidden)]
    pub async fn call_dynamic_tool(
        &self,
        ctx: &ServerCtx,
        name: &str,
        arguments: Option<Arguments>,
    ) -> Result<CallToolResult> {
        self.call_tool(ctx, name, arguments).await
    }

    /// Return the group name for a registered tool, if any.
    fn tool_group_name(tool_name: &str) -> Option<String> {
        tool_name
            .rsplit_once('.')
            .map(|x| x.0)
            .map(|group| group.to_string())
    }

    /// Count tools per group name.
    fn group_tool_counts(&self) -> HashMap<String, usize> {
        let tools = self.tools.read().unwrap_or_else(|err| err.into_inner());
        let mut counts = HashMap::new();
        for name in tools.keys() {
            if let Some(group) = Self::tool_group_name(name) {
                *counts.entry(group).or_insert(0) += 1;
            }
        }
        counts
    }

    /// Insert a tool entry into the registry.
    fn register_entry(
        &self,
        name: &str,
        mut tool: Tool,
        visibility: Visibility,
        handler: Option<ToolHandler>,
        origin: ToolOrigin,
    ) -> Result<()> {
        self.validate_tool_registration(name, &visibility)?;
        tool.name = name.to_string();
        let mut tools = self.tools.write().unwrap_or_else(|err| err.into_inner());
        if let Some(existing) = tools.get(name)
            && (existing.origin != ToolOrigin::AutoGroup || origin != ToolOrigin::Explicit)
        {
            return Err(Error::InvalidConfiguration(format!(
                "tool already registered: {name}"
            )));
        }
        tools.insert(
            name.to_string(),
            ToolEntry {
                tool,
                visibility,
                handler,
                origin,
            },
        );
        Ok(())
    }

    /// Validate tool names against visibility and group registration.
    fn validate_tool_registration(&self, name: &str, visibility: &Visibility) -> Result<()> {
        let (group_path, _base) = split_tool_name(name)?;
        match visibility {
            Visibility::Group(group_name) => {
                if group_path != Some(group_name.as_str()) {
                    return Err(Error::InvalidConfiguration(format!(
                        "tool '{name}' must be prefixed with group '{group_name}'"
                    )));
                }
                self.groups.ensure_group_exists(group_name)?;
            }
            _ => {
                if let Some(group) = group_path {
                    self.groups.ensure_group_exists(group)?;
                }
            }
        }
        Ok(())
    }

    /// Retrieve a tool entry clone by name.
    fn tool_entry(&self, name: &str) -> Option<ToolEntry> {
        self.tools
            .read()
            .unwrap_or_else(|err| err.into_inner())
            .get(name)
            .cloned()
    }

    /// Retrieve a cloned handler for a tool, if present.
    fn tool_handler(&self, name: &str) -> Option<ToolHandler> {
        self.tool_entry(name).and_then(|entry| entry.handler)
    }

    /// Register auto-generated activation and deactivation tools.
    fn register_auto_tools(&self, config: &AutoGroupConfig) -> Result<()> {
        let group = config.name.clone();
        let activate_name = Self::qualified_name(&group, "activate");
        let activate_tool = activation_tool(&activate_name, &config.description, true);
        let toolset = self.clone();
        let handler_group = group.clone();
        let activate_handler: ToolHandler =
            Arc::new(move |ctx: &ServerCtx, _args: Option<Arguments>| {
                let toolset = toolset.clone();
                let handler_group = handler_group.clone();
                let ctx = ctx.clone();
                Box::pin(async move {
                    let _ = toolset.activate_group(&handler_group, &ctx).await?;
                    Ok(CallToolResult::new())
                })
            });
        let mut entries = vec![(activate_name, activate_tool, Some(activate_handler))];

        if config.show_deactivator {
            let deactivate_name = Self::qualified_name(&group, "deactivate");
            let deactivate_tool = activation_tool(&deactivate_name, &config.description, false);
            let toolset = self.clone();
            let handler_group = group;
            let deactivate_handler: ToolHandler =
                Arc::new(move |ctx: &ServerCtx, _args: Option<Arguments>| {
                    let toolset = toolset.clone();
                    let handler_group = handler_group.clone();
                    let ctx = ctx.clone();
                    Box::pin(async move {
                        let _ = toolset.deactivate_group(&handler_group, &ctx).await?;
                        Ok(CallToolResult::new())
                    })
                });
            entries.push((deactivate_name, deactivate_tool, Some(deactivate_handler)));
        }

        let visibility = Visibility::Always;
        for (name, tool, _handler) in &mut entries {
            self.validate_tool_registration(name, &visibility)?;
            tool.name = name.clone();
        }

        let mut tools = self.tools.write().unwrap_or_else(|err| err.into_inner());
        for (name, _, _) in &entries {
            if tools.contains_key(name) {
                return Err(Error::InvalidConfiguration(format!(
                    "tool already registered: {name}"
                )));
            }
        }

        for (name, tool, handler) in entries {
            tools.insert(
                name,
                ToolEntry {
                    tool,
                    visibility: visibility.clone(),
                    handler,
                    origin: ToolOrigin::AutoGroup,
                },
            );
        }

        Ok(())
    }
}

/// Handler function for a tool.
type ToolHandler = Arc<
    dyn for<'a> Fn(&'a ServerCtx, Option<Arguments>) -> ToolFuture<'a, Result<CallToolResult>>
        + Send
        + Sync,
>;

/// Shared activation hook used in group state.
type SharedActivationHook = Arc<dyn Fn(&ServerCtx) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// A registered tool with its metadata, visibility rule, and handler.
#[derive(Clone)]
struct ToolEntry {
    /// Tool definition used for listing.
    tool: Tool,
    /// Visibility rule for the tool.
    visibility: Visibility,
    /// Optional handler for dynamic tools.
    handler: Option<ToolHandler>,
    /// Origin tag for conflict resolution.
    origin: ToolOrigin,
}

/// Origin metadata for tool registrations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolOrigin {
    /// Tool generated automatically by a group definition.
    AutoGroup,
    /// Tool registered explicitly by the user or macro.
    Explicit,
}

/// Auto-generated activation tool settings derived from a group configuration.
struct AutoGroupConfig {
    /// Fully-qualified group name.
    name: String,
    /// Group description used in auto tool metadata.
    description: String,
    /// Whether to expose the auto deactivator tool.
    show_deactivator: bool,
}

impl From<&GroupConfig> for AutoGroupConfig {
    fn from(config: &GroupConfig) -> Self {
        Self {
            name: config.name.clone(),
            description: config.description.clone(),
            show_deactivator: config.show_deactivator,
        }
    }
}

/// Snapshot of group state used for visibility evaluation.
#[derive(Clone)]
struct GroupSnapshot {
    /// Whether the group is active.
    active: bool,
    /// Optional parent group name.
    parent: Option<String>,
}

/// Shared container for group registry state.
#[derive(Clone, Default)]
struct ToolGroups {
    /// Registry state guarded by a lock.
    registry: Arc<RwLock<GroupRegistry>>,
}

impl ToolGroups {
    /// Register a group configuration.
    fn register_group(&self, config: GroupConfig) -> Result<()> {
        let mut registry = self.registry.write().unwrap_or_else(|err| err.into_inner());
        if registry.groups.contains_key(&config.name) {
            return Err(Error::InvalidConfiguration(format!(
                "group already registered: {}",
                config.name
            )));
        }
        if let Some(parent) = &config.parent {
            if !registry.groups.contains_key(parent) {
                return Err(Error::GroupNotFound(parent.clone()));
            }
            if !config.name.starts_with(&format!("{parent}.")) {
                return Err(Error::InvalidConfiguration(format!(
                    "group '{}' must be nested under parent '{}'",
                    config.name, parent
                )));
            }
        }
        let state = GroupState {
            description: config.description.clone(),
            active: false,
            parent: config.parent.clone(),
            on_activate: config.on_activate.map(Arc::from),
            on_deactivate: config.on_deactivate.map(Arc::from),
        };
        registry.groups.insert(config.name, state);
        Ok(())
    }

    /// Remove a group configuration, if present.
    fn unregister_group(&self, name: &str) {
        let mut registry = self.registry.write().unwrap_or_else(|err| err.into_inner());
        registry.groups.remove(name);
        registry
            .exclusions
            .retain(|exclusion| !exclusion.iter().any(|group| group == name));
    }

    /// Register a mutual exclusion set.
    fn register_exclusion(&self, groups: &[&str]) -> Result<()> {
        if groups.len() < 2 {
            return Err(Error::InvalidConfiguration(
                "exclusion sets require at least two groups".to_string(),
            ));
        }
        let mut unique = HashSet::new();
        let mut entries = Vec::new();
        let registry = self.registry.read().unwrap_or_else(|err| err.into_inner());
        for group in groups {
            if !registry.groups.contains_key(*group) {
                return Err(Error::GroupNotFound((*group).to_string()));
            }
            if unique.insert(*group) {
                entries.push((*group).to_string());
            }
        }
        drop(registry);
        let mut registry = self.registry.write().unwrap_or_else(|err| err.into_inner());
        registry.exclusions.push(entries);
        Ok(())
    }

    /// Ensure a group exists.
    fn ensure_group_exists(&self, name: &str) -> Result<()> {
        let registry = self.registry.read().unwrap_or_else(|err| err.into_inner());
        if registry.groups.contains_key(name) {
            Ok(())
        } else {
            Err(Error::GroupNotFound(name.to_string()))
        }
    }

    /// Check whether a group is active.
    fn is_active(&self, name: &str) -> bool {
        let snapshot = self.snapshot();
        is_group_active_snapshot(&snapshot, name)
    }

    /// Snapshot group state for visibility evaluation.
    fn snapshot(&self) -> HashMap<String, GroupSnapshot> {
        let registry = self.registry.read().unwrap_or_else(|err| err.into_inner());
        registry
            .groups
            .iter()
            .map(|(name, state)| {
                (
                    name.clone(),
                    GroupSnapshot {
                        active: state.active,
                        parent: state.parent.clone(),
                    },
                )
            })
            .collect()
    }

    /// List group info using provided tool counts.
    fn list_groups(&self, tool_counts: &HashMap<String, usize>) -> Vec<GroupInfo> {
        let registry = self.registry.read().unwrap_or_else(|err| err.into_inner());
        registry
            .groups
            .iter()
            .map(|(name, state)| GroupInfo {
                name: name.clone(),
                description: state.description.clone(),
                active: state.active,
                parent: state.parent.clone(),
                tool_count: *tool_counts.get(name).unwrap_or(&0),
            })
            .collect()
    }

    /// Build activation plan for a group.
    fn plan_activation(&self, name: &str) -> Result<GroupChangePlan> {
        let registry = self.registry.read().unwrap_or_else(|err| err.into_inner());
        let snapshot = snapshot_from_registry(&registry);
        let target = registry
            .groups
            .get(name)
            .ok_or_else(|| Error::GroupNotFound(name.to_string()))?;
        if let Some(parent) = &target.parent
            && !is_group_active_snapshot(&snapshot, parent)
        {
            return Err(Error::GroupInactive {
                group: name.to_string(),
                parent: parent.clone(),
            });
        }
        let mut to_deactivate = Vec::new();
        let mut deactivation_set = HashSet::new();
        for exclusion in &registry.exclusions {
            if exclusion.iter().any(|group| group == name) {
                for other in exclusion {
                    if other != name && is_group_active_snapshot(&snapshot, other) {
                        for group in collect_descendants_including_self(&registry.groups, other) {
                            if deactivation_set.insert(group.clone()) {
                                to_deactivate.push(group);
                            }
                        }
                    }
                }
            }
        }
        let mut hooks = Vec::new();
        for group in &to_deactivate {
            if let Some(state) = registry.groups.get(group)
                && state.active
                && let Some(hook) = &state.on_deactivate
            {
                hooks.push(hook.clone());
            }
        }
        if !target.active
            && let Some(hook) = &target.on_activate
        {
            hooks.push(hook.clone());
        }
        let changed = !to_deactivate.is_empty() || !target.active;
        Ok(GroupChangePlan {
            target: name.to_string(),
            deactivate: to_deactivate,
            activate: !target.active,
            hooks,
            changed,
        })
    }

    /// Apply activation changes to group state.
    fn apply_activation(&self, plan: GroupChangePlan) -> Result<bool> {
        if !plan.changed {
            return Ok(false);
        }
        let mut registry = self.registry.write().unwrap_or_else(|err| err.into_inner());
        if !registry.groups.contains_key(&plan.target) {
            return Err(Error::GroupNotFound(plan.target));
        }
        for group in plan.deactivate {
            if let Some(state) = registry.groups.get_mut(&group) {
                state.active = false;
            }
        }
        if plan.activate
            && let Some(state) = registry.groups.get_mut(&plan.target)
        {
            state.active = true;
        }
        Ok(true)
    }

    /// Build deactivation plan for a group.
    fn plan_deactivation(&self, name: &str) -> Result<GroupChangePlan> {
        let registry = self.registry.read().unwrap_or_else(|err| err.into_inner());
        let target = registry
            .groups
            .get(name)
            .ok_or_else(|| Error::GroupNotFound(name.to_string()))?;
        let mut to_deactivate = Vec::new();
        for group in collect_descendants_including_self(&registry.groups, name) {
            to_deactivate.push(group);
        }
        let mut hooks = Vec::new();
        for group in &to_deactivate {
            if let Some(state) = registry.groups.get(group)
                && state.active
                && let Some(hook) = &state.on_deactivate
            {
                hooks.push(hook.clone());
            }
        }
        let changed = target.active
            || to_deactivate.iter().any(|group| {
                registry
                    .groups
                    .get(group)
                    .map(|state| state.active)
                    .unwrap_or(false)
            });
        Ok(GroupChangePlan {
            target: name.to_string(),
            deactivate: to_deactivate,
            activate: false,
            hooks,
            changed,
        })
    }

    /// Apply deactivation changes to group state.
    fn apply_deactivation(&self, plan: GroupChangePlan) -> Result<bool> {
        if !plan.changed {
            return Ok(false);
        }
        let mut registry = self.registry.write().unwrap_or_else(|err| err.into_inner());
        if !registry.groups.contains_key(&plan.target) {
            return Err(Error::GroupNotFound(plan.target));
        }
        for group in plan.deactivate {
            if let Some(state) = registry.groups.get_mut(&group) {
                state.active = false;
            }
        }
        Ok(true)
    }
}

/// Group registry state shared across the tool set.
#[derive(Default)]
struct GroupRegistry {
    /// Group states keyed by name.
    groups: HashMap<String, GroupState>,
    /// Mutually exclusive group sets.
    exclusions: Vec<Vec<String>>,
}

/// Internal group state tracked by the tool set.
struct GroupState {
    /// Description of the group.
    description: String,
    /// Whether the group is currently active.
    active: bool,
    /// Optional parent group name.
    parent: Option<String>,
    /// Optional activation hook.
    on_activate: Option<SharedActivationHook>,
    /// Optional deactivation hook.
    on_deactivate: Option<SharedActivationHook>,
}

/// Planning data for a group activation or deactivation.
struct GroupChangePlan {
    /// Target group for the change.
    target: String,
    /// Groups to deactivate.
    deactivate: Vec<String>,
    /// Whether to activate the target group.
    activate: bool,
    /// Hooks to run before applying the change.
    hooks: Vec<SharedActivationHook>,
    /// Whether any state will change.
    changed: bool,
}

/// Run activation or deactivation hooks in order.
async fn run_activation_hooks(hooks: &[SharedActivationHook], ctx: &ServerCtx) -> Result<()> {
    for hook in hooks {
        hook(ctx).await?;
    }
    Ok(())
}

/// Validate an individual group name segment.
fn validate_group_segment(segment: &str) -> Result<()> {
    if segment.is_empty() || segment.contains('.') {
        return Err(Error::InvalidConfiguration(format!(
            "invalid group segment: {segment}"
        )));
    }
    Ok(())
}

/// Validate a group path containing one or more segments.
fn validate_group_path(group: &str) -> Result<()> {
    if group.is_empty() {
        return Err(Error::InvalidConfiguration(
            "group name is empty".to_string(),
        ));
    }
    for segment in group.split('.') {
        validate_group_segment(segment)?;
    }
    Ok(())
}

/// Split a tool name into group path (if any) and base name.
fn split_tool_name(name: &str) -> Result<(Option<&str>, &str)> {
    let mut parts = name.rsplitn(2, '.');
    let base = parts
        .next()
        .ok_or_else(|| Error::InvalidConfiguration("tool name is empty".to_string()))?;
    if base.is_empty() {
        return Err(Error::InvalidConfiguration(
            "tool name is empty".to_string(),
        ));
    }
    let group = parts.next();
    if let Some(group) = group {
        validate_group_path(group)?;
        Ok((Some(group), base))
    } else {
        Ok((None, base))
    }
}

/// Parse a cursor into a numeric offset.
fn parse_cursor_offset(cursor: Cursor) -> Result<usize> {
    let Cursor(value) = cursor;
    value
        .parse::<usize>()
        .map_err(|_| Error::InvalidParams("invalid cursor".to_string()))
}

/// Determine if a group is active within a snapshot map.
fn is_group_active_snapshot(groups: &HashMap<String, GroupSnapshot>, name: &str) -> bool {
    let mut current = match groups.get(name) {
        Some(state) => state,
        None => return false,
    };
    if !current.active {
        return false;
    }
    let mut guard = HashSet::new();
    while let Some(parent) = current.parent.as_deref() {
        if !guard.insert(parent) {
            return false;
        }
        current = match groups.get(parent) {
            Some(state) => state,
            None => return false,
        };
        if !current.active {
            return false;
        }
    }
    true
}

/// Build a snapshot map from a registry reference.
fn snapshot_from_registry(registry: &GroupRegistry) -> HashMap<String, GroupSnapshot> {
    registry
        .groups
        .iter()
        .map(|(name, state)| {
            (
                name.clone(),
                GroupSnapshot {
                    active: state.active,
                    parent: state.parent.clone(),
                },
            )
        })
        .collect()
}

/// Collect descendants of a group in child-first order.
fn collect_descendants_including_self(
    groups: &HashMap<String, GroupState>,
    root: &str,
) -> Vec<String> {
    let mut collected = Vec::new();
    collect_descendants(groups, root, &mut collected);
    collected.push(root.to_string());
    collected
}

/// Recursively collect descendants for a group.
fn collect_descendants(
    groups: &HashMap<String, GroupState>,
    root: &str,
    collected: &mut Vec<String>,
) {
    for (name, _) in groups
        .iter()
        .filter(|(_, state)| state.parent.as_deref() == Some(root))
    {
        collect_descendants(groups, name, collected);
        collected.push(name.clone());
    }
}

/// Build an auto activation or deactivation tool.
fn activation_tool(name: &str, description: &str, activate: bool) -> Tool {
    let mut tool = Tool::new(name.to_string(), ToolSchema::empty());
    if !description.is_empty() {
        let label = if activate { "Activate" } else { "Deactivate" };
        tool = tool.with_description(format!("{label} {description}"));
    }
    tool
}
