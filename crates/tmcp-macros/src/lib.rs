//! Macros to make defining MCP servers easier
//!
//! Using the #[mcp_server] macro on an impl block, this crate will pick up all methods
//! marked with #[tool] and derive the necessary ServerHandler::call_tool, ServerHandler::list_tools and
//! ServerHandler::initialize methods. The name of the server is derived from the name of the struct
//! converted to snake_case (e.g., MyServer becomes my_server), and the description is derived
//! from the doc comment on the impl block. The version is set to "0.1.0" by default.
//!
//! The macro supports customization through attributes:
//! - `initialize_fn`: Specify a custom initialize function instead of using the default
//!
//! All tool methods have the following exact signature:  
//!
//! ```ignore
//! async fn tool_name(&self, context: &ServerCtx, params: ToolParams) -> Result<schema::CallToolResult>
//! ```
//!
//! The parameter struct (ToolParams in this example) must implement `schemars::JsonSchema` and
//! `serde::Deserialize`.
//!
//! Example usage:
//!
//! ```ignore
//! use tmcp::{ServerCtx, schema};
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
//! struct EchoParams {
//!     /// The message to echo back
//!     message: String,
//! }
//!
//! /// Basic server connection that provides an echo tool
//! #[derive(Debug, Default)]
//! struct Basic {}
//!
//! #[mcp_server]
//! /// This is the description field for the server.
//! impl Basic {
//!     #[tool]
//!     async fn echo(
//!         &self,
//!         context: &ServerCtx,
//!         params: EchoParams,
//!     ) -> Result<schema::CallToolResult> {
//!         Ok(schema::CallToolResult::new().with_text_content(params.message))
//!     }
//! }
//! ```
//!
//! Example with custom initialize function:
//!
//! ```ignore
//! #[mcp_server(initialize_fn = my_custom_initialize)]
//! impl MyServer {
//!     async fn my_custom_initialize(
//!         &self,
//!         context: &ServerCtx,
//!         protocol_version: String,
//!         capabilities: schema::ClientCapabilities,
//!         client_info: schema::Implementation,
//!     ) -> Result<schema::InitializeResult> {
//!         // Custom initialization logic
//!         Ok(schema::InitializeResult {
//!             protocol_version: schema::LATEST_PROTOCOL_VERSION.to_string(),
//!             capabilities: schema::ServerCapabilities {
//!                 tools: Some(schema::ToolsCapability {
//!                     list_changed: Some(true),
//!                 }),
//!                 ..Default::default()
//!             },
//!             server_info: schema::Implementation::new("my_custom_server", "2.0.0"),
//!             },
//!             instructions: Some("Custom server with advanced features".to_string()),
//!             meta: None,
//!         })
//!     }
//!
//!     #[tool]
//!     async fn my_tool(&self, context: &ServerCtx, params: MyParams) -> Result<schema::CallToolResult> {
//!         // Tool implementation
//!     }
//! }
//! ```

use std::result::Result as StdResult;

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ExprLit, ImplItem, ItemImpl, Lit, Meta, parse::Parse, spanned::Spanned};

/// Internal result type for macro parsing.
type Result<T> = StdResult<T, syn::Error>;

#[derive(Debug)]
/// Description of an impl method tagged as a tool.
struct ToolMethod {
    /// Name of the tool method.
    name: String,
    /// Collected doc comments for the tool.
    docs: String,
    /// Parameter type of the tool method.
    params_type: syn::Type,
}

#[derive(Debug)]
/// Summary of the server impl block and its tool methods.
struct ServerInfo {
    /// Name of the server struct.
    struct_name: String,
    /// Doc comment used as the server description.
    description: String,
    /// Tool methods discovered in the impl block.
    tools: Vec<ToolMethod>,
}

#[derive(Debug, Default)]
/// Parsed macro arguments for #[mcp_server].
struct ServerMacroArgs {
    /// Optional custom initialize function name.
    initialize_fn: Option<syn::Ident>,
}

