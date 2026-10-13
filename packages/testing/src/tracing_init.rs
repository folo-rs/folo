// Startup convention adapted from microsoft/oxidizer's testing_aids init_tracing macro.
// Copyright (c) Microsoft Corporation. Licensed under the MIT License.
// See ../LICENSE-oxidizer.

/// Enables silent, fully formatted tracing before a test executable starts.
///
/// Invoke at the crate root, behind `#[cfg(test)]` for library unit tests.
/// Integration test binaries invoke it separately because they link non-test
/// library builds. Repeated invocations share one initialization.
///
/// Do not combine this with [`TracingCapture::start()`][crate::TracingCapture::start]
/// or another subscriber initializer in the same executable.
#[macro_export]
macro_rules! init_tracing {
    () => {
        const _: () = {
            $crate::__private::ctor! {
                // This constructor only configures process-local tracing state. It performs
                // no output, thread creation or calls into application code, and does not
                // depend on the order of other constructors. See docs/testing.md,
                // "Tracing log coverage", for the executable initialization boundary.
                #[ctor(unsafe)]
                fn initialize_tracing() {
                    $crate::initialize_tracing();
                }
            }
        };
    };
}
