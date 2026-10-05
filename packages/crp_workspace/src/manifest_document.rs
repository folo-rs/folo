//! Parsed TOML shared by readers of already acquired content.

use std::collections::HashMap;
use std::path::Path;

use ohno::AppError;
use toml_edit::DocumentMut;

use crate::manifest::parse_document;

/// Manifest syntax owned by one operation's acquisitions.
///
/// Exact string values and TOML shapes remain intact, unlike Cargo's normalized
/// requirements. Fresh content is supplied by each acquisition; no paths or
/// inherited interpretations are retained here.
#[derive(Debug, Default)]
pub struct ManifestDocuments {
    documents: HashMap<String, DocumentMut>,
}

impl ManifestDocuments {
    /// Reuses only syntax belonging to the supplied, already acquired complete text.
    pub fn parse(&mut self, path: &Path, text: &str) -> Result<DocumentMut, AppError> {
        self.parse_with(text, || parse_document(path, text))
    }

    fn parse_with(
        &mut self,
        text: &str,
        acquire: impl FnOnce() -> Result<DocumentMut, AppError>,
    ) -> Result<DocumentMut, AppError> {
        if let Some(document) = self.documents.get(text) {
            return Ok(document.clone());
        }
        let document = acquire()?;
        self.documents.insert(text.to_owned(), document.clone());
        Ok(document)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn equal_content_reuses_parsing_but_changed_content_and_new_memory_reacquire() {
        let mut documents = ManifestDocuments::default();
        let text = "[dependencies]\na = '= 1.2.3'";
        let document = documents
            .parse_with(text, || parse_document(Path::new("first"), text))
            .unwrap();
        let reused = documents
            .parse_with(text, || panic!("duplicate parse"))
            .unwrap();
        assert_eq!(document.to_string(), reused.to_string());
        let changed = "[dependencies]\na = '=1.2.4'";
        let document = documents.parse(Path::new("second"), changed).unwrap();
        assert_eq!(
            document
                .get("dependencies")
                .unwrap()
                .get("a")
                .unwrap()
                .as_str(),
            Some("=1.2.4")
        );
        let mut documents = ManifestDocuments::default();
        let mut called = false;
        documents
            .parse_with(text, || {
                called = true;
                parse_document(Path::new("relocated"), text)
            })
            .unwrap();
        assert!(called);
    }

    #[test]
    fn failed_parses_are_not_retained() {
        let mut documents = ManifestDocuments::default();
        for _ in 0..2 {
            let error = documents.parse(Path::new("manifest"), "[").unwrap_err();
            assert!(error.find_source::<crate::ParseTomlError>().is_some());
        }
        assert!(documents.documents.is_empty());
    }
}