impl Parse for ServerMacroArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = Self::default();

        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;

            if ident == "initialize_fn" {
                let fn_name: syn::Ident = input.parse()?;
                args.initialize_fn = Some(fn_name);
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

/// Collect and normalize doc comment strings from attributes.
fn extract_doc_comment(attrs: &[syn::Attribute]) -> String {
    let mut docs = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("doc")
            && let Meta::NameValue(meta) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &meta.value
        {
            let doc = s.value();
            let doc = doc.trim();
            if !doc.is_empty() {
                docs.push(doc.to_string());
            }
        }
    }
    docs.join("\n")
}

/// Parse a tool method from an impl item if it has a #[tool] attribute.
fn parse_tool_method(method: &syn::ImplItemFn) -> Result<Option<ToolMethod>> {
    let has_tool_attr = method.attrs.iter().any(|attr| attr.path().is_ident("tool"));

    if !has_tool_attr {
        return Ok(None);
    }

    let name = method.sig.ident.to_string();
    let docs = extract_doc_comment(&method.attrs);

    // Validate method signature
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            "tool methods must be async",
        ));
    }

    // Check parameters
    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.len() != 3 {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            "tool methods must have exactly 3 parameters: &self, context: &ServerCtx, params: ParamsType",
        ));
    }

    // Validate &self or &mut self
    match params[0] {
        syn::FnArg::Receiver(_r) => {
            // Accept both &self and &mut self
        }
        _ => {
            return Err(syn::Error::new(
                params[0].span(),
                "first parameter must be &self or &mut self",
            ));
        }
    }

    // Extract params type (third parameter)
    let params_type = match params[2] {
        syn::FnArg::Typed(pat_type) => (*pat_type.ty).clone(),
        _ => {
            return Err(syn::Error::new(
                params[2].span(),
                "third parameter must be a typed parameter",
            ));
        }
    };

    // Validate return type
    match &method.sig.output {
        syn::ReturnType::Type(_, _ty) => {
            // We just check it exists, actual validation would be more complex
        }
        _ => {
            return Err(syn::Error::new(
                method.sig.output.span(),
                "tool methods must return Result<schema::CallToolResult>",
            ));
        }
    }

    Ok(Some(ToolMethod {
        name,
        docs,
        params_type,
    }))
}

/// Parse a server impl block and gather tool metadata.
fn parse_impl_block(input: &TokenStream) -> Result<(ItemImpl, ServerInfo)> {
    let impl_block = syn::parse2::<ItemImpl>(input.clone())?;

    // Extract struct name
    let struct_name = match &*impl_block.self_ty {
        syn::Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .ok_or_else(|| syn::Error::new(impl_block.self_ty.span(), "Invalid type name"))?
            .ident
            .to_string(),
        _ => {
            return Err(syn::Error::new(
                impl_block.self_ty.span(),
                "Expected a struct or type name",
            ));
        }
    };

    // Extract description from doc comment
    let description = extract_doc_comment(&impl_block.attrs);

    // Extract tool methods
    let mut tools = Vec::new();
    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item
            && let Some(tool) = parse_tool_method(method)?
        {
            tools.push(tool);
        }
    }

    Ok((
        impl_block,
        ServerInfo {
            struct_name,
            description,
            tools,
        },
    ))
}

/// Generate the ServerHandler::call_tool implementation.
fn generate_call_tool(info: &ServerInfo) -> TokenStream {
    let tool_matches = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let method = syn::Ident::new(name, proc_macro2::Span::call_site());
        let params_type = &tool.params_type;

        quote! {
            #name => {
                let args = arguments.ok_or_else(||
                    tmcp::Error::InvalidParams("Missing arguments".to_string())
                )?;
                let params: #params_type = args.deserialize()
                    .map_err(|e| tmcp::Error::InvalidParams(e.to_string()))?;
                self.#method(context, params).await
            }
        }
    });

    quote! {
        async fn call_tool(
            &self,
            context: &tmcp::ServerCtx,
            name: String,
            arguments: Option<tmcp::Arguments>,
            _task: Option<tmcp::schema::TaskMetadata>,
        ) -> tmcp::Result<tmcp::schema::CallToolResult> {
            match name.as_str() {
                #(#tool_matches)*
                _ => Err(tmcp::Error::MethodNotFound(format!("Unknown tool: {}", name)))
            }
        }
    }
}

