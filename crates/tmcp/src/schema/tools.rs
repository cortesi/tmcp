use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::*;
use crate::macros::{with_basename, with_meta};

/// The server's response to a tools/list request from the client.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ListToolsResult {
    /// Tool entries returned by the server.
    pub tools: Vec<Tool>,
    #[serde(rename = "nextCursor", skip_serializing_if = "Option::is_none")]
    /// Cursor for the next page of results.
    pub next_cursor: Option<Cursor>,
}

impl ListToolsResult {
    /// Create an empty tools list result.
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            next_cursor: None,
        }
    }

    /// Add a single tool to the result.
    pub fn with_tool(mut self, tool: Tool) -> Self {
        self.tools.push(tool);
        self
    }

    /// Add multiple tools to the result.
    pub fn with_tools(mut self, tools: impl IntoIterator<Item = Tool>) -> Self {
        self.tools.extend(tools);
        self
    }

    /// Set the pagination cursor for the next page.
    pub fn with_cursor(mut self, cursor: impl Into<Cursor>) -> Self {
        self.next_cursor = Some(cursor.into());
        self
    }
}

/// The server's response to a tool call.
#[with_meta]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    /// Content returned by the tool call.
    pub content: Vec<ContentBlock>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    /// Whether the tool call resulted in an error.
    pub is_error: Option<bool>,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    /// Structured payload returned by the tool, if any.
    pub structured_content: Option<Value>,
}

impl CallToolResult {
    /// Create an empty tool result.
    pub fn new() -> Self {
        Self {
            content: Vec::new(),
            is_error: None,
            structured_content: None,
            _meta: None,
        }
    }

    /// Append a content item to the result.
    pub fn with_content(mut self, content: ContentBlock) -> Self {
        self.content.push(content);
        self
    }

    /// Append a text content item to the result.
    pub fn with_text_content(mut self, text: impl Into<String>) -> Self {
        self.content.push(ContentBlock::Text(TextContent {
            text: text.into(),
            annotations: None,
            _meta: None,
        }));
        self
    }

    /// Mark this result as indicating an error.
    ///
    /// Tool results are successful by default (when `is_error` is `None`),
    /// so this method only needs to be called when the tool execution failed
    /// but you still want to return content describing the failure.
    pub fn mark_as_error(mut self) -> Self {
        self.is_error = Some(true);
        self
    }

    /// Attach structured content to the result.
    pub fn with_structured_content(mut self, content: Value) -> Self {
        self.structured_content = Some(content);
        self
    }
}

impl Default for CallToolResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Additional properties describing a Tool to clients.
///
/// NOTE: all properties in ToolAnnotations are hints.
/// They are not guaranteed to provide a faithful description of
/// tool behavior (including descriptive properties like `title`).
///
/// Clients should never make tool use decisions based on ToolAnnotations
/// received from untrusted servers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolAnnotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional display title for the tool.
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Hint that the tool is read-only.
    pub read_only_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Hint that the tool is destructive.
    pub destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Hint that the tool is idempotent.
    pub idempotent_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Hint that the tool is open-world.
    pub open_world_hint: Option<bool>,
}

/// Execution-related properties for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecution {
    #[serde(rename = "taskSupport", skip_serializing_if = "Option::is_none")]
    pub task_support: Option<ToolTaskSupport>,
}

/// Task support options for tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolTaskSupport {
    Forbidden,
    Optional,
    Required,
}

/// Definition for a tool the client can call.
#[with_meta]
#[with_basename]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "inputSchema")]
    /// JSON Schema describing tool input.
    pub input_schema: ToolSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional tool description.
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Execution-related properties for the tool.
    pub execution: Option<ToolExecution>,
    #[serde(rename = "outputSchema", skip_serializing_if = "Option::is_none")]
    /// JSON Schema describing tool output.
    pub output_schema: Option<ToolSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional annotations describing tool behavior.
    pub annotations: Option<ToolAnnotations>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Optional icons for the tool.
    pub icons: Option<Vec<Icon>>,
}

impl Tool {
    /// Create a new tool with the provided name and input schema.
    pub fn new(name: impl Into<String>, input_schema: ToolSchema) -> Self {
        Self {
            input_schema,
            description: None,
            execution: None,
            output_schema: None,
            annotations: None,
            icons: None,
            name: name.into(),
            title: None,
            _meta: None,
        }
    }

    /// Set the tool description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the output schema for the tool.
    pub fn with_output_schema(mut self, schema: ToolSchema) -> Self {
        self.output_schema = Some(schema);
        self
    }

    /// Set execution metadata for the tool.
    pub fn with_execution(mut self, execution: ToolExecution) -> Self {
        self.execution = Some(execution);
        self
    }

