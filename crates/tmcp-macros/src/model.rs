//! Data model for the macro expansion pipeline.
//!
//! These types capture everything the macros learn while parsing an annotated
//! impl block or attribute argument list, and everything code generation needs
//! to emit the expansion.

use std::ptr;

use syn::parse::Parse;

use crate::parse::{parse_ident_from_expr, parse_protocol_version_strategy};

#[derive(Debug)]
/// Description of an impl method tagged as a tool.
pub struct ToolMethod {
    /// Method identifier for the tool.
    pub ident: syn::Ident,
    /// Tool name advertised to clients (identifier without any raw prefix).
    pub name: String,
    /// Collected doc comments for the tool.
    pub docs: String,
    /// Whether the tool expects a ServerCtx parameter.
    pub has_ctx: bool,
    /// Whether the tool expects task metadata.
    pub task_param: TaskParamKind,
    /// Parameter type metadata for the tool method.
    pub params_kind: ParamsKind,
    /// Tool return type kind for call routing.
    pub return_kind: ToolReturnKind,
    /// Parsed tool attribute metadata.
    pub attrs: ToolAttrs,
}

#[derive(Debug)]
/// Description of an impl method tagged as a group factory.
pub struct GroupMethod {
    /// Method identifier for constructing the group.
    pub ident: syn::Ident,
    /// Optional segment override for this group edge.
    pub segment_override: Option<String>,
}

#[derive(Debug, Clone)]
/// A single flat parameter in a tool signature.
pub struct FlatParam {
    /// Identifier for the parameter.
    pub ident: syn::Ident,
    /// Type of the parameter.
    pub ty: syn::Type,
    /// Attributes to attach to the generated struct field.
    pub attrs: Vec<syn::Attribute>,
}

#[derive(Debug, Clone)]
/// Parameter shape for a tool method.
pub enum ParamsKind {
    /// Tool takes no parameters.
    None,
    /// Tool takes unit parameters (`()`).
    Unit,
    /// Tool takes a typed parameter struct.
    Typed(Box<syn::Type>),
    /// Tool takes flat parameters expanded from the signature.
    Flat(Vec<FlatParam>),
}