/// Generate the ServerHandler::list_tools implementation.
fn generate_list_tools(info: &ServerInfo) -> TokenStream {
    let tools = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let description = &tool.docs;
        let params_type = &tool.params_type;

        quote! {
            {
                tmcp::schema::Tool::new(#name, tmcp::schema::ToolSchema::from_json_schema::<#params_type>())
                    .with_description(#description)
            }
        }
    });

    quote! {
        async fn list_tools(
            &self,
            _context: &tmcp::ServerCtx,
            _cursor: Option<tmcp::schema::Cursor>,
        ) -> tmcp::Result<tmcp::schema::ListToolsResult> {
            Ok(tmcp::schema::ListToolsResult {
                tools: vec![#(#tools),*],
                next_cursor: None,
            })
        }
    }
}

/// Generate the ServerHandler::initialize implementation.
fn generate_initialize(info: &ServerInfo, custom_init_fn: Option<&syn::Ident>) -> TokenStream {
    if let Some(init_fn) = custom_init_fn {
        // Use the custom initialize function
        quote! {
            async fn initialize(
                &self,
                context: &tmcp::ServerCtx,
                protocol_version: String,
                capabilities: tmcp::schema::ClientCapabilities,
                client_info: tmcp::schema::Implementation,
            ) -> tmcp::Result<tmcp::schema::InitializeResult> {
                self.#init_fn(context, protocol_version, capabilities, client_info).await
            }
        }
    } else {
        // Use the default implementation
        generate_default_initialize(info)
    }
}

/// Generate the default ServerHandler::initialize implementation.
fn generate_default_initialize(info: &ServerInfo) -> TokenStream {
    let snake_case_name = info.struct_name.to_snake_case();
    let description = &info.description;

    let initialize_result = if description.is_empty() {
        quote! {
            tmcp::schema::InitializeResult::new(#snake_case_name)
                .with_version("0.1.0")
                .with_tools(false)
        }
    } else {
        quote! {
            tmcp::schema::InitializeResult::new(#snake_case_name)
                .with_version("0.1.0")
                .with_tools(false)
                .with_instructions(#description)
        }
    };

    quote! {
        async fn initialize(
            &self,
            _context: &tmcp::ServerCtx,
            _protocol_version: String,
            _capabilities: tmcp::schema::ClientCapabilities,
            _client_info: tmcp::schema::Implementation,
        ) -> tmcp::Result<tmcp::schema::InitializeResult> {
            Ok(#initialize_result)
        }
    }
}

/// Validate the signature of a custom initialize function.
fn validate_custom_initialize_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    // Find the method in the impl block
    let method = impl_block.items.iter().find_map(|item| {
        if let ImplItem::Fn(method) = item {
            if method.sig.ident == *fn_name {
                Some(method)
            } else {
                None
            }
        } else {
            None
        }
    });

    let method = method.ok_or_else(|| {
        syn::Error::new(
            fn_name.span(),
            format!("Custom initialize function '{fn_name}' not found in impl block"),
        )
    })?;

    // Validate it's async
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            "Custom initialize function must be async",
        ));
    }

    // Validate parameters
    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.len() != 5 {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            "Custom initialize function must have exactly 5 parameters: &self, context: &ServerCtx, protocol_version: String, capabilities: ClientCapabilities, client_info: Implementation",
        ));
    }

    // Validate &self
    match params[0] {
        syn::FnArg::Receiver(_) => {}
        _ => {
            return Err(syn::Error::new(
                params[0].span(),
                "First parameter must be &self",
            ));
        }
    }

    // Validate return type exists
    match &method.sig.output {
        syn::ReturnType::Type(_, _) => {
            // We just check it exists, full type validation would be complex
        }
        _ => {
            return Err(syn::Error::new(
                method.sig.output.span(),
                "Custom initialize function must return Result<InitializeResult>",
            ));
        }
    }

    Ok(())
}