    /// Set task support for the tool.
    pub fn with_task_support(mut self, support: ToolTaskSupport) -> Self {
        self.execution
            .get_or_insert(ToolExecution { task_support: None })
            .task_support = Some(support);
        self
    }

    /// Set the annotation title hint.
    pub fn with_annotation_title(mut self, title: impl Into<String>) -> Self {
        self.annotations.get_or_insert_with(Default::default).title = Some(title.into());
        self
    }

    /// Set the read-only hint.
    pub fn with_read_only_hint(mut self, read_only: bool) -> Self {
        self.annotations
            .get_or_insert_with(Default::default)
            .read_only_hint = Some(read_only);
        self
    }

    /// Set the destructive hint.
    pub fn with_destructive_hint(mut self, destructive: bool) -> Self {
        self.annotations
            .get_or_insert_with(Default::default)
            .destructive_hint = Some(destructive);
        self
    }

    /// Set the idempotent hint.
    pub fn with_idempotent_hint(mut self, idempotent: bool) -> Self {
        self.annotations
            .get_or_insert_with(Default::default)
            .idempotent_hint = Some(idempotent);
        self
    }

    /// Set the open-world hint.
    pub fn with_open_world_hint(mut self, open_world: bool) -> Self {
        self.annotations
            .get_or_insert_with(Default::default)
            .open_world_hint = Some(open_world);
        self
    }

    /// Replace the annotations entirely.
    pub fn with_annotations(mut self, annotations: ToolAnnotations) -> Self {
        self.annotations = Some(annotations);
        self
    }

    /// Set the icons for the tool.
    pub fn with_icons(mut self, icons: impl IntoIterator<Item = Icon>) -> Self {
        self.icons = Some(icons.into_iter().collect());
        self
    }
}

/// A JSON Schema object defining the input or output schema for a tool.
///
/// This type preserves the complete JSON Schema, including all fields like
/// descriptions, enums, formats, and constraints. It serializes transparently
/// as a JSON Schema object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolSchema(pub Value);

impl Default for ToolSchema {
    fn default() -> Self {
        Self(serde_json::json!({
            "type": "object"
        }))
    }
}

impl ToolSchema {
    /// Create a new schema from a JSON value.
    pub fn new(schema: Value) -> Self {
        Self(schema)
    }

    /// Get the underlying JSON value.
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Get a mutable reference to the underlying JSON value.
    pub fn as_value_mut(&mut self) -> &mut Value {
        &mut self.0
    }

    /// Get the schema type (e.g., "object", "string").
    pub fn schema_type(&self) -> Option<&str> {
        self.0.get("type").and_then(|v| v.as_str())
    }

    /// Get the properties map if this is an object schema.
    pub fn properties(&self) -> Option<&serde_json::Map<String, Value>> {
        self.0.get("properties").and_then(|v| v.as_object())
    }

    /// Get the required field names if this is an object schema.
    pub fn required(&self) -> Option<Vec<&str>> {
        self.0
            .get("required")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
    }

    /// Add a property schema.
    pub fn with_property(mut self, name: impl Into<String>, schema: Value) -> Self {
        if let Some(obj) = self.0.as_object_mut() {
            let properties = obj
                .entry("properties")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(props) = properties.as_object_mut() {
                props.insert(name.into(), schema);
            }
        }
        self
    }

    /// Replace the properties map.
    pub fn with_properties(mut self, properties: HashMap<String, Value>) -> Self {
        if let Some(obj) = self.0.as_object_mut() {
            obj.insert(
                "properties".to_string(),
                Value::Object(properties.into_iter().collect()),
            );
        }
        self
    }

    /// Add a required property name.
    pub fn with_required(mut self, name: impl Into<String>) -> Self {
        if let Some(obj) = self.0.as_object_mut() {
            let required = obj
                .entry("required")
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(arr) = required.as_array_mut() {
                arr.push(Value::String(name.into()));
            }
        }
        self
    }

    /// Add multiple required property names.
    pub fn with_required_properties(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        if let Some(obj) = self.0.as_object_mut() {
            let required = obj
                .entry("required")
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(arr) = required.as_array_mut() {
                arr.extend(names.into_iter().map(|n| Value::String(n.into())));
            }
        }
        self
    }

    /// Build a schema from a schemars JsonSchema type.
    ///
    /// This preserves the complete schema including descriptions, enums,
    /// formats, and all other JSON Schema features.
    pub fn from_json_schema<T: schemars::JsonSchema>() -> Self {
        let schema = schemars::schema_for!(T);
        Self(schema.as_value().clone())
    }

    /// Checks if a given property name is required in this schema.
    pub fn is_required(&self, name: &str) -> bool {
        self.required()
            .map(|req| req.contains(&name))
            .unwrap_or(false)
    }
}