#[derive(Debug, Clone)]
/// Return type shape for a tool method.
pub enum ToolReturnKind {
    /// Tool returns `Result<CallToolResult>` (tmcp::Result).
    CallResult,
    /// Tool returns `Result<CreateTaskResult>` (tmcp::Result).
    TaskResult,
    /// Tool returns `Result<CallToolResponse>` (tmcp::Result).
    CallResponse,
    /// Tool returns `ToolResult`.
    ToolResult {
        /// Optional output type for `ToolResult<T>`.
        output: Box<Option<syn::Type>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Task metadata parameter shape for a tool method.
pub enum TaskParamKind {
    /// The method does not receive task metadata.
    None,
    /// The method receives `TaskMetadata` and requires task metadata.
    Required,
    /// The method receives `Option<TaskMetadata>`.
    Optional,
}

#[derive(Debug, Default, Clone)]
/// Metadata parsed from a #[tool(...)] attribute.
pub struct ToolAttrs {
    /// Optional display title for the tool.
    pub title: Option<String>,
    /// Whether the tool should be treated as read-only.
    pub read_only: Option<bool>,
    /// Whether the tool is destructive.
    pub destructive: Option<bool>,
    /// Whether the tool is idempotent.
    pub idempotent: Option<bool>,
    /// Whether the tool can access open-world resources.
    pub open_world: Option<bool>,
    /// Task support requirements for the tool.
    pub task_support: Option<ToolTaskSupport>,
    /// Optional output schema override.
    pub output_schema: Option<syn::Type>,
    /// Icon URLs for the tool.
    pub icons: Vec<String>,
    /// Whether to apply default argument handling.
    pub defaults: bool,
    /// Whether to force flat handling for single-argument tools.
    pub flat: bool,
    /// Whether the tool should always be visible when using ToolSet.
    pub always: bool,
}

#[derive(Debug, Default)]
/// Metadata parsed from a #[group(...)] attribute.
pub struct GroupMeta {
    /// Optional group name override.
    pub name: Option<String>,
    /// Optional description override.
    pub description: Option<String>,
    /// Optional deactivator visibility override.
    pub show_deactivator: Option<bool>,
    /// Optional activation hook method name.
    pub on_activate: Option<syn::Ident>,
    /// Optional deactivation hook method name.
    pub on_deactivate: Option<syn::Ident>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Whether task support is forbidden, optional, or required.
pub enum ToolTaskSupport {
    /// Task metadata must not be provided.
    Forbidden,
    /// Task metadata may be provided.
    Optional,
    /// Task metadata must be provided.
    Required,
}

#[derive(Debug)]
/// Summary of the server impl block and its tool methods.
pub struct ServerInfo {
    /// Self type of the annotated impl block.
    pub self_ty: syn::Type,
    /// Generics declared on the annotated impl block.
    pub generics: syn::Generics,
    /// Identifier of the server type (last path segment, without any raw prefix).
    pub struct_name: String,
    /// Doc comment used as the server description.
    pub description: String,
    /// Tool methods discovered in the impl block.
    pub tools: Vec<ToolMethod>,
    /// Group factory methods discovered in the impl block.
    pub groups: Vec<GroupMethod>,
}

/// Parameter shape accepted by a forwarder callback after `&self`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwarderParam {
    /// A `&ServerCtx` context reference, passed through as `context`.
    Ctx,
    /// A `String` payload such as a resource URI or task id.
    Str(&'static str),
    /// An `Option<Cursor>` pagination cursor.
    Cursor,
}

/// Capability advertisement implied by configuring a forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwarderCap {
    /// The forwarder implies no capability change.
    None,
    /// The forwarder implies the resources capability.
    Resources {
        /// Whether the forwarder also implies resource list-change notifications.
        list_changed: bool,
    },
    /// The forwarder implies the `tasks/list` capability.
    TasksList,
    /// The forwarder implies the `tasks/cancel` capability.
    TasksCancel,
}

/// Specification of one `*_fn` forwarder argument of `#[mcp_server]`.
///
/// A single table of these specs drives argument parsing, callback signature
/// validation, and trait method generation for all forwarders.
#[derive(Debug)]
pub struct ForwarderSpec {
    /// Keyword argument name accepted by `#[mcp_server(...)]`.
    pub arg: &'static str,
    /// `ServerHandler` trait method the forwarder implements.
    pub trait_method: &'static str,
    /// Parameter shapes after the `&self` receiver.
    pub params: &'static [ForwarderParam],
    /// `Result<T>` payload type in `tmcp::schema`, or `None` for `Result<()>`.
    pub payload: Option<&'static str>,
    /// Capability advertisement implied by configuring the forwarder.
    pub cap: ForwarderCap,
}

/// All `*_fn` forwarder arguments accepted by `#[mcp_server]`.
pub const FORWARDERS: &[ForwarderSpec] = &[
    ForwarderSpec {
        arg: "shutdown_fn",
        trait_method: "on_shutdown",
        params: &[],
        payload: None,
        cap: ForwarderCap::None,
    },
    ForwarderSpec {
        arg: "resources_fn",
        trait_method: "list_resources",
        params: &[ForwarderParam::Ctx, ForwarderParam::Cursor],
        payload: Some("ListResourcesResult"),
        cap: ForwarderCap::Resources { list_changed: true },
    },
    ForwarderSpec {
        arg: "read_resource_fn",
        trait_method: "read_resource",
        params: &[ForwarderParam::Ctx, ForwarderParam::Str("uri")],
        payload: Some("ReadResourceResult"),
        cap: ForwarderCap::Resources {
            list_changed: false,
        },
    },
    ForwarderSpec {
        arg: "resource_templates_fn",
        trait_method: "list_resource_templates",
        params: &[ForwarderParam::Ctx, ForwarderParam::Cursor],
        payload: Some("ListResourceTemplatesResult"),
        cap: ForwarderCap::Resources { list_changed: true },
    },
    ForwarderSpec {
        arg: "get_task_fn",
        trait_method: "get_task",
        params: &[ForwarderParam::Ctx, ForwarderParam::Str("task_id")],
        payload: Some("GetTaskResult"),
        cap: ForwarderCap::None,
    },
    ForwarderSpec {
        arg: "get_task_payload_fn",
        trait_method: "get_task_payload",
        params: &[ForwarderParam::Ctx, ForwarderParam::Str("task_id")],
        payload: Some("GetTaskPayloadResult"),
        cap: ForwarderCap::None,
    },
    ForwarderSpec {
        arg: "list_tasks_fn",
        trait_method: "list_tasks",
        params: &[ForwarderParam::Ctx, ForwarderParam::Cursor],
        payload: Some("ListTasksResult"),
        cap: ForwarderCap::TasksList,
    },
    ForwarderSpec {
        arg: "cancel_task_fn",
        trait_method: "cancel_task",
        params: &[ForwarderParam::Ctx, ForwarderParam::Str("task_id")],
        payload: Some("CancelTaskResult"),
        cap: ForwarderCap::TasksCancel,
    },
];

/// A forwarder argument bound to a callback method name.
#[derive(Debug)]
pub struct ForwarderBinding {
    /// The spec entry for the forwarder argument.
    pub spec: &'static ForwarderSpec,
    /// The callback method named in the macro arguments.
    pub fn_name: syn::Ident,
}

#[derive(Debug, Default)]
/// Parsed macro arguments for #[mcp_server].
pub struct ServerMacroArgs {
    /// Optional custom initialize function name.
    pub initialize_fn: Option<syn::Ident>,
    /// Configured `*_fn` forwarder callbacks in declaration order.
    pub forwarders: Vec<ForwarderBinding>,
    /// Optional server name override.
    pub name: Option<syn::Expr>,
    /// Optional server version override.
    pub version: Option<syn::Expr>,
    /// Optional instructions override.
    pub instructions: Option<syn::Expr>,
    /// Protocol version negotiation strategy.
    pub protocol_version: Option<ProtocolVersionStrategy>,
    /// Optional ToolSet field name for progressive discovery.
    pub toolset: Option<syn::Ident>,
}

impl Parse for ServerMacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;

            if let Some(spec) = FORWARDERS.iter().find(|spec| ident == spec.arg) {
                if args
                    .forwarders
                    .iter()
                    .any(|binding| ptr::eq::<ForwarderSpec>(binding.spec, spec))
                {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("duplicate argument: {ident}"),
                    ));
                }
                let fn_name: syn::Ident = input.parse()?;
                args.forwarders.push(ForwarderBinding { spec, fn_name });
            } else if ident == "initialize_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.initialize_fn = Some(fn_name);
            } else if ident == "name" {
                let expr: syn::Expr = input.parse()?;
                args.name = Some(expr);
            } else if ident == "version" {
                let expr: syn::Expr = input.parse()?;
                args.version = Some(expr);
            } else if ident == "instructions" {
                let expr: syn::Expr = input.parse()?;
                args.instructions = Some(expr);
            } else if ident == "protocol_version" {
                let expr: syn::Expr = input.parse()?;
                args.protocol_version = Some(parse_protocol_version_strategy(&expr)?);
            } else if ident == "toolset" {
                let expr: syn::Expr = input.parse()?;
                args.toolset = Some(parse_ident_from_expr(&expr)?);
            } else {
                return Err(syn::Error::new(
                    ident.span(),
                    format!("Unknown argument: {ident}"),
                ));
            }

            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
        }

        Ok(args)
    }
}

