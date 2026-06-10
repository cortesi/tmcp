//! Human-oriented rendering for MCP API snapshots.

use std::{
    collections::BTreeSet,
    fmt::{Arguments, Write},
};

use owo_colors::{OwoColorize, Style};
use serde_json::{Map, Value};

use crate::McpApi;

/// Rendering options for human-facing MCP API output.
#[derive(Clone, Copy, Debug, Default)]
pub struct McpApiRenderOptions {
    /// Whether ANSI color should be emitted.
    pub color: bool,
}

/// Render styling for human-facing MCP API output.
#[derive(Clone, Copy, Debug)]
struct RenderTheme {
    /// Rendering options selected by the caller.
    options: McpApiRenderOptions,
}

impl RenderTheme {
    /// Create a display theme.
    fn new(options: McpApiRenderOptions) -> Self {
        Self { options }
    }

    /// Style the top-level heading.
    fn header(self, text: &str) -> String {
        self.apply(text, Style::new().bold().cyan())
    }

    /// Style a section heading.
    fn section(self, text: &str) -> String {
        self.apply(text, Style::new().bold().blue())
    }

    /// Style an API item name.
    fn item(self, text: &str) -> String {
        self.apply(text, Style::new().bold())
    }

    /// Style a field label.
    fn label(self, text: &str) -> String {
        self.apply(text, Style::new().dimmed())
    }

    /// Style literal identifiers and values.
    fn literal(self, text: &str) -> String {
        self.apply(text, Style::new().green())
    }

    /// Style descriptive prose.
    fn prose(self, text: &str) -> String {
        self.apply(text, Style::new().italic().dimmed())
    }

    /// Apply one `owo-colors` style when color is enabled.
    fn apply(self, text: &str, style: Style) -> String {
        if self.options.color {
            format!("{}", text.style(style))
        } else {
            text.to_owned()
        }
    }
}

/// Render an MCP API snapshot for a human reader.
pub fn render_mcp_api(api: &McpApi, options: McpApiRenderOptions) -> String {
    let value = serde_json::to_value(api).expect("McpApi can be serialized to JSON");
    render_mcp_api_value(&value, RenderTheme::new(options))
}

/// Render a serialized MCP API object.
fn render_mcp_api_value(value: &Value, theme: RenderTheme) -> String {
    let mut out = String::new();
    push_line(&mut out, 0, &theme.header("MCP API"));
    if let Some(initialize) = value.get("initialize").and_then(Value::as_object) {
        render_initialize(&mut out, initialize, theme);
    }
    render_collection(
        &mut out,
        value,
        "Tools",
        "tools",
        EmptyCollection::Show,
        theme,
        render_tool,
    );
    render_collection(
        &mut out,
        value,
        "Resources",
        "resources",
        EmptyCollection::Omit,
        theme,
        render_resource,
    );
    render_collection(
        &mut out,
        value,
        "Resource Templates",
        "resourceTemplates",
        EmptyCollection::Omit,
        theme,
        render_resource_template,
    );
    render_collection(
        &mut out,
        value,
        "Prompts",
        "prompts",
        EmptyCollection::Omit,
        theme,
        render_prompt,
    );
    out
}

/// Render the initialize response.
fn render_initialize(out: &mut String, initialize: &Map<String, Value>, theme: RenderTheme) {
    push_blank(out);
    push_line(out, 0, &theme.section("Server"));
    if let Some(server_info) = initialize.get("serverInfo").and_then(Value::as_object) {
        render_server_info(out, server_info, theme);
    }
    render_optional_field(out, initialize, "protocolVersion", "Protocol", theme, 1);
    render_optional_field(out, initialize, "instructions", "Instructions", theme, 1);
    if let Some(capabilities) = initialize.get("capabilities") {
        push_line(out, 1, &theme.label("Capabilities"));
        render_value_tree(out, capabilities, theme, 2);
    }
    render_extra_fields(
        out,
        initialize,
        &[
            "protocolVersion",
            "serverInfo",
            "instructions",
            "capabilities",
        ],
        theme,
        1,
    );
}

