//! Field-injection macros used internally by tmcp's schema module.
//!
//! These expansions reference `serde`, `serde_json`, and `std` by their plain
//! crate names because they only ever expand inside the tmcp crate itself,
//! where those dependencies are guaranteed to be present.

use proc_macro2::TokenStream;
use quote::quote;

/// Expand either closed `_meta` support or open `_meta` plus extension support.
pub fn expand_meta(mut input: syn::DeriveInput, open: bool) -> TokenStream {
    // Only process structs
    let syn::Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new(
            input.ident.span(),
            "with_meta can only be applied to structs with named fields",
        )
        .to_compile_error();
    };

    let syn::Fields::Named(fields) = &mut data_struct.fields else {
        return syn::Error::new(
            input.ident.span(),
            "with_meta can only be applied to structs with named fields",
        )
        .to_compile_error();
    };

    // Create the extension fields.
    let meta_field: syn::Field = syn::parse_quote! {
        /// Optional metadata field for extensions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub _meta: Option<std::collections::HashMap<String, serde_json::Value>>
    };

    // Add the fields.
    fields.named.push(meta_field);
    if open {
        let extra_field: syn::Field = syn::parse_quote! {
            /// Unknown protocol fields preserved for forward compatibility on open MCP objects.
            #[serde(flatten, default, skip_serializing_if = "std::collections::HashMap::is_empty")]
            pub _extra: std::collections::HashMap<String, serde_json::Value>
        };
        fields.named.push(extra_field);
    }

    // Generate the struct name and generics
    let struct_name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let extra_impl = if open {
        quote! {
            /// Set the preserved extra-field map.
            ///
            /// Only use this for MCP objects whose schema inherits `Result` or is otherwise
            /// explicitly open to top-level extension fields.
            pub fn with_extra(mut self, extra: std::collections::HashMap<String, serde_json::Value>) -> Self {
                self._extra = extra;
                self
            }

            /// Add a single preserved extra-field entry.
            ///
            /// Only use this for MCP objects whose schema inherits `Result` or is otherwise
            /// explicitly open to top-level extension fields.
            pub fn with_extra_entry(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
                self._extra.insert(key.into(), value);
                self
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #input

        impl #impl_generics #struct_name #ty_generics #where_clause {
            /// Set the MCP `_meta` map.
            ///
            /// Third-party extension keys should be namespaced, such as with reverse-DNS keys,
            /// and must not use MCP-reserved prefixes.
            pub fn with_meta(mut self, meta: std::collections::HashMap<String, serde_json::Value>) -> Self {
                self._meta = Some(meta);
                self
            }

            /// Add a single MCP `_meta` entry.
            ///
            /// Third-party extension keys should be namespaced, such as with reverse-DNS keys,
            /// and must not use MCP-reserved prefixes.
            pub fn with_meta_entry(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
                self._meta
                    .get_or_insert_with(std::collections::HashMap::new)
                    .insert(key.into(), value);
                self
            }

            #extra_impl
        }
    }
}

/// Expand `#[with_basename]`: add name/title fields and builder methods.
pub fn expand_basename(mut input: syn::DeriveInput) -> TokenStream {
    // Only process structs
    let syn::Data::Struct(data_struct) = &mut input.data else {
        return syn::Error::new(
            input.ident.span(),
            "with_basename can only be applied to structs",
        )
        .to_compile_error();
    };

    let syn::Fields::Named(fields) = &mut data_struct.fields else {
        return syn::Error::new(
            input.ident.span(),
            "with_basename can only be applied to structs with named fields",
        )
        .to_compile_error();
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
    quote! {
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
    }
}
