use std::{collections::HashMap, mem, result::Result as StdResult};

use schemars::generate::SchemaSettings;
use serde::{
    Deserialize, Serialize,
    de::{DeserializeOwned, Error as DeError},
};
use serde_json::{Value, json};

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

    /// Create a tool result with structured content.
    pub fn structured(content: impl Serialize) -> StdResult<Self, serde_json::Error> {
        Ok(Self::new().with_structured_content(serde_json::to_value(content)?))
    }

    /// Create a tool error result with a structured error payload.
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new().mark_as_error().with_structured_content(json!({
            "code": code,
            "message": message.into(),
        }))
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

    /// Get the first text content block, if any.
    ///
    /// This is a convenience method for the common pattern of extracting
    /// a single text response from a tool call.
    pub fn text(&self) -> Option<&str> {
        self.content.iter().find_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
    }

    /// Get all text content concatenated together.
    ///
    /// Multiple text blocks are joined with newlines. Returns an empty
    /// string if there are no text content blocks.
    pub fn all_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Parse the first text content block as JSON.
    ///
    /// This is useful when tools return structured JSON data in their
    /// text response.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no text content or if JSON parsing fails.
    pub fn json<T: DeserializeOwned>(&self) -> StdResult<T, serde_json::Error> {
        let text = self
            .text()
            .ok_or_else(|| DeError::custom("no text content in tool result"))?;
        serde_json::from_str(text)
    }

    /// Deserialize the structured content into a typed value.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no structured content or deserialization fails.
    pub fn structured_as<T: DeserializeOwned>(&self) -> StdResult<T, serde_json::Error> {
        let value = self
            .structured_content
            .as_ref()
            .ok_or_else(|| DeError::custom("no structured content in tool result"))?;
        serde_json::from_value(value.clone())
    }
}

/// Convert a response type into a tool result with structured content.
pub trait ToolResponse {
    /// Convert this response into a tool result.
    fn into_call_tool_result(self) -> CallToolResult;
}

impl<T: ToolResponse> From<T> for CallToolResult {
    fn from(value: T) -> Self {
        value.into_call_tool_result()
    }
}

impl ToolResponse for Value {
    fn into_call_tool_result(self) -> CallToolResult {
        CallToolResult::new().with_structured_content(self)
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

    /// Create a tool from a schemars JsonSchema type.
    ///
    /// This derives the input schema from the type's JSON Schema representation.
    /// If the schema has a description at the root level, it will be used as
    /// the tool's description.
    ///
    /// # Example
    ///
    /// ```ignore
    /// #[derive(JsonSchema)]
    /// /// Tool that echoes input back.
    /// struct EchoParams {
    ///     /// The message to echo
    ///     message: String,
    /// }
    ///
    /// let tool = Tool::from_schema::<EchoParams>("echo");
    /// // Tool has name "echo" and schema derived from EchoParams
    /// ```
    pub fn from_schema<T: schemars::JsonSchema>(name: impl Into<String>) -> Self {
        let schema = ToolSchema::from_json_schema::<T>();
        let description = schema.description().map(|s| s.to_string());
        let title = schema.title().map(|s| s.to_string());

        let mut tool = Self::new(name, schema);
        if let Some(desc) = description {
            tool.description = Some(desc);
        }
        if let Some(t) = title {
            tool.title = Some(t);
        }
        tool
    }

    /// Replace the input schema with one derived from a schemars JsonSchema type.
    ///
    /// This is useful when you want to set the schema after construction,
    /// or when combining with other builder methods.
    pub fn with_schema<T: schemars::JsonSchema>(mut self) -> Self {
        self.input_schema = ToolSchema::from_json_schema::<T>();
        self
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
/// tmcp defaults tool schemas to JSON Schema 2020-12.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolSchema(pub Value);

impl Default for ToolSchema {
    fn default() -> Self {
        Self::new(serde_json::json!({
            "type": "object"
        }))
    }
}

impl ToolSchema {
    /// Create a new schema from a JSON value.
    pub fn new(schema: Value) -> Self {
        Self(schema)
    }

    /// Create an empty schema for tools that take no arguments.
    ///
    /// This is an alias for `ToolSchema::default()` that makes the intent
    /// clearer when a tool genuinely takes no parameters.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let tool = Tool::new("ping", ToolSchema::empty())
    ///     .with_description("Ping the server");
    /// ```
    pub fn empty() -> Self {
        Self::default()
    }

    /// Get the schema description if present.
    ///
    /// This extracts the "description" field from the root of the JSON Schema.
    pub fn description(&self) -> Option<&str> {
        self.0.get("description").and_then(|v| v.as_str())
    }

    /// Get the schema title if present.
    ///
    /// This extracts the "title" field from the root of the JSON Schema.
    pub fn title(&self) -> Option<&str> {
        self.0.get("title").and_then(|v| v.as_str())
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
        let mut settings = SchemaSettings::draft2020_12();
        settings.inline_subschemas = true;
        settings.meta_schema = None;
        let generator = settings.into_generator();
        let schema = generator.into_root_schema_for::<T>();
        let mut value =
            serde_json::to_value(&schema).unwrap_or_else(|_| Value::Object(Default::default()));
        simplify_schema_value(&mut value);
        force_object_schema(&mut value);
        Self(value)
    }

    /// Checks if a given property name is required in this schema.
    pub fn is_required(&self, name: &str) -> bool {
        self.required()
            .map(|req| req.contains(&name))
            .unwrap_or(false)
    }
}

/// Normalize generated schemas for stricter JSON-schema consumers.
fn simplify_schema_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(replacement) = simplify_nullable_anyof(map) {
                *value = replacement;
                simplify_schema_value(value);
                return;
            }
            simplify_oneof_enum(map);
            for v in map.values_mut() {
                simplify_schema_value(v);
            }
        }
        Value::Array(items) => {
            for item in items {
                simplify_schema_value(item);
            }
        }
        _ => {}
    }
}