/// Render server implementation metadata.
fn render_server_info(out: &mut String, server_info: &Map<String, Value>, theme: RenderTheme) {
    render_optional_field(out, server_info, "name", "Name", theme, 1);
    render_optional_field(out, server_info, "version", "Version", theme, 1);
    render_optional_field(out, server_info, "title", "Title", theme, 1);
    render_description(out, server_info, theme, 1);
    render_optional_field(out, server_info, "websiteUrl", "Website", theme, 1);
    if let Some(icons) = server_info.get("icons") {
        render_json_field(out, "Icons", icons, theme, 1);
    }
    render_extra_fields(
        out,
        server_info,
        &[
            "name",
            "version",
            "title",
            "description",
            "websiteUrl",
            "icons",
        ],
        theme,
        1,
    );
}

/// Render a named top-level collection.
fn render_collection(
    out: &mut String,
    root: &Value,
    title: &str,
    key: &str,
    empty_collection: EmptyCollection,
    theme: RenderTheme,
    render_item: fn(&mut String, &Map<String, Value>, RenderTheme),
) {
    let items = root
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if items.is_empty() && matches!(empty_collection, EmptyCollection::Omit) {
        return;
    }
    push_blank(out);
    push_line(
        out,
        0,
        &theme.section(&format!("{title} ({})", items.len())),
    );
    if items.is_empty() {
        push_line(out, 1, "None advertised.");
        return;
    }
    for item in items {
        match item.as_object() {
            Some(object) => render_item(out, object, theme),
            None => render_value_tree(out, item, theme, 1),
        }
    }
}

/// How to render an empty top-level API collection.
#[derive(Clone, Copy, Debug)]
enum EmptyCollection {
    /// Render the section and include an empty-state line.
    Show,
    /// Skip the section entirely.
    Omit,
}

/// Render one tool entry.
fn render_tool(out: &mut String, tool: &Map<String, Value>, theme: RenderTheme) {
    push_blank(out);
    let name = string_field(tool, "name").unwrap_or("<unnamed>");
    push_line(out, 1, &theme.item(name));
    render_optional_field(out, tool, "title", "Title", theme, 2);
    render_description(out, tool, theme, 2);
    render_tool_execution(out, tool.get("execution"), theme, 2);
    render_input_schema_section(out, tool.get("inputSchema"), theme, 2);
    render_schema_section(out, "Output", tool.get("outputSchema"), theme, 2);
    if let Some(annotations) = tool.get("annotations") {
        render_json_field(out, "Annotations", annotations, theme, 2);
    }
    if let Some(icons) = tool.get("icons") {
        render_json_field(out, "Icons", icons, theme, 2);
    }
    render_extra_fields(
        out,
        tool,
        &[
            "name",
            "title",
            "description",
            "execution",
            "inputSchema",
            "outputSchema",
            "annotations",
            "icons",
        ],
        theme,
        2,
    );
}

/// Render tool execution metadata.
fn render_tool_execution(
    out: &mut String,
    execution: Option<&Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let Some(execution) = execution else {
        return;
    };
    let Some(object) = execution.as_object() else {
        render_json_field(out, "Execution", execution, theme, indent);
        return;
    };
    if object.is_empty() {
        return;
    }
    if object.len() == 1
        && let Some(task_support) = object.get("taskSupport")
    {
        render_json_field(out, "Task support", task_support, theme, indent);
        return;
    }
    render_json_field(out, "Execution", execution, theme, indent);
}

/// Render a tool input schema when the tool accepts input.
fn render_input_schema_section(
    out: &mut String,
    schema: Option<&Value>,
    theme: RenderTheme,
    indent: usize,
) {
    if schema.is_some_and(is_empty_object_schema) {
        return;
    }
    render_schema_section(out, "Input", schema, theme, indent);
}

/// Render one resource entry.
fn render_resource(out: &mut String, resource: &Map<String, Value>, theme: RenderTheme) {
    render_named_uri_item(out, resource, "uri", theme);
    render_optional_field(out, resource, "mimeType", "MIME type", theme, 2);
    render_optional_field(out, resource, "size", "Size", theme, 2);
    render_annotations_and_extras(
        out,
        resource,
        &[
            "name",
            "title",
            "uri",
            "description",
            "mimeType",
            "size",
            "annotations",
            "icons",
        ],
        theme,
    );
}

