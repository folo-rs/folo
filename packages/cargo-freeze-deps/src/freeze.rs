// Cargo.toml document walker.
//
// Parses a Cargo.toml document with `toml_edit` (so comments and layout are preserved on
// round-trip), walks every standard dependency table, and rewrites each entry's `version`
// string to its frozen form.

use toml_edit::{DocumentMut, Formatted, Item, TableLike, Value};

use crate::version::freeze_requirement;
use crate::{RunError, RunOutcome};

/// Standard dependency tables that can appear at the root of a package-level Cargo.toml.
const PACKAGE_DEP_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// Freezes every floating dependency version in `content` and returns the rewritten document
/// alongside a summary of how many versions were frozen vs left unchanged.
///
/// The returned string has the same comments and overall layout as the input — only the
/// rewritten version literals differ.
pub(crate) fn freeze_document(content: &str) -> Result<(String, RunOutcome), RunError> {
    let mut doc: DocumentMut = content
        .parse()
        .map_err(|e: toml_edit::TomlError| RunError::Parse(e.to_string()))?;

    let mut outcome = RunOutcome {
        frozen_count: 0,
        skipped_count: 0,
    };

    process_package_level(doc.as_table_mut(), &mut outcome)?;
    process_workspace_level(doc.as_table_mut(), &mut outcome)?;
    process_target_level(doc.as_table_mut(), &mut outcome)?;

    Ok((doc.to_string(), outcome))
}

/// Processes `[dependencies]`, `[dev-dependencies]`, and `[build-dependencies]` at the
/// document root (i.e. the package-level dependency tables).
fn process_package_level(
    root: &mut dyn TableLike,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    for table_name in PACKAGE_DEP_TABLES {
        if let Some(table) = root.get_mut(table_name).and_then(Item::as_table_like_mut) {
            freeze_dependency_table(table, outcome)?;
        }
    }
    Ok(())
}

/// Processes `[workspace.dependencies]` if a `[workspace]` table is present.
fn process_workspace_level(
    root: &mut dyn TableLike,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    let Some(workspace) = root.get_mut("workspace").and_then(Item::as_table_like_mut) else {
        return Ok(());
    };

    if let Some(deps) = workspace
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
    {
        freeze_dependency_table(deps, outcome)?;
    }

    Ok(())
}

/// Processes every `[target.<spec>.dependencies]`, `[target.<spec>.dev-dependencies]`, and
/// `[target.<spec>.build-dependencies]` table.
fn process_target_level(
    root: &mut dyn TableLike,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    let Some(target) = root.get_mut("target").and_then(Item::as_table_like_mut) else {
        return Ok(());
    };

    for (_target_spec, target_entry) in target.iter_mut() {
        let Some(target_table) = target_entry.as_table_like_mut() else {
            continue;
        };

        for table_name in PACKAGE_DEP_TABLES {
            if let Some(table) = target_table
                .get_mut(table_name)
                .and_then(Item::as_table_like_mut)
            {
                freeze_dependency_table(table, outcome)?;
            }
        }
    }

    Ok(())
}

/// Freezes every entry in a single dependency table.
fn freeze_dependency_table(
    table: &mut dyn TableLike,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    for (key, entry) in table.iter_mut() {
        let dep_name = key.get().to_string();
        freeze_dependency_entry(&dep_name, entry, outcome)?;
    }
    Ok(())
}

/// Freezes a single dependency entry, which can be:
///
/// 1. A bare string value: `dep = "1.2.3"`.
/// 2. An inline table: `dep = { version = "1.2.3", features = [...] }`.
/// 3. A detached table block: `[dependencies.dep]` followed by `version = "1.2.3"`.
fn freeze_dependency_entry(
    dep_name: &str,
    entry: &mut Item,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    match entry {
        Item::Value(Value::String(s)) => {
            // Form 1: bare string.
            freeze_version_string(dep_name, s, outcome)?;
        }
        Item::Value(Value::InlineTable(t)) => {
            // Form 2: inline table.
            if let Some(version_value) = t.get_mut("version") {
                freeze_version_in_value(dep_name, version_value, outcome)?;
            }
        }
        Item::Table(t) => {
            // Form 3: detached table.
            if let Some(version_item) = t.get_mut("version") {
                freeze_version_in_item(dep_name, version_item, outcome)?;
            }
        }
        // Other forms (array of tables, etc.) are not valid Cargo dependency entries —
        // skip rather than error so unrelated future TOML shapes don't break us.
        _ => {}
    }

    Ok(())
}

