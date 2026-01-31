use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rhai::{Dynamic, Engine, EvalAltResult, ParseError, Position, Scope};
use serde_json::Value;

use crate::{
    config::ScriptConfig,
    engine::build_engine,
    error::{
        ScriptAssertion, ScriptError, ScriptErrorInfo, ScriptEvalOutcome, ScriptInfoResult,
        ScriptTiming, ScriptValue, format_error,
    },
};

/// Options for executing a script.
#[derive(Debug, Clone, Default)]
pub struct ScriptExecutionOptions {
    /// Optional JSON arguments to pass to the script.
    pub args: Option<Value>,
    /// Optional source name for diagnostics.
    pub source_name: Option<String>,
    /// Optional timeout override for this evaluation.
    pub timeout: Option<Duration>,
}

/// API surface that a host exposes to Rhai scripts.
pub trait ScriptApi: Send + Sync + 'static {
    /// Register functions, types, and modules on the engine.
    fn register(&self, engine: &mut Engine);

    /// Callback invoked when the script prints.
    fn on_print(&self, _text: &str) {}
    /// Callback invoked when the script emits debug output.
    fn on_debug(&self, _text: &str, _source: Option<&str>, _pos: Position) {}

    /// Convert JSON arguments into a dynamic value.
    fn to_dynamic(&self, value: Value) -> ScriptInfoResult<Dynamic> {
        let error = |err| {
            Box::new(ScriptErrorInfo {
                error_type: "type_error".to_string(),
                message: format!("Failed to serialize args: {err}"),
                location: None,
                backtrace: None,
                code: None,
                details: None,
            })
        };
        rhai::serde::to_dynamic(value).map_err(error)
    }

    /// Convert a script value into a structured response.
    fn map_value(&self, value: &Dynamic) -> ScriptInfoResult<ScriptValue> {
        if value.is_unit() {
            return Ok(ScriptValue::default());
        }
        let json_value = rhai::serde::from_dynamic(value).map_err(|err| {
            Box::new(ScriptErrorInfo {
                error_type: "type_error".to_string(),
                message: format!("Failed to serialize value: {err}"),
                location: None,
                backtrace: None,
                code: None,
                details: None,
            })
        })?;
        Ok(ScriptValue {
            value: Some(json_value),
            ..ScriptValue::default()
        })
    }

    /// Map parse errors to structured error information.
    fn map_parse_error(&self, _error: &ParseError) -> Option<ScriptErrorInfo> {
        None
    }

    /// Map evaluation errors to structured error information.
    fn map_eval_error(&self, _error: &EvalAltResult) -> Option<ScriptErrorInfo> {
        None
    }

    /// Map timeout events to structured error information.
    fn map_timeout(&self, _timeout_ms: u64, _pos: Position) -> Option<ScriptErrorInfo> {
        None
    }

    /// Provide collected logs instead of the captured `print` output.
    fn collect_logs(&self) -> Option<Vec<String>> {
        None
    }

    /// Provide collected assertions for the outcome.
    fn collect_assertions(&self) -> Vec<ScriptAssertion> {
        Vec::new()
    }
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
        self.execute_with_options(
            script,
            ScriptExecutionOptions {
                args,
                ..ScriptExecutionOptions::default()
            },
        )
    }

    /// Execute a script with detailed options.
    pub fn execute_with_options(
        &self,
        script: &str,
        options: ScriptExecutionOptions,
    ) -> ScriptEvalOutcome {
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
        let timeout = options.timeout.unwrap_or(self.config.timeout);
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
        if let Some(a) = options.args {
            match self.api.to_dynamic(a) {
                Ok(val) => {
                    scope.push("args", val);
                }
                Err(info) => {
                    return self.build_error_outcome(&logs, *info, ScriptTiming::zero());
                }
            }
        }

        // Parse & Eval
        let compile_start = Instant::now();
        let mut ast = match engine.compile_with_scope(&scope, script) {
            Ok(ast) => ast,
            Err(err) => {
                let timing = ScriptTiming {
                    compile_ms: compile_start.elapsed().as_millis() as u64,
                    exec_ms: 0,
                    total_ms: start.elapsed().as_millis() as u64,
                };
                let info = self
                    .api
                    .map_parse_error(&err)
                    .unwrap_or_else(|| format_error(&ScriptError::Parse(err)));
                return self.build_error_outcome(&logs, info, timing);
            }
        };
        let compile_ms = compile_start.elapsed().as_millis() as u64;

        if let Some(source_name) = options.source_name {
            ast.set_source(source_name);
        }

        if start.elapsed() >= timeout {
            let timing = ScriptTiming {
                compile_ms,
                exec_ms: 0,
                total_ms: start.elapsed().as_millis() as u64,
            };
            let info = self
                .api
                .map_timeout(timeout_ms, Position::NONE)
                .unwrap_or_else(|| format_error(&ScriptError::Timeout { ms: timeout_ms }));
            return self.build_error_outcome(&logs, info, timing);
        }

        let exec_start = Instant::now();
        let result = engine.eval_ast_with_scope::<Dynamic>(&mut scope, &ast);
        let exec_ms = exec_start.elapsed().as_millis() as u64;
        let total_ms = start.elapsed().as_millis() as u64;
        let timing = ScriptTiming {
            compile_ms,
            exec_ms,
            total_ms,
        };

        match result {
            Ok(val) => match self.api.map_value(&val) {
                Ok(script_value) => self.build_success_outcome(&logs, script_value, timing),
                Err(info) => self.build_error_outcome(&logs, *info, timing),
            },
            Err(e) => {
                let info = if let Some(mapped) = self.api.map_eval_error(&e) {
                    mapped
                } else {
                    let final_err = if let EvalAltResult::ErrorRuntime(d, _) = e.as_ref() {
                        if let Some(script_err) = d.clone().try_cast::<ScriptError>() {
                            script_err
                        } else {
                            ScriptError::Runtime(Arc::from(e))
                        }
                    } else {
                        ScriptError::Runtime(Arc::from(e))
                    };
                    format_error(&final_err)
                };
                self.build_error_outcome(&logs, info, timing)
            }
        }
    }

    fn build_success_outcome(
        &self,
        logs: &Arc<Mutex<Vec<String>>>,
        script_value: ScriptValue,
        timing: ScriptTiming,
    ) -> ScriptEvalOutcome {
        let logs = self.collect_logs(logs);
        let assertions = self.api.collect_assertions();
        ScriptEvalOutcome {
            success: true,
            value: script_value.value,
            images: script_value.images,
            logs,
            assertions,
            timing,
            error: None,
            content: script_value.content,
        }
    }

    fn build_error_outcome(
        &self,
        logs: &Arc<Mutex<Vec<String>>>,
        info: ScriptErrorInfo,
        timing: ScriptTiming,
    ) -> ScriptEvalOutcome {
        let logs = self.collect_logs(logs);
        let assertions = self.api.collect_assertions();
        ScriptEvalOutcome {
            success: false,
            value: None,
            images: None,
            logs,
            assertions,
            timing,
            error: Some(info),
            content: Vec::new(),
        }
    }

    fn collect_logs(&self, logs: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        if let Some(logs) = self.api.collect_logs() {
            return logs;
        }
        logs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}
