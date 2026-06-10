//! Validation of forwarder-callback signatures named in `#[mcp_server]` arguments.

use syn::{ImplItem, ItemImpl, Result, spanned::Spanned};

use crate::{
    model::{ForwarderParam, ServerMacroArgs},
    parse::{
        is_option_cursor_type, is_server_ctx_type, is_string_type, is_unit_type,
        type_path_ends_with,
    },
};

/// Find a named method in an impl block for a macro forwarding hook.
fn find_impl_method<'a>(
    impl_block: &'a ItemImpl,
    fn_name: &syn::Ident,
    role: &str,
) -> Result<&'a syn::ImplItemFn> {
    impl_block
        .items
        .iter()
        .find_map(|item| {
            if let ImplItem::Fn(method) = item
                && method.sig.ident == *fn_name
            {
                Some(method)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            syn::Error::new(
                fn_name.span(),
                format!("{role} function '{fn_name}' not found in impl block"),
            )
        })
}

/// Validate that the first callback parameter is a shared self receiver.
fn validate_shared_self_receiver(arg: &syn::FnArg) -> Result<()> {
    match arg {
        syn::FnArg::Receiver(receiver)
            if receiver.reference.is_some() && receiver.mutability.is_none() =>
        {
            Ok(())
        }
        _ => Err(syn::Error::new(arg.span(), "first parameter must be &self")),
    }
}

/// Validate common callback shape and return the parsed parameter list.
fn validate_callback_signature<'a>(
    method: &'a syn::ImplItemFn,
    role: &str,
    expected_param_count: usize,
) -> Result<Vec<&'a syn::FnArg>> {
    if method.sig.asyncness.is_none() {
        return Err(syn::Error::new(
            method.sig.span(),
            format!("{role} function must be async"),
        ));
    }

    let params: Vec<_> = method.sig.inputs.iter().collect();
    if params.len() != expected_param_count {
        return Err(syn::Error::new(
            method.sig.inputs.span(),
            format!("{role} function must have exactly {expected_param_count} parameters"),
        ));
    }

    validate_shared_self_receiver(params[0])?;
    Ok(params)
}

/// Validate a callback parameter's type.
fn validate_callback_arg_type(
    arg: &syn::FnArg,
    role: &str,
    expected: &str,
    is_expected: fn(&syn::Type) -> bool,
) -> Result<()> {
    let syn::FnArg::Typed(pat_type) = arg else {
        return Err(syn::Error::new(
            arg.span(),
            format!("{role} parameter must be {expected}"),
        ));
    };
    if is_expected(pat_type.ty.as_ref()) {
        Ok(())
    } else {
        Err(syn::Error::new(
            pat_type.ty.span(),
            format!("{role} parameter must be {expected}"),
        ))
    }
}

/// Return the successful payload type from a `Result<T>` return type.
fn result_inner_type<'a>(output: &'a syn::ReturnType, role: &str) -> Result<&'a syn::Type> {
    let syn::ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new(
            output.span(),
            format!("{role} function must return Result<T>"),
        ));
    };
    let syn::Type::Path(type_path) = ty.as_ref() else {
        return Err(syn::Error::new(
            ty.span(),
            format!("{role} function must return Result<T>"),
        ));
    };
    let Some(segment) = type_path.path.segments.last() else {
        return Err(syn::Error::new(
            ty.span(),
            format!("{role} function must return Result<T>"),
        ));
    };
    if segment.ident != "Result" {
        return Err(syn::Error::new(
            segment.ident.span(),
            format!("{role} function must return Result<T>"),
        ));
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new(
            segment.arguments.span(),
            format!("{role} function must return Result<T>"),
        ));
    };

    arguments
        .args
        .iter()
        .find_map(|argument| {
            if let syn::GenericArgument::Type(ty) = argument {
                Some(ty)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            syn::Error::new(
                segment.arguments.span(),
                format!("{role} function must return Result<T>"),
            )
        })
}

/// Validate a callback result payload type.
fn validate_result_payload(
    method: &syn::ImplItemFn,
    role: &str,
    expected_payload: Option<&str>,
) -> Result<()> {
    let inner = result_inner_type(&method.sig.output, role)?;
    let matches = match expected_payload {
        Some(payload) => type_path_ends_with(inner, payload),
        None => is_unit_type(inner),
    };
    if matches {
        Ok(())
    } else {
        let payload = expected_payload.unwrap_or("()");
        Err(syn::Error::new(
            inner.span(),
            format!("{role} function must return Result<{payload}>"),
        ))
    }
}

/// Validate the signature of a custom initialize function.
pub fn validate_custom_initialize_fn(impl_block: &ItemImpl, fn_name: &syn::Ident) -> Result<()> {
    let method = find_impl_method(impl_block, fn_name, "initialize_fn")?;

    let params = validate_callback_signature(method, "initialize_fn", 5)?;
    validate_callback_arg_type(params[1], "initialize_fn", "&ServerCtx", is_server_ctx_type)?;
    validate_callback_arg_type(params[2], "initialize_fn", "String", is_string_type)?;
    validate_callback_arg_type(params[3], "initialize_fn", "ClientCapabilities", |ty| {
        type_path_ends_with(ty, "ClientCapabilities")
    })?;
    validate_callback_arg_type(params[4], "initialize_fn", "Implementation", |ty| {
        type_path_ends_with(ty, "Implementation")
    })?;
    validate_result_payload(method, "initialize_fn", Some("InitializeResult"))
}

/// Validate all forwarder callbacks configured in the macro arguments.
///
/// Each callback's parameter list and result payload are checked against the
/// forwarder's [`crate::model::ForwarderSpec`] entry.
pub fn validate_server_forwarders(impl_block: &ItemImpl, args: &ServerMacroArgs) -> Result<()> {
    for binding in &args.forwarders {
        let spec = binding.spec;
        let method = find_impl_method(impl_block, &binding.fn_name, spec.arg)?;
        let params = validate_callback_signature(method, spec.arg, 1 + spec.params.len())?;
        for (param, shape) in params[1..].iter().zip(spec.params) {
            match shape {
                ForwarderParam::Ctx => {
                    validate_callback_arg_type(param, spec.arg, "&ServerCtx", is_server_ctx_type)?;
                }
                ForwarderParam::Str(_) => {
                    validate_callback_arg_type(param, spec.arg, "String", is_string_type)?;
                }
                ForwarderParam::Cursor => {
                    validate_callback_arg_type(
                        param,
                        spec.arg,
                        "Option<Cursor>",
                        is_option_cursor_type,
                    )?;
                }
            }
        }
        validate_result_payload(method, spec.arg, spec.payload)?;
    }
    Ok(())
}
