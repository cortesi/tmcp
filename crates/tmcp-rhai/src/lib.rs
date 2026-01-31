#![warn(missing_docs)]

//! Rhai scripting integration for TMCP (Transport Model for Context Protocol).
//!
//! This crate provides a small, consistent surface area for executing Rhai
//! scripts with TMCP-aware APIs and structured error reporting.

mod config;
mod engine;
mod error;
mod executor;

pub use config::ScriptConfig;
pub use error::{
    ScriptAssertion, ScriptError, ScriptErrorInfo, ScriptEvalOutcome, ScriptImageInfo,
    ScriptInfoResult, ScriptLocation, ScriptResult, ScriptTiming, ScriptValue,
};
pub use executor::{RhaiScriptExecutor, ScriptApi, ScriptExecutionOptions};
