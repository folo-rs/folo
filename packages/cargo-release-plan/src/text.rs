// Shared message formatting helpers.

use std::fmt;

/// Renders a count with a noun that agrees with it.
///
/// Diagnostics report counts that are only known at runtime, and the workspace
/// prefers agreeing prose over `package(s)` forms, so every such message routes
/// through this helper instead of choosing a form at the call site.
pub(crate) fn plural(count: usize, singular: &str) -> impl fmt::Display {
    Plural {
        count,
        singular: singular.to_string(),
    }
}

struct Plural {
    count: usize,
    singular: String,
}

impl fmt::Display for Plural {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.count, self.singular)?;
        if self.count != 1 {
            // Every noun this crate pluralizes takes a bare `s`.
            f.write_str("s")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn agrees_with_the_count() {
        assert_eq!(plural(0, "package").to_string(), "0 packages");
        assert_eq!(plural(1, "package").to_string(), "1 package");
        assert_eq!(plural(2, "manifest").to_string(), "2 manifests");
    }
}
