//! Configuration failure types shared by parsing and input resolution.

use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::PathBuf;

/// Loading configuration or resolving a configured option failed.
///
/// The error preserves the concrete failure and any underlying I/O or TOML
/// error in its source chain.
#[ohno::error]
#[no_constructors]
#[from(ReadConfigError, ParseConfigError, SelectionEnvironmentRequiredError)]
pub struct ConfigError;

// The #[ohno::error] macro injects an OhnoCore field containing Arc<dyn Error + Send + Sync>,
// which is !UnwindSafe because Arc requires T: RefUnwindSafe and trait objects are !RefUnwindSafe.
// However, ohno error types are immutable after construction — no &self method mutates internal
// state — so observing them through a shared reference during unwind is harmless.
impl UnwindSafe for ConfigError {}
impl RefUnwindSafe for ConfigError {}

/// Reading the selected configuration file failed.
#[ohno::error]
#[display("failed to read configuration at {}", path.display())]
pub(crate) struct ReadConfigError {
    pub(crate) path: PathBuf,
}

impl UnwindSafe for ReadConfigError {}
impl RefUnwindSafe for ReadConfigError {}

/// Configuration text was not valid for the supported TOML schema.
#[ohno::error]
#[display("failed to parse configuration")]
#[from(toml::de::Error)]
pub(crate) struct ParseConfigError;

impl UnwindSafe for ParseConfigError {}
impl RefUnwindSafe for ParseConfigError {}

/// A path-selecting option required an environment value that was unavailable.
#[ohno::error]
#[display("{option} was given without a path and {environment} is unset or empty")]
pub(crate) struct SelectionEnvironmentRequiredError {
    pub(crate) option: &'static str,
    pub(crate) environment: &'static str,
}

impl UnwindSafe for SelectionEnvironmentRequiredError {}
impl RefUnwindSafe for SelectionEnvironmentRequiredError {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::error::Error;
    use std::fmt::Debug;

    use ohno::ErrorExt;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(
        ConfigError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ReadConfigError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        ParseConfigError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        SelectionEnvironmentRequiredError: Send,
        Sync,
        Debug,
        Error,
        UnwindSafe,
        RefUnwindSafe
    );

    #[test]
    fn aggregate_preserves_the_concrete_source_chain() {
        let io_error = std::io::Error::other("inner");
        let error = ConfigError::from(ReadConfigError::caused_by("config.toml", io_error));

        assert!(error.find_source::<ReadConfigError>().is_some());
        assert!(error.find_source::<std::io::Error>().is_some());
    }
}
