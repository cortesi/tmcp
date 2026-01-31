use std::time::Duration;

/// Limits and configuration for script execution.
#[derive(Debug, Clone)]
pub struct ScriptConfig {
    /// Maximum wall-clock duration for a script run.
    pub timeout: Duration,
    /// Maximum number of operations the engine may perform.
    pub max_operations: u64,
    /// Maximum call stack depth.
    pub max_call_levels: usize,
    /// Maximum expression nesting depth.
    pub max_expr_depth: usize,
    /// Maximum depth for function expressions.
    pub max_function_expr_depth: usize,
    /// Maximum size of any string value.
    pub max_string_size: usize,
    /// Maximum size of any array.
    pub max_array_size: usize,
    /// Maximum size of any map.
    pub max_map_size: usize,
    /// Maximum number of variables in scope.
    pub max_variables: usize,
    /// Maximum number of functions allowed.
    pub max_functions: usize,
    /// Maximum number of modules that can be loaded.
    pub max_modules: usize,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(60),
            max_operations: 1_000_000,
            max_call_levels: 64,
            max_expr_depth: 64,
            max_function_expr_depth: 32,
            max_string_size: 1_000_000,
            max_array_size: 100_000,
            max_map_size: 100_000,
            max_variables: 10_000,
            max_functions: 1_000,
            max_modules: 10,
        }
    }
}
