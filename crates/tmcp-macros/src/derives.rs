//! Derive expansion for tool response types and derive-merging attributes.

use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;

/// Collect derive identifiers from attributes.
fn collect_derive_idents(attrs: &[syn::Attribute]) -> Vec<String> {
    let mut idents = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        let derive_args = attr
            .parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
            .unwrap_or_default();
        for path in derive_args {
            if let Some(ident) = path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            {
                idents.push(ident);
            }
        }
    }
    idents
}

/// Add derive attributes from the provided paths if they are missing.
///
/// Returns the trailing identifiers of the derives that were added, so callers
/// can attach companion attributes (such as crate-path overrides) only when
/// the corresponding derive came from this macro.
fn add_missing_derives(item: &mut syn::DeriveInput, derive_paths: &[syn::Path]) -> Vec<String> {
    let existing = collect_derive_idents(&item.attrs);
    let mut missing = Vec::new();
    let mut added = Vec::new();

    for path in derive_paths {
        if let Some(ident) = path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            && !existing.iter().any(|e| e == &ident)
        {
            missing.push(path.clone());
            added.push(ident);
        }
    }

    if !missing.is_empty() {
        let derive_attr: syn::Attribute = syn::parse_quote! {
            #[derive(#(#missing),*)]
        };
        item.attrs.insert(0, derive_attr);
    }
    added
}

/// Attach serde/schemars crate-path attributes for derives added via
/// `__private`.
///
/// The serde and schemars derive macros resolve their support code through the
/// `serde`/`schemars` crate names unless redirected, so derives injected from
/// `::tmcp::__private` need explicit crate paths to work without direct
/// dependencies in the consuming crate.
fn add_private_crate_attrs(item: &mut syn::DeriveInput, added: &[String]) {
    if added
        .iter()
        .any(|name| name == "Serialize" || name == "Deserialize")
    {
        item.attrs
            .push(syn::parse_quote! { #[serde(crate = "::tmcp::__private::serde")] });
    }
    if added.iter().any(|name| name == "JsonSchema") {
        item.attrs
            .push(syn::parse_quote! { #[schemars(crate = "::tmcp::__private::schemars")] });
    }
}

/// Expand `#[tool_params]`: add serde + schemars derives for parameter structs.
pub fn expand_tool_params(mut item: syn::DeriveInput) -> TokenStream {
    let added = add_missing_derives(
        &mut item,
        &[
            syn::parse_quote!(::tmcp::__private::serde::Deserialize),
            syn::parse_quote!(::tmcp::__private::schemars::JsonSchema),
        ],
    );
    add_private_crate_attrs(&mut item, &added);
    quote!(#item)
}

/// Expand `#[tool_result]`: add serde + schemars + ToolResponse derives for
/// result structs.
pub fn expand_tool_result(mut item: syn::DeriveInput) -> TokenStream {
    let added = add_missing_derives(
        &mut item,
        &[
            syn::parse_quote!(::tmcp::__private::serde::Serialize),
            syn::parse_quote!(::tmcp::__private::schemars::JsonSchema),
            syn::parse_quote!(::tmcp::ToolResponse),
        ],
    );
    add_private_crate_attrs(&mut item, &added);
    quote!(#item)
}

/// Expand `#[derive(ToolResponse)]`: encode the type as structured content.
pub fn expand_derive_tool_response(input: &syn::DeriveInput) -> TokenStream {
    let ident = &input.ident;
    let mut generics = input.generics.clone();
    {
        let where_clause = generics.make_where_clause();
        where_clause
            .predicates
            .push(syn::parse_quote! { Self: ::tmcp::__private::serde::Serialize });
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics ::tmcp::ToolResponse for #ident #ty_generics #where_clause {
            fn into_call_tool_result(self) -> ::tmcp::schema::CallToolResult {
                match ::tmcp::schema::CallToolResult::structured(self) {
                    Ok(result) => result,
                    Err(err) => ::tmcp::schema::CallToolResult::error("INTERNAL", err.to_string()),
                }
            }
        }
    }
}
