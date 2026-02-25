// Copyright (c) Microsoft Corporation.
// Copyright (c) Folo authors.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![allow(
    missing_docs,
    reason = "Private API, public API is documented in `linked` package"
)]

pub mod linked_object;
mod syn_helpers;
