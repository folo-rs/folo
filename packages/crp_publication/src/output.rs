use std::fmt;
use std::sync::Arc;

use crp_diag::{DiagnosticSink, Verbose, diagnostic};

/// Carries executable producer identity and shell-selected publication diagnostics.
///
/// This is output wiring, not release policy or an environment abstraction. Native
/// execution receives only the diagnostic destination, and versioning only lazy notes.
#[derive(Clone, Debug)]
pub struct PublicationOutput {
    tool_version: String,
    verbose: bool,
    diagnostics: Arc<dyn DiagnosticSink>,
}

impl PublicationOutput {
    pub fn new(tool_version: &str, verbose: bool, diagnostics: Arc<dyn DiagnosticSink>) -> Self {
        Self {
            tool_version: tool_version.to_owned(),
            verbose,
            diagnostics,
        }
    }

    #[must_use]
    pub fn notes(&self) -> Verbose<'_> {
        Verbose::new(self.verbose, self.diagnostics.as_ref())
    }

    #[must_use]
    pub fn diagnostics(&self) -> &Arc<dyn DiagnosticSink> {
        &self.diagnostics
    }

    pub(crate) fn tool_version(&self) -> &str {
        &self.tool_version
    }

    pub(crate) fn user_agent(&self) -> String {
        format!("cargo-release-plan/{}", self.tool_version)
    }

    pub fn line(&self, message: fmt::Arguments<'_>) {
        diagnostic(self.diagnostics.as_ref(), &format!("{message}\n"));
    }

    pub fn text(&self, message: fmt::Arguments<'_>) {
        diagnostic(self.diagnostics.as_ref(), &message.to_string());
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Mutex;

    use crp_diag::DiagnosticSink;

    use super::*;

    #[derive(Debug, Default)]
    struct Recording(Mutex<Vec<String>>);

    impl DiagnosticSink for Recording {
        fn write(&self, text: &str) -> std::io::Result<()> {
            self.0.lock().unwrap().push(text.to_owned());
            Ok(())
        }
    }

    #[test]
    fn producer_identity_and_output_come_from_the_shell() {
        let sink = Arc::new(Recording::default());
        let output = PublicationOutput::new("9.8.7", false, Arc::<Recording>::clone(&sink));
        assert_eq!(output.tool_version(), "9.8.7");
        assert_eq!(output.user_agent(), "cargo-release-plan/9.8.7");
        output
            .notes()
            .note(|| panic!("disabled notes must not format"));
        output.line(format_args!("failure {}", 3));
        output.text(format_args!("child output"));
        assert_eq!(*sink.0.lock().unwrap(), ["failure 3\n", "child output"]);
        let output = PublicationOutput::new("9.8.7", true, Arc::<Recording>::clone(&sink));
        output.notes().note(|| "reason".to_owned());
        assert_eq!(
            sink.0.lock().unwrap().last().unwrap(),
            "[release-plan] reason\n"
        );
        assert!(Arc::ptr_eq(
            output.diagnostics(),
            &output.clone().diagnostics
        ));
    }
}