/// Render one resource template entry.
fn render_resource_template(out: &mut String, template: &Map<String, Value>, theme: RenderTheme) {
    render_named_uri_item(out, template, "uriTemplate", theme);
    render_optional_field(out, template, "mimeType", "MIME type", theme, 2);
    render_annotations_and_extras(
        out,
        template,
        &[
            "name",
            "title",
            "uriTemplate",
            "description",
            "mimeType",
            "annotations",
            "icons",
        ],
        theme,
    );
}

/// Render one prompt entry.
fn render_prompt(out: &mut String, prompt: &Map<String, Value>, theme: RenderTheme) {
    push_blank(out);
    let name = string_field(prompt, "name").unwrap_or("<unnamed>");
    push_line(out, 1, &theme.item(name));
    render_optional_field(out, prompt, "title", "Title", theme, 2);
    render_description(out, prompt, theme, 2);
    if let Some(arguments) = prompt.get("arguments") {
        render_json_field(out, "Arguments", arguments, theme, 2);
    }
    render_annotations_and_extras(
        out,
        prompt,
        &["name", "title", "description", "arguments", "icons"],
        theme,
    );
}

/// Render a resource-like entry with a name and URI field.
fn render_named_uri_item(
    out: &mut String,
    object: &Map<String, Value>,
    uri_key: &str,
    theme: RenderTheme,
) {
    push_blank(out);
    let name = string_field(object, "name").unwrap_or("<unnamed>");
    push_line(out, 1, &theme.item(name));
    render_optional_field(out, object, "title", "Title", theme, 2);
    render_optional_field(out, object, uri_key, "URI", theme, 2);
    render_description(out, object, theme, 2);
}

/// Render annotations, icons, and unhandled fields shared by several item types.
fn render_annotations_and_extras(
    out: &mut String,
    object: &Map<String, Value>,
    handled: &[&str],
    theme: RenderTheme,
) {
    if let Some(annotations) = object.get("annotations") {
        render_json_field(out, "Annotations", annotations, theme, 2);
    }
    if let Some(icons) = object.get("icons") {
        render_json_field(out, "Icons", icons, theme, 2);
    }
    render_extra_fields(out, object, handled, theme, 2);
}

/// Render a schema-valued section.
fn render_schema_section(
    out: &mut String,
    title: &str,
    schema: Option<&Value>,
    theme: RenderTheme,
    indent: usize,
) {
    push_line(out, indent, &theme.label(title));
    match schema {
        Some(Value::Null) | None => push_line(out, indent + 1, "Not advertised."),
        Some(schema) => render_schema_root(out, schema, theme, indent + 1),
    }
}

/// Render a root schema.
fn render_schema_root(out: &mut String, schema: &Value, theme: RenderTheme, indent: usize) {
    render_schema_body(out, schema, theme, indent, SchemaRenderContext::Root);
}

/// Render a root schema beneath a choice heading.
fn render_schema_choice_body(out: &mut String, schema: &Value, theme: RenderTheme, indent: usize) {
    render_schema_body(out, schema, theme, indent, SchemaRenderContext::Choice);
}

/// Render one schema body with context-sensitive heading elision.
fn render_schema_body(
    out: &mut String,
    schema: &Value,
    theme: RenderTheme,
    indent: usize,
    context: SchemaRenderContext,
) {
    let Some(object) = schema.as_object() else {
        render_value_tree(out, schema, theme, indent);
        return;
    };
    if schema_kind(schema) == "object" {
        render_schema_title(out, object, theme, indent);
    }
    render_description(out, object, theme, indent);
    if should_render_schema_type(object, context) {
        push_line_fmt(
            out,
            indent,
            format_args!(
                "{} {}",
                theme.label("Type:"),
                theme.literal(&schema_kind(schema))
            ),
        );
    }
    if matches!(context, SchemaRenderContext::Root) {
        render_schema_constraints(out, object, theme, indent);
    }
    render_empty_object_schema(out, object, indent);
    render_schema_children(out, object, theme, indent);
}

/// Where a schema is being rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SchemaRenderContext {
    /// Standalone section or nested object body.
    Root,
    /// Body below a choice heading that already summarizes type and constraints.
    Choice,
}

