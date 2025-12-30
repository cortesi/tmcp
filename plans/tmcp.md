# tmcp API simplification proposal

This proposal is grounded in the current tmcp implementation and how eguito uses it. I reviewed
`/Users/cortesi/git/public/tmcp/crates/tmcp-macros/src/lib.rs` to confirm the `#[tool]` signature
requirements, and `/Users/cortesi/git/public/tmcp/crates/tmcp/src/schema/tools.rs` to confirm the
current `CallToolResult` helpers. The concrete pain points come from
`/Users/cortesi/git/public/eguito/crates/eguito/src/mcp/mod.rs`.

## Observations from tmcp (ground truth)

The `#[tool]` macro currently requires an async method with exactly three parameters and a return
type of `Result<schema::CallToolResult>`. The macro also requires `arguments` to be present and
fails with `InvalidParams` when missing, regardless of the params type.

`CallToolResult` exposes builders for content, structured content, and marking as error, but there
is no first-class tool error type or convenience helper for returning structured errors.

These constraints drive a large amount of repetitive glue in eguito: empty params structs for tools
that take no args, `Ok(finish_tool(self.xxx_impl(params).await))` wrappers around every tool, and a
local `ToolCallError` type just to map errors into `CallToolResult` with `isError`.

## Proposal

### 1) Allow `#[tool]` methods to return a tool-specific result type

Add a new tmcp type, for example:

```rust
pub type ToolResult<T = CallToolResult> = Result<T, ToolError>;

pub struct ToolError {
    pub code: &'static str,
    pub message: String,
    pub structured: Option<serde_json::Value>,
}

impl From<ToolError> for CallToolResult {
    fn from(err: ToolError) -> CallToolResult { /* mark_as_error + structured */ }
}

impl<T: Into<CallToolResult>> From<T> for CallToolResult { /* existing path */ }
```

Then let the macro accept `ToolResult<T>` as a return type and auto-convert to
`CallToolResult` in the generated `call_tool` impl.

Before (eguito today):

```rust
#[tool]
async fn terminal_type(&self, _ctx: &ServerCtx, params: TerminalTypeParams)
    -> tmcp::Result<CallToolResult> {
    Ok(finish_tool(self.terminal_type_impl(params).await))
}
```

After (proposed):

```rust
#[tool]
async fn terminal_type(&self, _ctx: &ServerCtx, params: TerminalTypeParams)
    -> tmcp::ToolResult {
    self.terminal_type_impl(params).await
}
```

Impact: removes the local `ToolCallError`, `tool_ok`, and `finish_tool` helpers in eguito and
shrinks every tool wrapper to a single line. It also makes the error path explicit and typed.

Trade-off: expands the macro surface area and adds another public error type. There is a small
learning curve for new tmcp users, so docs and examples should show both the basic
`Result<CallToolResult>` path and the `ToolResult` path.

### 2) Support tools with no parameters (and `()` params) in the macro

Allow these signatures in `#[tool]`:

```rust
async fn terminal_state(&self, ctx: &ServerCtx) -> Result<CallToolResult>
async fn terminal_state(&self, ctx: &ServerCtx, _params: ()) -> Result<CallToolResult>
```

When `arguments` is `None`, the generated `call_tool` should pass `Arguments::new()` and deserialize
into `()`, or skip deserialization entirely for the two-argument form.

Before (eguito today):

```rust
#[derive(Debug, Deserialize, JsonSchema)]
struct TerminalStateParams {}

#[tool]
async fn terminal_state(&self, _ctx: &ServerCtx, _params: TerminalStateParams)
    -> tmcp::Result<CallToolResult> {
    Ok(finish_tool(self.terminal_state_impl().await))
}
```

After (proposed):

```rust
#[tool]
async fn terminal_state(&self, _ctx: &ServerCtx) -> tmcp::ToolResult {
    self.terminal_state_impl().await
}
```

Impact: removes empty params structs and the associated schema boilerplate for tools that take no
input.

Trade-off: the macro parser gets more complex and error messages for invalid signatures need to
stay clear. It also introduces a behavior change for missing arguments; existing users expecting
`InvalidParams` on `None` should keep using the three-argument form if they want that strictness.

### 3) Add a `CallToolResult::error` and `CallToolResult::structured` helper

Add convenience constructors:

```rust
impl CallToolResult {
    pub fn structured(content: impl Serialize) -> Result<Self, serde_json::Error> { /* ... */ }
    pub fn error(code: &'static str, message: impl Into<String>) -> Self { /* ... */ }
}
```

Before (eguito today):

```rust
fn tool_ok(payload: Value) -> CallToolResult {
    CallToolResult::new().with_structured_content(payload)
}

Err(ToolCallError::new("INVALID_INPUT", "bad input"))
```

After (proposed):

```rust
Ok(CallToolResult::structured(payload)?)
Err(ToolError::new("INVALID_INPUT", "bad input"))
```

Impact: reduces the local helper surface in eguito and makes simple tool success or failure paths
more self-descriptive.

Trade-off: more methods on `CallToolResult`, but they are narrow and aligned with existing patterns
(`with_structured_content`, `mark_as_error`).

### 4) Optional: `#[tool]` attribute to auto-fill defaults via serde

A small macro option like `#[tool(defaults)]` could allow `Arguments::new()` to be passed through
and rely on `#[serde(default)]` for missing fields. This would eliminate repeated
`unwrap_or(default)` logic inside tools when the defaults are already documented on the struct.

Before (eguito today):

```rust
let delay_ms = params.delay_ms.unwrap_or(0);
if delay_ms < 0 { return Err(ToolCallError::invalid("delay_ms must be >= 0")); }
```

After (proposed):

```rust
#[derive(Deserialize, JsonSchema)]
struct TerminalTypeParams {
    #[serde(default)]
    delay_ms: i64,
}

// delay_ms is already 0 if absent
```

Impact: encourages params structs to carry their own defaults, shrinking tool implementations.

Trade-off: this is more of a coding convention than a core tmcp feature, and it changes how missing
arguments are handled. It is also less broadly applicable than the other proposals.

## Suggested order of implementation

Start with the macro return type flexibility (Proposal 1) and no-params support (Proposal 2), since
those remove the most boilerplate in eguito. Add the `CallToolResult` helpers next. The default
argument support can be kept as an optional follow-on if you want to keep tmcp behavior strict by
default.