impl ServerMacroArgs {
    /// Return true if any resource protocol method should be generated.
    pub fn has_resource_callbacks(&self) -> bool {
        self.forwarders
            .iter()
            .any(|binding| matches!(binding.spec.cap, ForwarderCap::Resources { .. }))
    }

    /// Return true if resource listing capability should report list changes.
    pub fn resources_list_changed(&self) -> bool {
        self.forwarders.iter().any(|binding| {
            matches!(
                binding.spec.cap,
                ForwarderCap::Resources { list_changed: true }
            )
        })
    }

    /// Return true if the `tasks/list` capability should be advertised.
    pub fn tasks_list(&self) -> bool {
        self.forwarders
            .iter()
            .any(|binding| binding.spec.cap == ForwarderCap::TasksList)
    }

    /// Return true if the `tasks/cancel` capability should be advertised.
    pub fn tasks_cancel(&self) -> bool {
        self.forwarders
            .iter()
            .any(|binding| binding.spec.cap == ForwarderCap::TasksCancel)
    }
}

#[derive(Debug, Clone, Copy)]
/// Strategy for selecting the protocol version to use.
pub enum ProtocolVersionStrategy {
    /// Always use the latest supported protocol version.
    Latest,
    /// Use the client's requested protocol version.
    Client,
}
