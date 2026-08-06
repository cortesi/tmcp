//! Expansion of static groups of delegated free tools.

use std::collections::HashSet;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Result, ext::IdentExt, parse::Parse, spanned::Spanned};

use crate::codegen::delegated_descriptor_path;

/// Arguments accepted by `#[tool_group]`.
struct ToolGroupArgs {
    /// Shared state type used by every delegated tool.
    state: syn::Type,
    /// Delegated tool function paths.
    tools: Vec<syn::Path>,
}

impl Parse for ToolGroupArgs {
    fn parse(input: syn::parse::ParseStream) -> Result<Self> {
        let mut state = None;
        let mut tools = Vec::new();
        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            input.parse::<syn::Token![=]>()?;
            if ident == "state" {
                if state.is_some() {
                    return Err(syn::Error::new(ident.span(), "duplicate argument: state"));
                }
                state = Some(input.parse()?);
            } else if ident == "tools" {
                if !tools.is_empty() {
                    return Err(syn::Error::new(ident.span(), "duplicate argument: tools"));
                }
                let array: syn::ExprArray = input.parse()?;
                for element in array.elems {
                    let syn::Expr::Path(path) = element else {
                        return Err(syn::Error::new(
                            element.span(),
                            "tools entries must be function paths",
                        ));
                    };
                    tools.push(path.path);
                }
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
        let state = state.ok_or_else(|| syn::Error::new(input.span(), "missing state"))?;
        if tools.is_empty() {
            return Err(syn::Error::new(input.span(), "tools must not be empty"));
        }
        Ok(Self { state, tools })
    }
}

/// Expands one static delegated-tool group.
pub fn expand_tool_group(attr: &TokenStream, input: &TokenStream) -> Result<TokenStream> {
    let args = syn::parse2::<ToolGroupArgs>(attr.clone())?;
    let item = syn::parse2::<syn::ItemStruct>(input.clone())?;
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            item.generics.span(),
            "tool groups must not be generic",
        ));
    }
    let name = &item.ident;
    let state = &args.state;
    let mut names = HashSet::new();
    let entries = args
        .tools
        .iter()
        .map(|path| {
            let segment = path.segments.last().expect("tool paths are non-empty");
            let tool_name = segment.ident.unraw().to_string();
            if !names.insert(tool_name.clone()) {
                return Err(syn::Error::new(
                    segment.ident.span(),
                    format!("duplicate tool name `{tool_name}`"),
                ));
            }
            let descriptor = delegated_descriptor_path(path);
            Ok((tool_name, descriptor))
        })
        .collect::<Result<Vec<_>>>()?;
    let tool_names = entries.iter().map(|(name, _)| name);
    let schemas = entries.iter().map(|(_, descriptor)| {
        quote! { #descriptor::schema() }
    });
    let task_support = entries.iter().map(|(_, descriptor)| {
        quote! { #descriptor::SUPPORTS_TASKS }
    });
    let dispatch = entries.iter().map(|(name, descriptor)| {
        quote! {
            #name => #descriptor::call(state, context, arguments, task),
        }
    });

    Ok(quote! {
        #item

        impl ::tmcp::ToolGroup for #name {
            type State = #state;

            const NAMES: &'static [&'static str] = &[#(#tool_names),*];

            fn schemas() -> Vec<::tmcp::schema::Tool> {
                vec![#(#schemas),*]
            }

            fn supports_tasks() -> bool {
                false #( || #task_support )*
            }

            fn call<'a>(
                state: Self::State,
                context: &'a ::tmcp::ServerCtx,
                name: &str,
                arguments: Option<::tmcp::Arguments>,
                task: Option<::tmcp::schema::TaskMetadata>,
            ) -> ::tmcp::ToolCallFuture<'a> {
                match name {
                    #(#dispatch)*
                    _ => {
                        let name = name.to_owned();
                        Box::pin(async move { Err(::tmcp::Error::ToolNotFound(name)) })
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use quote::quote;

    use super::*;

    #[test]
    fn duplicate_tool_names_are_rejected() {
        let error = expand_tool_group(
            &quote! { state = State, tools = [first::echo, second::echo] },
            &quote! { struct EchoTools; },
        )
        .expect_err("duplicate names must fail");

        assert!(error.to_string().contains("duplicate tool name `echo`"));
    }
}
