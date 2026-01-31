use rhai::{
    Engine,
    default_limits::MAX_STRINGS_INTERNED,
    packages::{Package, StandardPackage},
};

use crate::config::ScriptConfig;

pub(crate) fn build_engine(config: &ScriptConfig) -> Engine {
    let mut engine = Engine::new_raw();
    engine.register_global_module(StandardPackage::new().as_shared_module());

    engine.set_max_strings_interned(MAX_STRINGS_INTERNED);
    engine.set_strict_variables(true);
    engine.set_fail_on_invalid_map_property(true);

    engine.set_max_operations(config.max_operations);
    engine.set_max_call_levels(config.max_call_levels);
    engine.set_max_expr_depths(config.max_expr_depth, config.max_function_expr_depth);
    engine.set_max_string_size(config.max_string_size);
    engine.set_max_array_size(config.max_array_size);
    engine.set_max_map_size(config.max_map_size);
    engine.set_max_variables(config.max_variables);
    engine.set_max_functions(config.max_functions);
    engine.set_max_modules(config.max_modules);

    engine
}