/// Collapse `anyOf` nullability into a nullable type when possible.
fn simplify_nullable_anyof(map: &serde_json::Map<String, Value>) -> Option<Value> {
    let any_of = map.get("anyOf")?.as_array()?;
    if any_of.len() != 2 {
        return None;
    }

    let (_null_idx, other_idx) = if is_null_schema(&any_of[0]) {
        (0, 1)
    } else if is_null_schema(&any_of[1]) {
        (1, 0)
    } else {
        return None;
    };

    let mut replacement = any_of[other_idx].clone();
    if let Value::Object(obj) = &mut replacement {
        for key in ["description", "title", "default", "examples"] {
            if let Some(value) = map.get(key) {
                obj.entry(key.to_string()).or_insert_with(|| value.clone());
            }
        }
    }
    add_null_type(&mut replacement);
    if let Value::Object(obj) = &mut replacement {
        obj.remove("anyOf");
    }
    Some(replacement)
}

/// Collapse `oneOf` const enums into a plain `enum`.
fn simplify_oneof_enum(map: &mut serde_json::Map<String, Value>) {
    let Some(Value::Array(one_of)) = map.get("oneOf") else {
        return;
    };
    if one_of.is_empty() || !one_of.iter().all(is_const_variant) {
        return;
    }

    let mut enums = Vec::with_capacity(one_of.len());
    let mut types = Vec::new();
    for entry in one_of {
        let Value::Object(obj) = entry else { continue };
        if let Some(const_val) = obj.get("const") {
            enums.push(const_val.clone());
        }
        if let Some(Value::String(ty)) = obj.get("type")
            && !types.iter().any(|t| t == ty)
        {
            types.push(ty.clone());
        }
    }
    if enums.is_empty() {
        return;
    }
    map.remove("oneOf");
    map.insert("enum".to_string(), Value::Array(enums));
    if !types.is_empty() {
        match map.get_mut("type") {
            Some(Value::String(existing)) => {
                let mut values = Vec::with_capacity(types.len() + 1);
                values.push(Value::String(mem::take(existing)));
                for ty in types {
                    if !values
                        .iter()
                        .any(|v| matches!(v, Value::String(s) if s == &ty))
                    {
                        values.push(Value::String(ty));
                    }
                }
                *map.get_mut("type").unwrap() = Value::Array(values);
            }
            Some(Value::Array(existing)) => {
                for ty in types {
                    if !existing
                        .iter()
                        .any(|v| matches!(v, Value::String(s) if s == &ty))
                    {
                        existing.push(Value::String(ty));
                    }
                }
            }
            Some(_) => {}
            None => {
                if types.len() == 1 {
                    map.insert("type".to_string(), Value::String(types.remove(0)));
                } else {
                    map.insert(
                        "type".to_string(),
                        Value::Array(types.into_iter().map(Value::String).collect()),
                    );
                }
            }
        }
    }
}

/// Check whether a schema entry looks like a simple const enum variant.
fn is_const_variant(value: &Value) -> bool {
    let Value::Object(obj) = value else {
        return false;
    };
    if !obj.contains_key("const") {
        return false;
    }
    obj.keys().all(|key| {
        matches!(
            key.as_str(),
            "const" | "description" | "type" | "title" | "deprecated"
        )
    })
}