/// Parse the #[mcp_server] macro inputs and emit the expanded tokens.
fn inner_mcp_server(attr: TokenStream, input: &TokenStream) -> Result<TokenStream> {
    // Parse macro attributes
    let args = syn::parse2::<ServerMacroArgs>(attr).unwrap_or_default();
    let (impl_block, info) = parse_impl_block(input)?;

    if info.tools.is_empty() {
        return Err(syn::Error::new(
            input.span(),
            "No tool methods found. Use #[tool] to mark methods as MCP tools",
        ));
    }

    // Validate custom initialize function if provided
    if let Some(ref init_fn) = args.initialize_fn {
        validate_custom_initialize_fn(&impl_block, init_fn)?;
    }

    let struct_name = syn::Ident::new(&info.struct_name, proc_macro2::Span::call_site());
    let call_tool = generate_call_tool(&info);
    let list_tools = generate_list_tools(&info);
    let initialize = generate_initialize(&info, args.initialize_fn.as_ref());

    Ok(quote! {
        #impl_block

        #[async_trait::async_trait]
        impl tmcp::ServerHandler for #struct_name {
            #initialize
            #list_tools
            #call_tool
        }
    })
}

/// Derive the ServerHandler methods from an impl block.
#[proc_macro_attribute]
pub fn mcp_server(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let attr_tokens = TokenStream::from(attr);
    let input_tokens = TokenStream::from(input);

    match inner_mcp_server(attr_tokens, &input_tokens) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Mark a method as an mcp tool.
#[proc_macro_attribute]
pub fn tool(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    input
}

/// Adds a _meta field to a struct with proper serde attributes and builder methods.
///
/// This macro adds the following field to the struct:
/// ```ignore
/// #[serde(skip_serializing_if = "Option::is_none")]
/// pub _meta: Option<HashMap<String, Value>>,
/// ```
///
/// And generates these builder methods:
/// - `with_meta(mut self, meta: HashMap<String, Value>) -> Self`
/// - `with_meta_entry(mut self, key: impl Into<String>, value: Value) -> Self`
#[proc_macro_attribute]
pub fn with_meta(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut input = syn::parse_macro_input!(input as syn::DeriveInput);

    // Only process structs
    let syn::Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new(
            input.ident.span(),
            "with_meta can only be applied to structs",
        )
        .to_compile_error()
        .into();
    };

    let syn::Fields::Named(fields) = &mut data_struct.fields else {
        return syn::Error::new(
            input.ident.span(),
            "with_meta can only be applied to structs with named fields",
        )
        .to_compile_error()
        .into();
    };

    // Create the _meta field
    let meta_field: syn::Field = syn::parse_quote! {
        /// Optional metadata field for extensions.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub _meta: Option<std::collections::HashMap<String, serde_json::Value>>
    };

    // Add the field
    fields.named.push(meta_field);

    // Generate the struct name and generics
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the output with builder methods
    let output = quote! {
        #input

        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Set the metadata map
            pub fn with_meta(mut self, meta: std::collections::HashMap<String, serde_json::Value>) -> Self {
                self._meta = Some(meta);
                self
            }

            /// Add a single metadata entry
            pub fn with_meta_entry(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
                self._meta
                    .get_or_insert_with(std::collections::HashMap::new)
                    .insert(key.into(), value);
                self
            }
        }
    };

    output.into()
}

