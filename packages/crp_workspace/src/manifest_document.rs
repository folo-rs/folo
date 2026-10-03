//! Content-addressed TOML interpretation, independent of workspace location.

use std::collections::HashMap;
use std::path::Path;

use crp_diag::Verbose;
use ohno::AppError;
use serde::{Deserialize, Serialize};
use toml_edit::{Datetime, DocumentMut, Item, Table, Value};

use crate::cache::{Cache, CacheEntry};
use crate::manifest::parse_document;

/// Parsed manifest syntax shared by readers of freshly established content.
///
/// Interpretation does not need comments or formatting. Edits still parse the original text
/// to preserve those bytes. Exact string values and TOML shapes remain intact, unlike Cargo's
/// normalized dependency requirements. No filesystem paths or inherited values are stored.
#[derive(Debug, Default)]
pub struct ManifestDocuments {
    storage: Cache,
    documents: HashMap<String, DocumentMut>,
}

impl ManifestDocuments {
    #[must_use]
    pub fn new(storage: Cache) -> Self {
        Self {
            storage,
            documents: HashMap::new(),
        }
    }

    /// Reuses only syntax belonging to the supplied, already acquired complete text.
    #[cfg_attr(test, mutants::skip)] // Storage adapter; parse_with tests memory admission.
    pub fn parse(
        &mut self,
        path: &Path,
        text: &str,
        verbose: Verbose<'_>,
    ) -> Result<DocumentMut, AppError> {
        let storage = self.storage.clone();
        self.parse_with(text, || {
            let key = (env!("CARGO_PKG_VERSION"), text.to_owned());
            let document = storage.get(&key, verbose, || {
                parse_document(path, text).map(|doc| ManifestDocument::from_document(&doc))
            })?;
            Ok(document.into_document())
        })
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

/// Read-only TOML syntax with a stable, type-preserving storage representation.
///
/// `toml_edit` does not serialize its syntax tree. A tagged tree retains inline tables versus
/// tables, array-of-tables, datetime values and floating-point bits; a JSON value projection
/// would conflate them. Rehydration builds nodes directly, never reparsing the source document.
#[derive(Deserialize, Serialize)]
struct ManifestDocument(Vec<(String, Syntax)>);

impl CacheEntry for ManifestDocument {
    const SUBJECT: &'static str = "manifest-document";
    // Bump for syntax interpretation changes within a release; the key isolates releases too.
    const REVISION: u32 = 1;
    type Key = (&'static str, String);
}

impl ManifestDocument {
    fn from_document(document: &DocumentMut) -> Self {
        Self(table_syntax(document.as_table()))
    }

    fn into_document(self) -> DocumentMut {
        table_from_syntax(self.0).into()
    }
}

/// TOML item distinctions needed by manifest and inherited-value interpretation.
#[derive(Deserialize, Serialize)]
enum Syntax {
    Absent,
    Value(Scalar),
    Table(Vec<(String, Self)>),
    ArrayOfTables(Vec<Vec<(String, Self)>>),
}

/// TOML values retain exact strings rather than Cargo-normalized equivalents.
#[derive(Deserialize, Serialize)]
enum Scalar {
    String(String),
    Integer(i64),
    Float(u64),
    Boolean(bool),
    Datetime(Datetime),
    Array(Vec<Self>),
    InlineTable(Vec<(String, Self)>),
}

fn table_syntax(table: &Table) -> Vec<(String, Syntax)> {
    table
        .iter()
        .map(|(key, item)| {
            let item = match item {
                Item::None => Syntax::Absent,
                Item::Value(value) => Syntax::Value(Scalar::from_value(value)),
                Item::Table(table) => Syntax::Table(table_syntax(table)),
                Item::ArrayOfTables(tables) => {
                    Syntax::ArrayOfTables(tables.iter().map(table_syntax).collect())
                }
            };
            (key.to_owned(), item)
        })
        .collect()
}

fn table_from_syntax(entries: Vec<(String, Syntax)>) -> Table {
    entries
        .into_iter()
        .map(|(key, item)| {
            let item = match item {
                Syntax::Absent => Item::None,
                Syntax::Value(value) => Item::Value(value.into_value()),
                Syntax::Table(table) => Item::Table(table_from_syntax(table)),
                Syntax::ArrayOfTables(tables) => {
                    Item::ArrayOfTables(tables.into_iter().map(table_from_syntax).collect())
                }
            };
            (key, item)
        })
        .collect()
}

impl Scalar {
    fn from_value(value: &Value) -> Self {
        match value {
            Value::String(value) => Self::String(value.value().clone()),
            Value::Integer(value) => Self::Integer(*value.value()),
            Value::Float(value) => Self::Float(value.value().to_bits()),
            Value::Boolean(value) => Self::Boolean(*value.value()),
            Value::Datetime(value) => Self::Datetime(*value.value()),
            Value::Array(value) => Self::Array(value.iter().map(Self::from_value).collect()),
            Value::InlineTable(value) => Self::InlineTable(
                value
                    .iter()
                    .map(|(key, value)| (key.to_owned(), Self::from_value(value)))
                    .collect(),
            ),
        }
    }

    fn into_value(self) -> Value {
        match self {
            Self::String(value) => Value::from(value),
            Self::Integer(value) => Value::from(value),
            Self::Float(value) => Value::from(f64::from_bits(value)),
            Self::Boolean(value) => Value::from(value),
            Self::Datetime(value) => Value::from(value),
            Self::Array(value) => Value::Array(value.into_iter().map(Self::into_value).collect()),
            Self::InlineTable(value) => Value::InlineTable(
                value
                    .into_iter()
                    .map(|(key, value)| (key, value.into_value()))
                    .collect(),
            ),
        }
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
        let document = documents
            .parse_with(changed, || parse_document(Path::new("second"), changed))
            .unwrap();
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
            let error = documents
                .parse_with("[", || parse_document(Path::new("manifest"), "["))
                .unwrap_err();
            assert!(error.find_source::<crate::ParseTomlError>().is_some());
        }
        assert!(documents.documents.is_empty());
    }

    #[test]
    fn stored_syntax_preserves_every_toml_shape_without_parsing_source() {
        let text = r#"
strings = ["= 1.2.3", "=1.2.3", "a\nb"]
integer = -42
float = -0.0
nan = nan
infinity = inf
boolean = false
datetime = 1979-05-27T07:32:00Z
date = 1979-05-27
time = 07:32:00
inline = { version = "= 1.2.3", workspace = true }
[workspace]
members = ["a/*"]
[[bin]]
name = "tool"
"#;
        let document = parse_document(Path::new("Cargo.toml"), text).unwrap();
        let syntax = ManifestDocument::from_document(&document);
        let encoded = serde_json::to_string(&syntax).unwrap();
        let decoded: ManifestDocument = serde_json::from_str(&encoded).unwrap();
        let restored = decoded.into_document();
        assert_eq!(
            serde_json::to_string(&ManifestDocument::from_document(&restored)).unwrap(),
            encoded
        );
        assert_eq!(
            restored
                .get("inline")
                .unwrap()
                .get("version")
                .unwrap()
                .as_str(),
            Some("= 1.2.3")
        );
        assert!(restored.get("inline").unwrap().is_inline_table());
        assert!(restored.get("workspace").unwrap().is_table());
        assert!(restored.get("bin").unwrap().is_array_of_tables());
        assert_eq!(
            restored.get("float").unwrap().as_float().unwrap().to_bits(),
            (-0.0_f64).to_bits()
        );
    }
}
