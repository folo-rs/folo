// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

#![allow(
    missing_docs,
    reason = "Private API, public API is documented in `linked` package"
)]

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn __macro_linked_object(attr: TokenStream, item: TokenStream) -> TokenStream {
    linked_macros_impl::linked_object::entrypoint(&attr.into(), &item.into()).into()
}