/// Check whether a schema entry represents null.
fn is_null_schema(value: &Value) -> bool {
    match value {
        Value::Object(obj) => {
            if let Some(Value::String(ty)) = obj.get("type") {
                ty == "null"
            } else if let Some(Value::Array(types)) = obj.get("type") {
                types
                    .iter()
                    .any(|t| matches!(t, Value::String(s) if s == "null"))
            } else if let Some(Value::Null) = obj.get("const") {
                true
            } else if let Some(Value::Array(values)) = obj.get("enum") {
                values.iter().any(|v| v.is_null())
            } else {
                false
            }
        }
        _ => false,
    }
}

/// Add nullability to a schema by widening the `type` field.
fn add_null_type(value: &mut Value) {
    let Value::Object(obj) = value else {
        return;
    };
    if let Some(ty) = obj.get_mut("type") {
        match ty {
            Value::String(s) => {
                if s != "null" {
                    let current = mem::take(s);
                    *ty = Value::Array(vec![Value::String(current), Value::String("null".into())]);
                }
            }
            Value::Array(arr) => {
                if !arr
                    .iter()
                    .any(|v| matches!(v, Value::String(s) if s == "null"))
                {
                    arr.push(Value::String("null".into()));
                }
            }
            _ => {}
        }
    } else {
        obj.insert(
            "type".to_string(),
            Value::Array(vec![Value::String("null".into())]),
        );
    }
}