/// Freezes the `version` field when accessed as a `toml_edit::Value`.
fn freeze_version_in_value(
    dep_name: &str,
    version_value: &mut Value,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    match version_value {
        Value::String(s) => freeze_version_string(dep_name, s, outcome),
        other => Err(RunError::UnexpectedVersionType {
            dep: dep_name.to_string(),
            actual_type: other.type_name().to_string(),
        }),
    }
}

/// Freezes the `version` field when accessed as a `toml_edit::Item`.
fn freeze_version_in_item(
    dep_name: &str,
    version_item: &mut Item,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    match version_item {
        Item::Value(Value::String(s)) => freeze_version_string(dep_name, s, outcome),
        Item::Value(other) => Err(RunError::UnexpectedVersionType {
            dep: dep_name.to_string(),
            actual_type: other.type_name().to_string(),
        }),
        Item::None => Ok(()),
        Item::Table(_) | Item::ArrayOfTables(_) => Err(RunError::UnexpectedVersionType {
            dep: dep_name.to_string(),
            actual_type: "table".to_string(),
        }),
    }
}

/// Replaces the underlying version literal while preserving the surrounding decor
/// (whitespace and inline comments around the value).
fn freeze_version_string(
    dep_name: &str,
    formatted: &mut Formatted<String>,
    outcome: &mut RunOutcome,
) -> Result<(), RunError> {
    let original = formatted.value().as_str();

    let frozen = freeze_requirement(original).map_err(|source| RunError::InvalidVersion {
        dep: dep_name.to_string(),
        version: original.to_string(),
        source,
    })?;

    let Some(frozen) = frozen else {
        outcome.skipped_count = outcome
            .skipped_count
            .checked_add(1)
            .expect("skipped_count cannot overflow usize for a single Cargo.toml file");
        return Ok(());
    };

    // No-op rewrite (already at the target literal) should not be counted as a change.
    if frozen == original {
        return Ok(());
    }

    // Preserve the surrounding whitespace and comments by cloning the decor and reapplying
    // it to the new value. `Formatted::new` resets decor to default; that's why we explicitly
    // copy it across.
    let decor = formatted.decor().clone();
    let mut replacement = Formatted::new(frozen);
    *replacement.decor_mut() = decor;
    *formatted = replacement;

    outcome.frozen_count = outcome
        .frozen_count
        .checked_add(1)
        .expect("frozen_count cannot overflow usize for a single Cargo.toml file");

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn freeze(content: &str) -> (String, RunOutcome) {
        freeze_document(content).expect("test inputs are expected to be well-formed")
    }

    // -- Dependency entry forms -----------------------------------------------------------

    #[test]
    fn bare_string_form() {
        let input = r#"
[dependencies]
serde = "1.2.3"
"#;
        let expected = r#"
[dependencies]
serde = "=1.2.3"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, expected);
        assert_eq!(outcome.frozen_count, 1);
        assert_eq!(outcome.skipped_count, 0);
    }

    #[test]
    fn inline_table_form() {
        let input = r#"
[dependencies]
serde = { version = "1.2.3", features = ["derive"] }
"#;
        let expected = r#"
[dependencies]
serde = { version = "=1.2.3", features = ["derive"] }
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, expected);
        assert_eq!(outcome.frozen_count, 1);
    }

    #[test]
    fn detached_table_form() {
        let input = r#"
[dependencies.serde]
version = "1.2.3"
features = ["derive"]
"#;
        let expected = r#"
[dependencies.serde]
version = "=1.2.3"
features = ["derive"]
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, expected);
        assert_eq!(outcome.frozen_count, 1);
    }

    // -- Dependency table kinds -----------------------------------------------------------

    #[test]
    fn dev_dependencies_processed() {
        let input = r#"
[dev-dependencies]
mockall = "0.14.0"
"#;
        let expected = r#"
[dev-dependencies]
mockall = "=0.14.0"
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn build_dependencies_processed() {
        let input = r#"
[build-dependencies]
cc = "1.0"
"#;
        let expected = r#"
[build-dependencies]
cc = "=1.0.0"
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn workspace_dependencies_processed() {
        let input = r#"
[workspace.dependencies]
serde = "1.2.3"
"#;
        let expected = r#"
[workspace.dependencies]
serde = "=1.2.3"
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn target_specific_dependencies_processed() {
        let input = r#"
[target."cfg(unix)".dependencies]
libc = "0.2.172"

[target."cfg(windows)".dependencies]
windows = "0.62.0"
"#;
        let expected = r#"
[target."cfg(unix)".dependencies]
libc = "=0.2.172"

[target."cfg(windows)".dependencies]
windows = "=0.62.0"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, expected);
        assert_eq!(outcome.frozen_count, 2);
    }

    #[test]
    fn target_dev_and_build_deps_processed() {
        let input = r#"
[target.'cfg(unix)'.dev-dependencies]
nix = "0.27.0"

[target.'cfg(unix)'.build-dependencies]
cc = "1.0.0"
"#;
        let (_, outcome) = freeze(input);
        assert_eq!(outcome.frozen_count, 2);
    }

    #[test]
    fn detached_target_dependency_table() {
        // `[target.'cfg(unix)'.dependencies.foo]` is the detached form of an entry inside a
        // target-specific dependency table.
        let input = r#"
[target.'cfg(unix)'.dependencies.libc]
version = "0.2.172"
default-features = false
"#;
        let expected = r#"
[target.'cfg(unix)'.dependencies.libc]
version = "=0.2.172"
default-features = false
"#;
        assert_eq!(freeze(input).0, expected);
    }

    // -- Entries without a version field --------------------------------------------------

    #[test]
    fn workspace_inheritance_left_unchanged() {
        let input = "
[dependencies]
serde = { workspace = true }
";
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
        assert_eq!(outcome.skipped_count, 0);
    }

    #[test]
    fn path_only_dependency_left_unchanged() {
        let input = r#"
[dependencies]
my_crate = { path = "../my_crate" }
"#;
        let (output, _) = freeze(input);
        assert_eq!(output, input);
    }

    #[test]
    fn git_only_dependency_left_unchanged() {
        let input = r#"
[dependencies]
my_crate = { git = "https://example.com/repo.git", rev = "deadbeef" }
"#;
        let (output, _) = freeze(input);
        assert_eq!(output, input);
    }

    // -- Mixed sources with version field -------------------------------------------------

    #[test]
    fn path_plus_version_freezes_version() {
        let input = r#"
[dependencies]
my_crate = { path = "../my_crate", version = "0.1.0" }
"#;
        let expected = r#"
[dependencies]
my_crate = { path = "../my_crate", version = "=0.1.0" }
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn git_plus_version_freezes_version() {
        let input = r#"
[dependencies]
my_crate = { git = "https://example.com/repo.git", version = "0.1.0" }
"#;
        let expected = r#"
[dependencies]
my_crate = { git = "https://example.com/repo.git", version = "=0.1.0" }
"#;
        assert_eq!(freeze(input).0, expected);
    }

    // -- Non-freezable versions left as-is, counted as skipped ----------------------------

    #[test]
    fn strict_less_left_unchanged_and_counted() {
        let input = r#"
[dependencies]
serde = "<1.2.3"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
        assert_eq!(outcome.skipped_count, 1);
    }

    #[test]
    fn multi_comparator_left_unchanged_and_counted() {
        let input = r#"
[dependencies]
serde = ">=1.0.0, <2.0.0"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
        assert_eq!(outcome.skipped_count, 1);
    }

    #[test]
    fn bare_star_left_unchanged_and_counted() {
        let input = r#"
[dependencies]
serde = "*"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.skipped_count, 1);
    }

    // -- Already-frozen versions are not counted as new freezes ---------------------------

    #[test]
    fn already_frozen_is_no_op() {
        let input = r#"
[dependencies]
serde = "=1.2.3"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
        assert_eq!(outcome.skipped_count, 0);
    }

    // -- Decor preservation ---------------------------------------------------------------

    #[test]
    fn comments_preserved_above_entry() {
        let input = r#"
[dependencies]
# Important serialization library.
serde = "1.2.3"
# Async runtime.
tokio = "1.48.0"
"#;
        let expected = r#"
[dependencies]
# Important serialization library.
serde = "=1.2.3"
# Async runtime.
tokio = "=1.48.0"
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn suffix_comments_preserved() {
        let input = r#"
[dependencies]
serde = "1.2.3" # pinned for compatibility
"#;
        let expected = r#"
[dependencies]
serde = "=1.2.3" # pinned for compatibility
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn inline_table_suffix_comments_preserved() {
        let input = r#"
[dependencies]
serde = { version = "1.2.3", features = ["derive"] } # the heart of our config
"#;
        let expected = r#"
[dependencies]
serde = { version = "=1.2.3", features = ["derive"] } # the heart of our config
"#;
        assert_eq!(freeze(input).0, expected);
    }

    #[test]
    fn detached_table_comments_preserved() {
        let input = r#"
# Crates we own
[dependencies.my_crate]
# Always pinned to a tested version.
version = "0.1.0"
path = "../my_crate"
"#;
        let expected = r#"
# Crates we own
[dependencies.my_crate]
# Always pinned to a tested version.
version = "=0.1.0"
path = "../my_crate"
"#;
        assert_eq!(freeze(input).0, expected);
    }

    // -- Tables we do NOT process ---------------------------------------------------------

    #[test]
    fn patch_table_left_unchanged() {
        let input = r#"
[patch.crates-io]
serde = "1.2.3"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
        assert_eq!(outcome.skipped_count, 0);
    }

    #[test]
    fn replace_table_left_unchanged() {
        let input = r#"
[replace]
"serde:1.2.3" = { version = "1.2.4" }
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
    }

    #[test]
    fn package_metadata_version_field_left_unchanged() {
        // The package's own `version` field is not a dependency version requirement.
        let input = r#"
[package]
name = "my_crate"
version = "1.2.3"

[dependencies]
serde = "1.2.3"
"#;
        let expected = r#"
[package]
name = "my_crate"
version = "1.2.3"

[dependencies]
serde = "=1.2.3"
"#;
        assert_eq!(freeze(input).0, expected);
    }

    // -- Error cases ----------------------------------------------------------------------

    #[test]
    fn invalid_toml_returns_parse_error() {
        let input = "this is = not [valid toml";
        match freeze_document(input).unwrap_err() {
            RunError::Parse(_) => {}
            other => panic!("expected Parse error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_version_returns_typed_error_with_dep_name() {
        let input = r#"
[dependencies]
serde = "garbage"
"#;
        match freeze_document(input).unwrap_err() {
            RunError::InvalidVersion { dep, version, .. } => {
                assert_eq!(dep, "serde");
                assert_eq!(version, "garbage");
            }
            other => panic!("expected InvalidVersion error, got {other:?}"),
        }
    }

    #[test]
    fn non_string_version_returns_typed_error() {
        let input = "
[dependencies]
serde = { version = 123 }
";
        match freeze_document(input).unwrap_err() {
            RunError::UnexpectedVersionType { dep, .. } => {
                assert_eq!(dep, "serde");
            }
            other => panic!("expected UnexpectedVersionType error, got {other:?}"),
        }
    }

    #[test]
    fn non_string_version_in_detached_table_returns_typed_error() {
        let input = "
[dependencies.serde]
version = 123
";
        match freeze_document(input).unwrap_err() {
            RunError::UnexpectedVersionType { dep, .. } => {
                assert_eq!(dep, "serde");
            }
            other => panic!("expected UnexpectedVersionType error, got {other:?}"),
        }
    }

    // -- Aggregated counts and idempotency ------------------------------------------------

    #[test]
    fn counts_are_accumulated_across_tables() {
        let input = r#"
[dependencies]
serde = "1.2.3"
tokio = "<1.0"

[dev-dependencies]
mockall = "0.14.0"

[workspace.dependencies]
shared = ">1.0, <2.0"
"#;
        let (_, outcome) = freeze(input);
        assert_eq!(outcome.frozen_count, 2);
        assert_eq!(outcome.skipped_count, 2);
    }

    #[test]
    fn idempotent_double_freeze() {
        let input = r#"
[dependencies]
serde = "1.2.3"
tokio = "^1.48"
"#;
        let (once, _) = freeze(input);
        let (twice, second_outcome) = freeze(&once);
        assert_eq!(once, twice);
        // Already frozen; nothing else should be rewritten.
        assert_eq!(second_outcome.frozen_count, 0);
    }

    // -- Other edge cases -----------------------------------------------------------------

    #[test]
    fn empty_file_returns_empty_output() {
        let (output, outcome) = freeze("");
        assert_eq!(output, "");
        assert_eq!(outcome.frozen_count, 0);
    }

    #[test]
    fn file_without_dependency_tables_returns_unchanged() {
        let input = r#"
[package]
name = "lonely"
version = "0.1.0"
"#;
        let (output, outcome) = freeze(input);
        assert_eq!(output, input);
        assert_eq!(outcome.frozen_count, 0);
    }

    #[test]
    fn single_quoted_input_rewritten_with_double_quotes() {
        // Quote-style reset is acceptable: the tool is explicitly opt-in for rewrites and
        // toml_edit's default value formatting uses double quotes.
        let input = "
[dependencies]
serde = '1.2.3'
";
        let expected = r#"
[dependencies]
serde = "=1.2.3"
"#;
        assert_eq!(freeze(input).0, expected);
    }
}