/// Render an object schema title as a compact type heading.
fn render_schema_title(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let Some(title) = string_field(object, "title") else {
        return;
    };
    push_line(out, indent, &theme.literal(title));
}

/// Render one object property schema.
fn render_schema_property(
    out: &mut String,
    name: &str,
    schema: &Value,
    required: bool,
    theme: RenderTheme,
    indent: usize,
) {
    let requirement = if required { "required" } else { "optional" };
    let mut summary = vec![schema_kind(schema), requirement.to_owned()];
    if let Some(object) = schema.as_object() {
        summary.extend(schema_constraint_parts(object));
    }
    push_line_fmt(
        out,
        indent,
        format_args!("- {} ({})", theme.literal(name), summary.join(", ")),
    );
    if let Some(object) = schema.as_object() {
        render_description(out, object, theme, indent + 1);
        render_schema_children(out, object, theme, indent + 1);
    }
}

/// Render nested schema keywords.
fn render_schema_children(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    render_schema_properties(out, object, theme, indent);
    render_schema_additional_properties(out, object, theme, indent);
    render_schema_array_items(out, object, theme, indent);
    render_schema_choices(out, object, theme, indent);
    render_schema_constants(out, object, theme, indent);
    render_schema_definitions(out, object, theme, indent);
    render_schema_extras(out, object, theme, indent);
}

/// Render a no-fields marker for an object schema with no object-specific detail.
fn render_empty_object_schema(out: &mut String, object: &Map<String, Value>, indent: usize) {
    if matches!(object.get("type").and_then(Value::as_str), Some("object"))
        && !object.contains_key("properties")
        && !object.contains_key("additionalProperties")
    {
        push_line(out, indent, "No fields.");
    }
}

/// Render object properties.
fn render_schema_properties(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        return;
    };
    if properties.is_empty() {
        push_line(out, indent, "No fields.");
        return;
    }
    let required = required_fields(object);
    push_line(out, indent, &theme.label("Fields"));
    for (name, property) in properties {
        render_schema_property(
            out,
            name,
            property,
            required.contains(name),
            theme,
            indent + 1,
        );
    }
}

/// Whether a standalone schema type line adds useful information.
fn should_render_schema_type(object: &Map<String, Value>, context: SchemaRenderContext) -> bool {
    if matches!(context, SchemaRenderContext::Choice) {
        return false;
    }
    if object.get("type").is_none() && has_schema_choices(object) {
        return false;
    }
    !matches!(object.get("type").and_then(Value::as_str), Some("object"))
        || object
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(Map::is_empty)
}

/// Whether a schema object contains a JSON Schema choice keyword.
fn has_schema_choices(object: &Map<String, Value>) -> bool {
    ["anyOf", "oneOf", "allOf"]
        .into_iter()
        .any(|key| object.contains_key(key))
}

/// Whether a schema describes the conventional no-argument tool input object.
fn is_empty_object_schema(schema: &Value) -> bool {
    let Some(object) = schema.as_object() else {
        return false;
    };
    matches!(object.get("type").and_then(Value::as_str), Some("object"))
        && object
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(Map::is_empty)
        && object
            .get("required")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        && object
            .get("additionalProperties")
            .is_none_or(|value| matches!(value, Value::Bool(false)))
}

/// Render additional properties schema.
fn render_schema_additional_properties(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let Some(additional) = object.get("additionalProperties") else {
        return;
    };
    push_line(out, indent, &theme.label("Additional properties"));
    match additional {
        Value::Object(_) => render_schema_root(out, additional, theme, indent + 1),
        value => render_value_tree(out, value, theme, indent + 1),
    }
}

/// Render array item schema.
fn render_schema_array_items(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let Some(items) = object.get("items") else {
        return;
    };
    push_line(out, indent, &theme.label("Items"));
    render_schema_root(out, items, theme, indent + 1);
}

/// Render schema choices.
fn render_schema_choices(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    for (key, title) in [
        ("anyOf", "Any of"),
        ("oneOf", "One of"),
        ("allOf", "All of"),
    ] {
        let Some(choices) = object.get(key).and_then(Value::as_array) else {
            continue;
        };
        push_line(out, indent, &theme.label(title));
        for choice in choices {
            push_line_fmt(
                out,
                indent + 1,
                format_args!("- {}", theme.literal(&schema_choice_summary(choice))),
            );
            render_schema_choice_body(out, choice, theme, indent + 2);
        }
    }
}