/// Ensure the root schema for tool inputs is an object schema.
fn force_object_schema(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };

    let has_object_shape = map.contains_key("properties")
        || map.contains_key("required")
        || map.contains_key("additionalProperties");

    match map.get_mut("type") {
        Some(Value::String(ty)) => {
            if ty != "object" && has_object_shape {
                *ty = "object".to_string();
            }
        }
        Some(Value::Array(types)) => {
            let has_object = types
                .iter()
                .any(|v| matches!(v, Value::String(s) if s == "object"));
            if has_object || has_object_shape {
                map.insert("type".to_string(), Value::String("object".to_string()));
            }
        }
        Some(_) => {
            if has_object_shape {
                map.insert("type".to_string(), Value::String("object".to_string()));
            }
        }
        None => {
            if has_object_shape {
                map.insert("type".to_string(), Value::String("object".to_string()));
            }
        }
    }

    if matches!(map.get("type"), Some(Value::String(s)) if s == "object") {
        map.entry("properties")
            .or_insert_with(|| Value::Object(Default::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_refs(value: &Value, refs: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if let Some(Value::String(reference)) = map.get("$ref") {
                    refs.push(reference.clone());
                }
                for entry in map.values() {
                    collect_refs(entry, refs);
                }
            }
            Value::Array(items) => {
                for entry in items {
                    collect_refs(entry, refs);
                }
            }
            _ => {}
        }
    }

    fn ref_target_exists(schema: &Value, reference: &str) -> bool {
        let Some(pointer) = reference.strip_prefix('#') else {
            return true;
        };
        if pointer.is_empty() {
            return true;
        }
        schema.pointer(pointer).is_some()
    }

    #[test]
    fn test_call_tool_result_text() {
        // Empty result returns None
        let result = CallToolResult::new();
        assert!(result.text().is_none());

        // Single text content
        let result = CallToolResult::new().with_text_content("hello world");
        assert_eq!(result.text(), Some("hello world"));

        // Multiple text blocks returns first
        let result = CallToolResult::new()
            .with_text_content("first")
            .with_text_content("second");
        assert_eq!(result.text(), Some("first"));
    }

    #[test]
    fn test_call_tool_result_all_text() {
        // Empty result returns empty string
        let result = CallToolResult::new();
        assert_eq!(result.all_text(), "");

        // Single text content
        let result = CallToolResult::new().with_text_content("hello world");
        assert_eq!(result.all_text(), "hello world");

        // Multiple text blocks joined with newlines
        let result = CallToolResult::new()
            .with_text_content("line one")
            .with_text_content("line two")
            .with_text_content("line three");
        assert_eq!(result.all_text(), "line one\nline two\nline three");
    }

    #[test]
    fn test_call_tool_result_json() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Response {
            value: i32,
            message: String,
        }

        // Valid JSON
        let result =
            CallToolResult::new().with_text_content(r#"{"value": 42, "message": "hello"}"#);
        let parsed: Response = result.json().unwrap();
        assert_eq!(parsed.value, 42);
        assert_eq!(parsed.message, "hello");

        // No text content
        let result = CallToolResult::new();
        let err = result.json::<Response>().unwrap_err();
        assert!(err.to_string().contains("no text content"));

        // Invalid JSON
        let result = CallToolResult::new().with_text_content("not json");
        assert!(result.json::<Response>().is_err());
    }

    #[test]
    fn test_call_tool_result_structured() {
        #[derive(serde::Serialize)]
        struct Payload {
            value: i32,
        }

        let result = CallToolResult::structured(Payload { value: 42 }).unwrap();
        assert_eq!(result.structured_content, Some(json!({ "value": 42 })));
        assert_eq!(result.is_error, None);
    }

    #[test]
    fn test_call_tool_result_error() {
        let result = CallToolResult::error("BAD_INPUT", "oops");
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content,
            Some(json!({ "code": "BAD_INPUT", "message": "oops" }))
        );
    }

    #[test]
    fn test_tool_from_schema() {
        use schemars::JsonSchema;

        /// A tool that echoes messages back.
        #[allow(dead_code)] // Fields are read via JsonSchema derivation
        #[derive(JsonSchema)]
        struct EchoParams {
            /// The message to echo
            message: String,
            /// Number of times to repeat
            count: Option<i32>,
        }

        // Tool::from_schema creates a tool with the derived schema
        let tool = Tool::from_schema::<EchoParams>("echo");
        assert_eq!(tool.name, "echo");

        // The schema should have the properties
        let props = tool.input_schema.properties().unwrap();
        assert!(props.contains_key("message"));
        assert!(props.contains_key("count"));

        // Description is extracted from the type's doc comment
        // (schemars includes doc comments in the description field)
        // Note: schemars may or may not include this depending on version/config
    }

    #[test]
    fn test_tool_schema_keeps_ref_definitions() {
        use schemars::JsonSchema;

        #[derive(JsonSchema)]
        struct Node {
            value: i32,
            next: Option<Box<Self>>,
        }

        let schema = ToolSchema::from_json_schema::<Node>();
        let value = schema.as_value();
        let mut refs = Vec::new();
        collect_refs(value, &mut refs);
        assert!(!refs.is_empty(), "expected schema to contain $ref entries");
        for reference in refs {
            assert!(
                ref_target_exists(value, &reference),
                "missing schema definition for {reference}"
            );
        }

        let node = Node {
            value: 1,
            next: None,
        };
        let _ = node.value;
        let _ = node.next;
    }

    #[test]
    fn test_tool_with_schema() {
        use schemars::JsonSchema;

        #[allow(dead_code)] // Fields are read via JsonSchema derivation
        #[derive(JsonSchema)]
        struct MyParams {
            name: String,
        }

        // Start with default schema, then replace with typed schema
        let tool = Tool::new("test", ToolSchema::empty())
            .with_description("A test tool")
            .with_schema::<MyParams>();

        assert_eq!(tool.name, "test");
        assert_eq!(tool.description.as_deref(), Some("A test tool"));

        // The schema now has the MyParams properties
        let props = tool.input_schema.properties().unwrap();
        assert!(props.contains_key("name"));
    }

    #[test]
    fn test_tool_with_title() {
        let tool = Tool::new("test", ToolSchema::empty())
            .with_title("Test Tool")
            .with_description("A test tool");

        assert_eq!(tool.title.as_deref(), Some("Test Tool"));
        assert_eq!(tool.description.as_deref(), Some("A test tool"));
    }

    #[test]
    fn test_tool_schema_empty() {
        let schema = ToolSchema::empty();
        assert_eq!(schema.schema_type(), Some("object"));
        assert!(schema.properties().is_none());
        assert!(schema.as_value().get("$schema").is_none());
    }

    #[test]
    fn test_tool_schema_description_and_title() {
        let schema = ToolSchema::new(serde_json::json!({
            "type": "object",
            "title": "My Schema",
            "description": "A schema for testing"
        }));

        assert_eq!(schema.title(), Some("My Schema"));
        assert_eq!(schema.description(), Some("A schema for testing"));

        // Empty schema has no title or description
        let empty = ToolSchema::empty();
        assert!(empty.title().is_none());
        assert!(empty.description().is_none());
    }

    #[test]
    fn test_tool_schema_from_json_schema_no_dialect() {
        use schemars::JsonSchema;

        #[derive(JsonSchema)]
        struct Params {
            name: String,
        }

        let params = Params {
            name: String::new(),
        };
        let _ = params.name;

        let schema = ToolSchema::from_json_schema::<Params>();
        assert!(schema.as_value().get("$schema").is_none());
    }
}
