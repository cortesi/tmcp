# tmcp ergonomics + correctness execution plan

This plan stages the recommendations from `plans/ergo.md` into coherent, safe changesets that
improve initialization correctness, capability alignment, and day-to-day ergonomics across the
client/server APIs and examples.

1. Stage One: Initialization correctness and lifecycle ordering

Make the connect/init sequence unambiguous, and ensure lifecycle hooks run in protocol order.

1. [x] Decide and implement a single connect/init contract in `crates/tmcp/src/client.rs`
       (auto-init everywhere vs explicit `*_raw` + `*_and_init`), and update public docs.
2. [x] Reorder lifecycle callbacks so server/client notifications cannot precede `initialize`
       completion (adjust `crates/tmcp/src/client.rs` and `crates/tmcp/src/connection.rs`).
3. [x] Update all examples and README snippets to match the new connect/init flow
       (`examples/*.rs`, `README.md`).

2. Stage Two: Capability alignment and metadata accuracy

Make advertised capabilities match runtime behavior and macro defaults reflect actual tooling.

1. [ ] Plumb `Server` capability configuration into `initialize` responses so runtime behavior
       and handshake metadata agree (`crates/tmcp/src/server.rs`, `crates/tmcp-macros/src/lib.rs`).
2. [ ] Fix macro defaults to use `env!("CARGO_PKG_VERSION")`, set tools capability correctly
       when tools exist, and avoid empty descriptions (`crates/tmcp-macros/src/lib.rs`).
3. [ ] Add coverage tests for the updated initialize/capabilities behavior
       (`crates/tmcp-macros/tests/*`, `crates/tmcp/tests/*`).

3. Stage Three: API ergonomics cleanup

Reduce friction in common call sites and standardize naming.

1. [ ] Provide a small `tmcp::prelude` or inherent wrappers so clients do not need to import
       `ServerAPI` just to call `list_tools`/`call_tool` (`crates/tmcp/src/lib.rs`).
2. [ ] Unify notification method naming between `ClientCtx` and `ServerCtx`
       (`crates/tmcp/src/context.rs`) and update examples.
3. [ ] Make missing server handlers fail fast (constructor or typestate), avoiding late runtime
       errors (`crates/tmcp/src/server.rs`).

4. Stage Four: Schema fidelity and error consistency

Improve correctness of tool schemas and error mapping to JSON-RPC.

1. [ ] Standardize tool-not-found vs execution errors and map them to JSON-RPC error codes
       consistently (`crates/tmcp/src/error.rs`, `crates/tmcp/src/connection.rs`,
       `crates/tmcp-macros/src/lib.rs`).
2. [ ] Preserve full JSON Schema in tool metadata (new `ToolSchema` representation or
       `from_value` path) and update macro generation/tests (`crates/tmcp/src/schema/tools.rs`,
       `crates/tmcp-macros/src/lib.rs`).
3. [ ] Deduplicate protocol version constants by deriving HTTP header values from
       `schema::LATEST_PROTOCOL_VERSION` (`crates/tmcp/src/http.rs`).

5. Stage Five: Quality-of-life improvements

Add small helpers that reduce boilerplate in examples and real usage.

1. [ ] Add typed tool-call helpers (e.g., `call_tool_typed`) and update examples to use them
       (`crates/tmcp/src/client.rs`, `examples/*`).
2. [ ] Provide `ServerHandle` for TCP so callers can stop servers gracefully
       (`crates/tmcp/src/server.rs`).
3. [ ] Add a server-initiated request example using `ServerCtx` + `ClientAPI`
       (`examples/*`).