/// Render enum, const, and default schema values.
fn render_schema_constants(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        let values = values
            .iter()
            .map(value_inline)
            .collect::<Vec<_>>()
            .join(", ");
        push_line_fmt(
            out,
            indent,
            format_args!("{} {values}", theme.label("Allowed values:")),
        );
    }
    if let Some(value) = object.get("const") {
        push_line_fmt(
            out,
            indent,
            format_args!("{} {}", theme.label("Constant:"), value_inline(value)),
        );
    }
    render_schema_default(out, object, theme, indent);
}

/// Render schema definitions.
fn render_schema_definitions(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    for key in ["$defs", "definitions"] {
        let Some(definitions) = object.get(key).and_then(Value::as_object) else {
            continue;
        };
        push_line(out, indent, &theme.label("Definitions"));
        for (name, definition) in definitions {
            push_line(out, indent + 1, &theme.literal(name));
            render_schema_root(out, definition, theme, indent + 2);
        }
    }
}

/// Render schema keys not covered by the semantic renderer.
fn render_schema_extras(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    render_extra_fields(out, object, SCHEMA_KEYS, theme, indent);
}

/// Render constraints that fit naturally on one line.
fn render_schema_constraints(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let constraints = schema_constraint_parts(object);
    if !constraints.is_empty() {
        push_line_fmt(
            out,
            indent,
            format_args!("{} {}", theme.label("Constraints:"), constraints.join(", ")),
        );
    }
}

/// Render a default value when one is present.
fn render_schema_default(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    if let Some(default) = object.get("default") {
        push_line_fmt(
            out,
            indent,
            format_args!("{} {}", theme.label("Default:"), value_inline(default)),
        );
    }
}

/// Return concise schema constraint fragments.
fn schema_constraint_parts(object: &Map<String, Value>) -> Vec<String> {
    [
        "format",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "minLength",
        "maxLength",
        "pattern",
        "minItems",
        "maxItems",
    ]
    .into_iter()
    .filter_map(|key| {
        object
            .get(key)
            .map(|value| format!("{key} {}", value_inline(value)))
    })
    .collect()
}

/// Return a concise heading for one schema choice.
fn schema_choice_summary(schema: &Value) -> String {
    let mut summary = vec![schema_kind(schema)];
    if let Some(object) = schema.as_object() {
        summary.extend(schema_constraint_parts(object));
    }
    summary.join(", ")
}

/// Return required field names for an object schema.
fn required_fields(object: &Map<String, Value>) -> BTreeSet<String> {
    object
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Return a compact schema type label.
fn schema_kind(schema: &Value) -> String {
    let Some(object) = schema.as_object() else {
        return value_inline(schema);
    };
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return format!("ref {reference}");
    }
    if let Some(kind) = object.get("type") {
        return schema_type(kind);
    }
    for (key, label) in [
        ("anyOf", "any of"),
        ("oneOf", "one of"),
        ("allOf", "all of"),
    ] {
        if object.contains_key(key) {
            return label.to_owned();
        }
    }
    "unspecified".to_owned()
}

/// Return a compact representation for a JSON Schema `type` value.
fn schema_type(kind: &Value) -> String {
    match kind {
        Value::String(kind) => kind.clone(),
        Value::Array(kinds) => kinds
            .iter()
            .map(value_inline)
            .collect::<Vec<_>>()
            .join(" | "),
        value => value_inline(value),
    }
}

/// Render an optional object field.
fn render_optional_field(
    out: &mut String,
    object: &Map<String, Value>,
    key: &str,
    label: &str,
    theme: RenderTheme,
    indent: usize,
) {
    if let Some(value) = object.get(key) {
        render_json_field(out, label, value, theme, indent);
    }
}

/// Render a schema or item description as prose.
fn render_description(
    out: &mut String,
    object: &Map<String, Value>,
    theme: RenderTheme,
    indent: usize,
) {
    let Some(description) = object.get("description") else {
        return;
    };
    match description {
        Value::String(text) => render_prose(out, text, theme, indent),
        value => render_json_field(out, "Description", value, theme, indent),
    }
}

