# Flat tool argument lists

Add a natural, flat-parameter syntax for tool arguments while keeping the current
struct-based API. The macro should synthesize an argument struct and schema automatically and
route calls correctly.

## Status quo (observed)

- `#[mcp_server]` only accepts tool methods with 1-3 params: `&self`, optional `&ServerCtx`,
  and an optional params struct or `()`.
- Params structs must implement `serde::Deserialize` and `schemars::JsonSchema`; helpers
  `#[tool_params]` and `#[tool_result]` exist to add derives.
- `list_tools` uses `ToolSchema::from_json_schema::<Params>()` (empty schema for no params).
- `call_tool` deserializes `Arguments` (a JSON object map) into the params type and returns
  a tool error on missing or invalid input. `#[tool(defaults)]` treats missing arguments as
  an empty object.
- `Client::call_tool` always serializes arguments into an object (or `{}` for `()`).

## Goals

- Allow tool methods to declare “flat” arguments directly in the signature.
- Keep the existing struct-based params approach intact and preferred for complex cases.
- Generate tool schema and deserialization from flat params without extra user boilerplate.
- Preserve error handling semantics and `#[tool(defaults)]` behavior.

## Proposed API

### Before (current)

```rust
#[derive(Debug)]
#[tool_params]
struct AddParams {
    a: f64,
    b: f64,
}

#[tool]
async fn add(&self, params: AddParams) -> ToolResult<AddResponse> {
    Ok(AddResponse { sum: params.a + params.b })
}
```

### After (flat args)

```rust
#[tool]
async fn add(&self, a: f64, b: f64) -> ToolResult<AddResponse> {
    Ok(AddResponse { sum: a + b })
}
```

Single-argument flat (opt-in when ambiguous):

```rust
#[tool(flat)]
async fn echo(&self, message: String) -> ToolResult<EchoResponse> {
    Ok(EchoResponse { message })
}
```

With context and optional args:

```rust
#[tool]
async fn log_message(
    &self,
    ctx: &ServerCtx,
    message: String,
    level: Option<String>,
) -> ToolResult<LogResponse> {
    // ...
}
```

## Macro behavior (design)

### Detecting flat params

- If there are 2+ non-`ServerCtx` parameters, treat them as flat arguments.
- If there is exactly 1 non-`ServerCtx` parameter, keep current behavior by default.
  Use `#[tool(flat)]` to force flat handling for single-argument tools.
- `()` remains supported for unit params; no-arg tools remain unchanged.
- No single-arg heuristics: the macro cannot type-check parameters, so single-argument flat
  tools are always explicit via `#[tool(flat)]`.

### Generated struct (conceptual)

For the `add` example, macro expansion should include a hidden struct like:

```rust
#[derive(serde::Deserialize, schemars::JsonSchema)]
struct __TmcpToolArgs_TestServer_add {
    a: f64,
    b: f64,
}
```

`list_tools` uses `ToolSchema::from_json_schema::<__TmcpToolArgs_...>()`, and `call_tool`
uses `Arguments` to deserialize into the struct, then calls the method with fields.
The generated struct is private, `#[doc(hidden)]`, and annotated with
`#[allow(non_camel_case_types)]` to avoid lint noise.

### Parameter mapping rules

- JSON argument keys use the Rust parameter identifiers verbatim (e.g., `foo_bar`).
- Only simple identifier patterns are allowed for flat params (`arg: Type`). Destructuring
  patterns produce a compile-time macro error.
- Parameter attributes (`serde`, `schemars`, and doc comments) are copied onto the generated
  struct fields to support per-arg docs and renames.

### Error handling

- Missing or invalid fields result in the same tool error behavior as today.
- `#[tool(defaults)]` keeps its current meaning: missing `arguments` becomes `{}` so optional
  fields and serde defaults can apply.

## Compatibility

- Existing struct-based signatures continue to work unchanged.
- Multi-parameter tool methods that were previously rejected are now supported as flat tools.
- Single-parameter tools remain struct-based unless `#[tool(flat)]` is set.
- Tool schemas for existing tools remain stable unless the signature is explicitly changed to
  a flat form; no automatic behavior change for 1-arg tools.
- Client usage stays the same: `call_tool` still serializes to an object, so flat args map
  directly to the existing JSON object shape by key.

## Documentation updates (planned)

- Update macro crate docs to list flat-arg signatures and the `flat` flag.
- Update README/examples to show at least one flat-arg tool.

## Execution plan

1. Stage One: Finalize flat-arg API surface

Confirm open questions and lock down the flat-arg detection rules and attribute support.

1. [x] Resolve doc-interview questions in this plan file and update the design accordingly.
2. [x] Confirm any naming/compatibility constraints for generated arg structs.

2. Stage Two: Macro changes (tmcp-macros)

Extend `#[mcp_server]` parsing and code generation to synthesize flat-arg structs and routing.

1. [x] Update tool parsing to support multiple non-ctx params and `#[tool(flat)]`.
2. [x] Generate hidden arg structs for flat tools, including attribute passthrough.
3. [x] Update `list_tools` generation to use the synthetic structs for flat tools.
4. [x] Update `call_tool` generation to deserialize args and bind field vars correctly.
5. [x] Add/extend macro tests covering flat args, single-arg flat, and error cases.

3. Stage Three: Supporting library changes (tmcp)

Add helper APIs to keep generated code small and consistent.

1. [x] Add a helper on `Arguments` to deserialize with consistent invalid-input errors (and
   optional defaults), then use it in macro output.
2. [x] Add/adjust unit tests for any new `Arguments` helpers.

4. Stage Four: Documentation and examples

Update docs to reflect flat-arg support and ensure examples compile.

1. [x] Update macro crate docs and README tool examples with flat-arg signatures.
2. [x] Add or update an example to use flat args in `examples/*`.

5. Stage Five: Verification

1. [x] Run `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests
   --examples`.
2. [x] Run `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml`.
3. [x] Run `cargo nextest run --all --all-features` (fallback to `cargo test`).
