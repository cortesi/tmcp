//! Expansion of `#[group]` impl blocks, `#[group]` attributes, and `#[derive(Group)]`.

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Result, spanned::Spanned};

use crate::{
    codegen::{
        build_tool_expr, generate_flat_arg_structs, generate_tool_call_arm, tool_schema_expr,
    },
    model::{GroupMethod, ServerInfo},
    parse::{extract_doc_comment, parse_group_meta, parse_impl_block},
};

/// Generate group dispatch checks for nested group factory methods.
pub fn generate_group_dispatch_chain(
    groups: &[GroupMethod],
    receiver: &TokenStream,
) -> TokenStream {
    let checks = groups.iter().map(|group| {
        let method = &group.ident;
        let segment_expr = group
            .segment_override
            .as_ref()
            .map(|name| quote! { #name.to_string() })
            .unwrap_or_else(|| quote! { ::tmcp::Group::name(&group) });
        quote! {
            {
                let group = #receiver.#method();
                let segment = #segment_expr;
                if let Some(rest) = name.strip_prefix(segment.as_str()) {
                    if let Some(rest) = rest.strip_prefix('.') {
                        return ::tmcp::GroupDispatch::call_tool(&group, context, rest, arguments, task)
                            .await;
                    }
                }
            }
        }
    });

    quote! {
        #(#checks)*
    }
}

/// Generate the GroupDispatch implementation for a group impl block.
fn generate_group_dispatch_impl(info: &ServerInfo) -> TokenStream {
    let self_ty = &info.self_ty;
    let (impl_generics, _, where_clause) = info.generics.split_for_impl();

    let tool_registrations = info.tools.iter().map(|tool| {
        let name = &tool.name;
        let name_expr = quote! { name.clone() };
        let schema = tool_schema_expr(tool, &info.struct_name);
        let tool_expr = build_tool_expr(tool, &name_expr, &schema);
        let always_visible = tool.attrs.always || name == "activate" || name == "deactivate";
        let visibility = if always_visible {
            quote! { ::tmcp::Visibility::Always }
        } else {
            quote! { ::tmcp::Visibility::Group(group_name.to_string()) }
        };
        quote! {
            {
                let name = ::tmcp::ToolSet::qualified_name(group_name, #name);
                let tool = #tool_expr;
                toolset.register_schema(&name, tool, #visibility)?;
            }
        }
    });

    let group_registrations = info.groups.iter().map(|group| {
        let method = &group.ident;
        let override_expr = group
            .segment_override
            .as_ref()
            .map(|name| quote! { Some(#name) })
            .unwrap_or_else(|| quote! { None });
        quote! {
            {
                let group = self.#method();
                ::tmcp::GroupRegistration::register_with_override(
                    &group,
                    toolset,
                    Some(group_name),
                    #override_expr,
                )?;
            }
        }
    });

    let receiver = quote! { self };
    let tool_matches = info
        .tools
        .iter()
        .map(|tool| generate_tool_call_arm(tool, &receiver, &info.struct_name));
    let group_dispatch = generate_group_dispatch_chain(&info.groups, &receiver);

    quote! {
        impl #impl_generics ::tmcp::GroupDispatch for #self_ty #where_clause {
            fn register_tools(&self, toolset: &::tmcp::ToolSet, group_name: &str) -> ::tmcp::Result<()> {
                #(#tool_registrations)*
                #(#group_registrations)*
                Ok(())
            }

            fn call_tool<'a>(
                &'a self,
                context: &'a ::tmcp::ServerCtx,
                name: &'a str,
                arguments: Option<::tmcp::Arguments>,
                task: Option<::tmcp::schema::TaskMetadata>,
            ) -> ::tmcp::ToolCallFuture<'a> {
                Box::pin(async move {
                    match name {
                        #(#tool_matches)*
                        _ => {
                            #group_dispatch
                            Err(::tmcp::Error::ToolNotFound(name.to_string()))
                        }
                    }
                })
            }
        }
    }
}

/// Expand a #[group] impl block into dispatch and registration logic.
fn expand_group_impl(input: &TokenStream) -> Result<TokenStream> {
    let impl_block = syn::parse2::<syn::ItemImpl>(input.clone())?;
    if impl_block.trait_.is_some() {
        return Err(syn::Error::new(
            impl_block.impl_token.span(),
            "#[group] can only be used on inherent impl blocks",
        ));
    }

    let (impl_block, info) = parse_impl_block(input)?;
    let flat_structs = generate_flat_arg_structs(&info);
    let dispatch_impl = generate_group_dispatch_impl(&info);

    Ok(quote! {
        #(#flat_structs)*
        #impl_block
        #dispatch_impl
    })
}

/// Expand the `#[group]` attribute for any supported item shape.
///
/// Impl blocks get dispatch and registration glue; structs and enums have the
/// attribute arguments re-attached as `#[tmcp_group_meta(...)]` for
/// `#[derive(Group)]`; methods pass through (the enclosing `#[mcp_server]` or
/// `#[group]` impl expansion consumes them). Anything else is a compile error.
pub fn expand_group_attribute(attr: &TokenStream, input: &TokenStream) -> TokenStream {
    if let Ok(item_impl) = syn::parse2::<syn::ItemImpl>(input.clone()) {
        if !attr.is_empty() {
            return syn::Error::new(
                item_impl.impl_token.span(),
                "#[group] on impl blocks does not accept arguments",
            )
            .to_compile_error();
        }
        let impl_tokens = quote! { #item_impl };
        return match expand_group_impl(&impl_tokens) {
            Ok(tokens) => tokens,
            Err(err) => err.to_compile_error(),
        };
    }

    if let Ok(mut item) = syn::parse2::<syn::DeriveInput>(input.clone()) {
        if !attr.is_empty() {
            let meta_attr: syn::Attribute = syn::parse_quote! { #[tmcp_group_meta(#attr)] };
            item.attrs.push(meta_attr);
        }
        return quote!(#item);
    }

    if let Ok(method) = syn::parse2::<syn::ImplItemFn>(input.clone()) {
        return quote!(#method);
    }

    let error = syn::Error::new(
        input.span(),
        "#[group] must be applied to an impl block, a struct, or a group factory method",
    )
    .to_compile_error();
    quote! {
        #error
        #input
    }
}

/// Build the activation/deactivation hook expression for a derived group.
fn activation_hook_expr(method: Option<&syn::Ident>) -> TokenStream {
    match method {
        Some(method) => quote! {
            Some(Box::new({
                let group = self.clone();
                move |ctx| {
                    let group = group.clone();
                    let ctx = ctx.clone();
                    Box::pin(async move { group.#method(&ctx).await })
                }
            }))
        },
        None => quote! { None },
    }
}

/// Expand `#[derive(Group)]` for a group struct.
pub fn expand_derive_group(input: &syn::DeriveInput) -> TokenStream {
    if !matches!(input.data, syn::Data::Struct(_)) {
        return syn::Error::new(input.ident.span(), "Group can only be derived for structs")
            .to_compile_error();
    }
    let ident = &input.ident;

    let doc_description = extract_doc_comment(&input.attrs);
    let meta = match parse_group_meta(&input.attrs) {
        Ok(meta) => meta.unwrap_or_default(),
        Err(err) => return err.to_compile_error(),
    };

    let default_name = ident.to_string().to_snake_case();
    let name = meta.name.unwrap_or(default_name);
    if name.is_empty() || name.contains('.') {
        return syn::Error::new(
            ident.span(),
            "group name must be non-empty and must not contain '.'",
        )
        .to_compile_error();
    }
    let description = meta.description.unwrap_or(doc_description);
    let show_deactivator = meta.show_deactivator.unwrap_or(true);

    let requires_clone = meta.on_activate.is_some() || meta.on_deactivate.is_some();
    let on_activate = activation_hook_expr(meta.on_activate.as_ref());
    let on_deactivate = activation_hook_expr(meta.on_deactivate.as_ref());

    let mut generics = input.generics.clone();
    if requires_clone {
        let type_generics = {
            let (_, ty_generics, _) = generics.split_for_impl();
            quote! { #ty_generics }
        };
        let where_clause = generics.make_where_clause();
        where_clause.predicates.push(syn::parse_quote!(
            #ident #type_generics: Clone + Send + Sync + 'static
        ));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    quote! {
        impl #impl_generics ::tmcp::Group for #ident #ty_generics #where_clause {
            fn name(&self) -> String {
                #name.to_string()
            }

            fn config(&self) -> ::tmcp::GroupConfig {
                ::tmcp::GroupConfig {
                    name: #name.to_string(),
                    description: #description.to_string(),
                    parent: None,
                    on_activate: #on_activate,
                    on_deactivate: #on_deactivate,
                    show_deactivator: #show_deactivator,
                }
            }
        }
    }
}