/// Render fields not named in `handled`.
fn render_extra_fields(
    out: &mut String,
    object: &Map<String, Value>,
    handled: &[&str],
    theme: RenderTheme,
    indent: usize,
) {
    for (key, value) in object {
        if !handled.contains(&key.as_str()) {
            render_json_field(out, key, value, theme, indent);
        }
    }
}

/// Render descriptive prose with no repeated label.
fn render_prose(out: &mut String, text: &str, theme: RenderTheme, indent: usize) {
    for line in text.lines() {
        push_line(out, indent, &theme.prose(&format!("> {line}")));
    }
}

/// Render one JSON field.
fn render_json_field(
    out: &mut String,
    key: &str,
    value: &Value,
    theme: RenderTheme,
    indent: usize,
) {
    match value {
        Value::String(text) if text.contains('\n') => {
            render_multiline_field(out, &display_key(key), text, theme, indent)
        }
        Value::Array(items) if !is_scalar_array(items) => {
            push_line(out, indent, &theme.label(&display_key(key)));
            render_value_tree(out, value, theme, indent + 1);
        }
        Value::Object(_) => {
            push_line(out, indent, &theme.label(&display_key(key)));
            render_value_tree(out, value, theme, indent + 1);
        }
        value => push_line_fmt(
            out,
            indent,
            format_args!(
                "{} {}",
                theme.label(&format!("{}:", display_key(key))),
                value_inline(value)
            ),
        ),
    }
}

/// Render a multiline text field.
fn render_multiline_field(
    out: &mut String,
    key: &str,
    value: &str,
    theme: RenderTheme,
    indent: usize,
) {
    let mut lines = value.lines();
    let first = lines.next().unwrap_or_default();
    push_line_fmt(
        out,
        indent,
        format_args!("{} {first}", theme.label(&format!("{key}:"))),
    );
    for line in lines {
        push_line(out, indent + 1, line);
    }
}

/// Render an arbitrary JSON value as an indented tree.
fn render_value_tree(out: &mut String, value: &Value, theme: RenderTheme, indent: usize) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                render_json_field(out, key, value, theme, indent);
            }
        }
        Value::Array(items) => render_array_tree(out, items, theme, indent),
        value => push_line(out, indent, &value_inline(value)),
    }
}

/// Render an array as an indented tree.
fn render_array_tree(out: &mut String, items: &[Value], theme: RenderTheme, indent: usize) {
    if items.is_empty() {
        push_line(out, indent, "[]");
        return;
    }
    for item in items {
        match item {
            Value::Object(_) | Value::Array(_) => {
                push_line(out, indent, "-");
                render_value_tree(out, item, theme, indent + 1);
            }
            value => push_line_fmt(out, indent, format_args!("- {}", value_inline(value))),
        }
    }
}

/// Return whether every array item can fit on one line.
fn is_scalar_array(items: &[Value]) -> bool {
    items.iter().all(|item| {
        matches!(
            item,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
        )
    })
}

/// Return an inline value representation.
fn value_inline(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(items) => items
            .iter()
            .map(value_inline)
            .collect::<Vec<_>>()
            .join(" | "),
        Value::Object(_) => serde_json::to_string(value).unwrap_or_else(|_| "<object>".to_owned()),
    }
}

/// Return a string field from an object.
fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

/// Convert a serialized field key into a display label.
fn display_key(key: &str) -> String {
    let mut out = String::new();
    for (index, char) in key.chars().enumerate() {
        if index > 0 && char.is_uppercase() {
            out.push(' ');
        }
        if char == '_' {
            out.push(' ');
        } else {
            out.extend(char.to_lowercase());
        }
    }
    out
}

