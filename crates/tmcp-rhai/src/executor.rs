use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use rhai::{Dynamic, Engine, Scope};
use serde_json::Value;

use crate::{
    config::ScriptConfig,
    engine::build_engine,
    error::{ScriptError, ScriptEvalOutcome, format_error},
};

/// API surface that a host exposes to Rhai scripts.
pub trait ScriptApi: Send + Sync + 'static {
    /// Register functions, types, and modules on the engine.
    fn register(&self, engine: &mut Engine);

    /// Callback invoked when the script prints.
    fn on_print(&self, _text: &str) {}
    /// Callback invoked when the script emits debug output.
    fn on_debug(&self, _text: &str, _source: Option<&str>, _pos: rhai::Position) {}
}

/// Executes Rhai scripts using a provided API and configuration.
pub struct RhaiScriptExecutor<A> {
    api: Arc<A>,
    config: ScriptConfig,
}

impl<A: ScriptApi> RhaiScriptExecutor<A> {
    /// Create a new script executor with the provided API and configuration.
    pub fn new(api: Arc<A>, config: ScriptConfig) -> Self {
        Self { api, config }
    }

    /// Execute a script with optional JSON arguments.
    pub fn execute(&self, script: &str, args: Option<Value>) -> ScriptEvalOutcome {
        let mut engine = build_engine(&self.config);

        // Register API
        self.api.register(&mut engine);

        // Setup Logging
        let logs = Arc::new(Mutex::new(Vec::new()));
        let logs_c = logs.clone();
        let api_print = self.api.clone();
        engine.on_print(move |text| {
            if let Ok(mut l) = logs_c.lock() {
                l.push(text.to_string());
            }
            api_print.on_print(text);
        });

        let api_debug = self.api.clone();
        engine.on_debug(move |text, src, pos| api_debug.on_debug(text, src, pos));

        // Setup Timeout
        let start = Instant::now();
        let timeout = self.config.timeout;
        let timeout_ms = timeout.as_millis() as u64;

        engine.on_progress(move |_| {
            if start.elapsed() > timeout {
                Some(Dynamic::from(ScriptError::Timeout { ms: timeout_ms }))
            } else {
                None
            }
        });

        // Setup Scope
        let mut scope = Scope::new();
        if let Some(a) = args {
            match rhai::serde::to_dynamic(a) {
                Ok(val) => {
                    scope.push("args", val);
                }
                Err(e) => {
                    return ScriptEvalOutcome {
                        value: None,
                        logs: vec![],
                        error: Some(format_error(&ScriptError::Custom(format!(
                            "Failed to serialize args: {}",
                            e
                        )))),
                    };
                }
            }
        }

        // Parse & Eval
        let result = engine.eval_with_scope::<Dynamic>(&mut scope, script);

        let captured_logs = logs.lock().unwrap_or_else(|e| e.into_inner()).clone();

        match result {
            Ok(val) => {
                let json_val = rhai::serde::from_dynamic(&val).unwrap_or(Value::Null);
                ScriptEvalOutcome {
                    value: Some(json_val),
                    logs: captured_logs,
                    error: None,
                }
            }
            Err(e) => {
                // Check if it was our timeout wrapped in runtime error
                // We can't easily unwrap the inner error without consuming the Box,
                // and try_cast works on the inner dynamic.

                // If the error is a runtime error containing a custom type, try to extract it
                // Note: rhai::EvalAltResult is complex to destructure for custom types cleanly
                // without ownership issues sometimes, but let's try strict matching if possible.
                // For now, we wrap the whole thing.

                // If the user threw a custom exception map, it might be here.

                // Special check for timeout logic (usually returns ErrorRuntime(Dynamic))
                // But we used Dynamic::from(ScriptError), so we check if that dynamic is ScriptError
                let final_err = if let rhai::EvalAltResult::ErrorRuntime(d, _) = e.as_ref() {
                    if let Some(script_err) = d.clone().try_cast::<ScriptError>() {
                        script_err
                    } else {
                        ScriptError::Runtime(Arc::from(e))
                    }
                } else {
                    ScriptError::Runtime(Arc::from(e))
                };

                ScriptEvalOutcome {
                    value: None,
                    logs: captured_logs,
                    error: Some(format_error(&final_err)),
                }
            }
        }
    }
}