/// Adds name and title fields to a struct with proper serde attributes, documentation, and builder methods.
///
/// This macro adds the following fields to the struct:
/// ```ignore
/// /// Intended for programmatic or logical use, but used as a display name in past specs or fallback (if title isn't present).
/// pub name: String,
///
/// /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
/// /// even by those unfamiliar with domain-specific terminology.
/// ///
/// /// If not provided, the name should be used for display.
/// #[serde(skip_serializing_if = "Option::is_none")]
/// pub title: Option<String>,
/// ```
///
/// And generates these builder methods:
/// - `with_name(mut self, name: impl Into<String>) -> Self`
/// - `with_title(mut self, title: impl Into<String>) -> Self`
#[proc_macro_attribute]
pub fn with_basename(
    _attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let mut input = syn::parse_macro_input!(input as syn::DeriveInput);

    // Only process structs
    let syn::Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new(
            input.ident.span(),
            "with_basename can only be applied to structs",
        )
        .to_compile_error()
        .into();
    };

    let syn::Fields::Named(fields) = &mut data_struct.fields else {
        return syn::Error::new(
            input.ident.span(),
            "with_basename can only be applied to structs with named fields",
        )
        .to_compile_error()
        .into();
    };

    // Create the name field
    let name_field: syn::Field = syn::parse_quote! {
        /// Intended for programmatic or logical use, but used as a display name in past specs or fallback (if title isn't present).
        pub name: String
    };

    // Create the title field
    let title_field: syn::Field = syn::parse_quote! {
        /// Intended for UI and end-user contexts — optimized to be human-readable and easily understood,
        /// even by those unfamiliar with domain-specific terminology.
        ///
        /// If not provided, the name should be used for display.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub title: Option<String>
    };

    // Add the fields
    fields.named.push(name_field);
    fields.named.push(title_field);

    // Generate the struct name and generics
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the output with builder methods
    let output = quote! {
        #input

        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Set the name field
            pub fn with_name(mut self, name: impl Into<String>) -> Self {
                self.name = name.into();
                self
            }

            /// Set the title field
            pub fn with_title(mut self, title: impl Into<String>) -> Self {
                self.title = Some(title.into());
                self
            }
        }
    };

    output.into()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn test_doc_extraction() {
        let attrs = vec![
            syn::parse_quote! { #[doc = " First line"] },
            syn::parse_quote! { #[doc = " Second line"] },
            syn::parse_quote! { #[doc = ""] },
            syn::parse_quote! { #[doc = " Third line"] },
        ];

        let result = extract_doc_comment(&attrs);
        assert_eq!(result, "First line\nSecond line\nThird line");
    }

    #[test]
    fn test_parse_tool_method_valid() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            /// This is a test tool
            async fn test_tool(&self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method).unwrap();
        assert!(result.is_some());

        let tool = result.unwrap();
        assert_eq!(tool.name, "test_tool");
        assert_eq!(tool.docs, "This is a test tool");
    }

    #[test]
    fn test_parse_tool_method_not_async() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            fn test_tool(&mut self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("must be async"));
    }

    #[test]
    fn test_parse_tool_method_wrong_params() {
        let method: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&mut self) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };

        let result = parse_tool_method(&method);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exactly 3 parameters")
        );
    }

    #[test]
    fn test_parse_tool_method_both_self_types() {
        // Test that both &self and &mut self are accepted
        let method1: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };
        assert!(parse_tool_method(&method1).unwrap().is_some());

        let method2: syn::ImplItemFn = syn::parse_quote! {
            #[tool]
            async fn test_tool(&mut self, context: &ServerCtx, params: TestParams) -> Result<schema::CallToolResult> {
                Ok(schema::CallToolResult::new())
            }
        };
        assert!(parse_tool_method(&method2).unwrap().is_some());
    }

    #[test]
    fn test_parse_impl_block() {
        let input = quote! {
            /// Test server implementation
            impl TestServer {
                #[tool]
                /// Echo tool
                async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }

                #[tool]
                async fn ping(&self, context: &ServerCtx, params: PingParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }

                // This method should be ignored
                async fn helper(&self) -> String {
                    "helper".to_string()
                }
            }
        };

        let (_, info) = parse_impl_block(&input).unwrap();
        assert_eq!(info.struct_name, "TestServer");
        assert_eq!(info.description, "Test server implementation");
        assert_eq!(info.tools.len(), 2);
        assert_eq!(info.tools[0].name, "echo");
        assert_eq!(info.tools[0].docs, "Echo tool");
        assert_eq!(info.tools[1].name, "ping");
    }

    #[test]
    fn test_generate_call_tool() {
        let info = ServerInfo {
            struct_name: "TestServer".to_string(),
            description: "Test description".to_string(),
            tools: vec![
                ToolMethod {
                    name: "echo".to_string(),
                    docs: "Echo tool".to_string(),
                    params_type: syn::parse_quote! { EchoParams },
                },
                ToolMethod {
                    name: "ping".to_string(),
                    docs: "Ping tool".to_string(),
                    params_type: syn::parse_quote! { PingParams },
                },
            ],
        };

        let generated = generate_call_tool(&info);
        let generated_str = generated.to_string();

        assert!(generated_str.contains("call_tool"));
        assert!(generated_str.contains("match name . as_str ()"));
        assert!(generated_str.contains("\"echo\" =>"));
        assert!(generated_str.contains("\"ping\" =>"));
        assert!(generated_str.contains("self . echo (context , params) . await"));
        assert!(generated_str.contains("self . ping (context , params) . await"));
    }

    #[test]
    fn test_generate_list_tools() {
        let info = ServerInfo {
            struct_name: "TestServer".to_string(),
            description: "Test description".to_string(),
            tools: vec![ToolMethod {
                name: "echo".to_string(),
                docs: "Echo tool".to_string(),
                params_type: syn::parse_quote! { EchoParams },
            }],
        };

        let generated = generate_list_tools(&info);
        let generated_str = generated.to_string();

        assert!(generated_str.contains("list_tools"));
        assert!(generated_str.contains("Tool :: new"));
        assert!(generated_str.contains("with_description"));
        assert!(generated_str.contains("\"echo\""));
        assert!(generated_str.contains("\"Echo tool\""));
    }

    #[test]
    fn test_full_macro_expansion() {
        let input = quote! {
            /// Test server
            impl TestServer {
                #[tool]
                /// Echo back the input
                async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
        let result_str = result.to_string();

        // Check that original impl block is preserved
        assert!(result_str.contains("impl TestServer"));
        assert!(result_str.contains("async fn echo"));

        // Check that ServerHandler impl is generated
        assert!(result_str.contains("impl tmcp :: ServerHandler for TestServer"));
        assert!(result_str.contains("async fn initialize"));
        assert!(result_str.contains("async fn list_tools"));
        assert!(result_str.contains("async fn call_tool"));

        // Check that snake_case conversion is applied
        assert!(result_str.contains(r#"InitializeResult :: new ("test_server")"#));
    }

    #[test]
    fn test_no_tools_error() {
        let input = quote! {
            impl TestServer {
                async fn helper(&self) -> String {
                    "helper".to_string()
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No tool methods found")
        );
    }

    #[test]
    fn test_snake_case_conversion() {
        // Test various struct name patterns
        let test_cases = vec![
            ("TestServer", "test_server"),
            ("MyMCPServer", "my_mcp_server"),
            ("HTTPServer", "http_server"),
            ("SimpleServer", "simple_server"),
            ("MyHTTPAPIServer", "my_httpapi_server"),
        ];

        for (struct_name, expected_snake_case) in test_cases {
            let struct_ident = syn::Ident::new(struct_name, proc_macro2::Span::call_site());
            let input = quote! {
                impl #struct_ident {
                    #[tool]
                    async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                        Ok(schema::CallToolResult::new())
                    }
                }
            };

            let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
            let result_str = result.to_string();

            let expected_pattern = format!(r#"InitializeResult :: new ("{expected_snake_case}")"#);
            assert!(
                result_str.contains(&expected_pattern),
                "Expected server name '{expected_snake_case}' for struct '{struct_name}', but got: {result_str}"
            );
        }
    }

    #[test]
    fn test_empty_description_no_instructions() {
        let input = quote! {
            impl TestServer {
                #[tool]
                async fn echo(&self, context: &ServerCtx, params: EchoParams) -> Result<schema::CallToolResult> {
                    Ok(schema::CallToolResult::new())
                }
            }
        };

        let result = inner_mcp_server(TokenStream::new(), &input).unwrap();
        let result_str = result.to_string();

        // Check that with_instructions is NOT called when description is empty
        assert!(!result_str.contains("with_instructions"));
        assert!(result_str.contains("InitializeResult :: new"));
        assert!(result_str.contains("with_version"));
        assert!(result_str.contains("with_tools"));
    }
}