/// Append a blank line.
fn push_blank(out: &mut String) {
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// Append an indented line.
fn push_line(out: &mut String, indent: usize, line: &str) {
    push_indent(out, indent);
    out.push_str(line);
    out.push('\n');
}

/// Append an indented line built from format arguments.
fn push_line_fmt(out: &mut String, indent: usize, args: Arguments<'_>) {
    push_indent(out, indent);
    out.write_fmt(args)
        .expect("writing to a String cannot fail");
    out.push('\n');
}

/// Append two-space indentation without allocating.
fn push_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// JSON Schema keys that receive explicit rendering.
const SCHEMA_KEYS: &[&str] = &[
    "$defs",
    "$ref",
    "$schema",
    "additionalProperties",
    "allOf",
    "anyOf",
    "const",
    "default",
    "definitions",
    "description",
    "enum",
    "examples",
    "exclusiveMaximum",
    "exclusiveMinimum",
    "format",
    "items",
    "maxItems",
    "maxLength",
    "maximum",
    "minItems",
    "minLength",
    "minimum",
    "oneOf",
    "pattern",
    "properties",
    "required",
    "title",
    "type",
];

#[cfg(test)]
#[allow(clippy::missing_docs_in_private_items)]
mod tests {
    use serde_json::{Value, json};

    use super::{McpApiRenderOptions, RenderTheme, render_mcp_api_value};

    #[test]
    fn render_mcp_api_value_shows_human_schema() {
        let rendered = render_plain(&human_schema_api());

        assert_human_schema_content(&rendered);
        assert_human_schema_noise_is_omitted(&rendered);
    }

    #[test]
    fn render_mcp_api_value_can_emit_color() {
        let api = json!({
            "initialize": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "serverInfo": { "name": "server", "version": "1.0.0" }
            },
            "tools": [],
            "resources": [],
            "resourceTemplates": [],
            "prompts": []
        });

        let rendered =
            render_mcp_api_value(&api, RenderTheme::new(McpApiRenderOptions { color: true }));

        assert!(rendered.contains("\x1b["));
    }

    fn render_plain(api: &Value) -> String {
        render_mcp_api_value(api, RenderTheme::new(McpApiRenderOptions { color: false }))
    }

    fn human_schema_api() -> Value {
        serde_json::from_str(
            r#"
            {
                "initialize": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {
                        "tools": { "listChanged": true }
                    },
                    "serverInfo": {
                        "name": "porter_server",
                        "version": "0.1.0"
                    }
                },
                "tools": [
                    {
                        "name": "eval",
                        "description": "Type-check and execute a Luau script.",
                        "execution": { "taskSupport": "optional" },
                        "inputSchema": {
                            "title": "EvalParams",
                            "type": "object",
                            "required": ["script"],
                            "properties": {
                                "script": {
                                    "type": "string",
                                    "description": "Luau source code."
                                },
                                "timeout_ms": {
                                    "type": ["integer", "null"],
                                    "format": "uint64",
                                    "minimum": 0
                                },
                                "value": {
                                    "description": "Typed value.",
                                    "anyOf": [
                                        {
                                            "type": "string",
                                            "description": "String value."
                                        },
                                        {
                                            "type": "number",
                                            "format": "double"
                                        }
                                    ]
                                }
                            }
                        },
                        "outputSchema": {
                            "title": "string",
                            "type": "string"
                        }
                    },
                    {
                        "name": "api",
                        "inputSchema": {
                            "type": "object"
                        },
                        "outputSchema": {
                            "type": "string"
                        }
                    }
                ],
                "resources": [],
                "resourceTemplates": [],
                "prompts": []
            }
            "#,
        )
        .expect("human schema API fixture")
    }

    fn assert_human_schema_content(rendered: &str) {
        for expected in [
            "MCP API",
            "Tools (2)",
            "eval",
            "api",
            "EvalParams",
            "script (string, required)",
            "timeout_ms (integer | null, optional, format uint64, minimum 0)",
            "Type: string",
            "task support: optional",
            "> Type-check and execute a Luau script.",
            "> Luau source code.",
            "- value (any of, optional)",
            "> Typed value.",
            "- string",
            "> String value.",
            "- number, format double",
        ] {
            assert!(rendered.contains(expected), "missing {expected:?}");
        }
    }

    fn assert_human_schema_noise_is_omitted(rendered: &str) {
        for unexpected in [
            "title: EvalParams",
            "Type: object",
            "No fields.",
            "title: string",
            "description:",
            "execution\n",
            "Type: any of",
            "- option 1",
            "inputSchema",
            "Resources (0)",
            "Resource Templates (0)",
            "Prompts (0)",
            "\x1b[",
        ] {
            assert!(!rendered.contains(unexpected), "found {unexpected:?}");
        }
    }
}
