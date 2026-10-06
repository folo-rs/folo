use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::{env, fs};

use ohno::AppError;

use crate::ReadFileError;
use crate::artifact_path::resolve_path;
use crate::command::run_capture_bytes;
use crate::git::{GitRepo, path_text, strip_terminator};

impl GitRepo {
    /// Resolves repository administration and effective external Git inputs.
    #[cfg_attr(test, mutants::skip)] // Native provenance is covered by report boundary tests.
    pub fn administrative_paths(&self) -> Result<Vec<PathBuf>, AppError> {
        let mut paths = Vec::new();
        for argument in ["--git-dir", "--git-common-dir"] {
            paths.push(self.storage_path(&[argument])?);
        }
        for name in [
            "index",
            "hooks",
            "info/grafts",
            "shallow",
            "info/attributes",
            "info/exclude",
        ] {
            paths.push(self.storage_path(&["--git-path", name])?);
        }
        let objects = self.storage_path(&["--git-path", "objects"])?;
        let alternates = objects.join("info/alternates");
        let environment = env::var_os("GIT_ALTERNATE_OBJECT_DIRECTORIES");
        // Let Git interpret recursive, relative and environment-selected alternates. Avoid
        // count-objects' object enumeration when no alternate source is configured.
        if alternates
            .try_exists()
            .map_err(|error| ReadFileError::caused_by(&alternates, error))?
            || environment.as_ref().is_some_and(|value| !value.is_empty())
        {
            let output = run_capture_bytes(
                "git",
                &["-c", "core.quotePath=true", "count-objects", "-v"],
                &self.root,
            )?;
            let stores = alternate_paths(path_text(&output)?)?;
            // Native Git supplies the finite active store set, but canonicalizes away
            // link entries. Read only these stores' descriptors to retain their input
            // spellings; do not independently traverse an alternate graph.
            let active = stores
                .iter()
                .chain([&objects])
                .map(|store| resolve_path(store))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(environment) = environment {
                let environment = environment.to_str().ok_or_else(InvalidStoragePath::new)?;
                // Git's environment list uses the native search-path separator, with
                // Git C quoting rather than shell quoting around entries containing it.
                let separator = if cfg!(windows) { ';' } else { ':' };
                retain_alternate_entries(environment, separator, &self.root, &active, &mut paths)?;
            }
            for store in stores.iter().chain([&objects]) {
                let descriptor = store.join("info/alternates");
                match fs::read(&descriptor) {
                    Ok(bytes) => retain_alternate_entries(
                        path_text(&bytes)?,
                        '\n',
                        store,
                        &active,
                        &mut paths,
                    )?,
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(ReadFileError::caused_by(&descriptor, error).into());
                    }
                }
                paths.push(descriptor);
            }
            paths.extend(stores);
        }
        paths.push(alternates);
        paths.push(objects);
        let config = run_capture_bytes(
            "git",
            &[
                "config",
                "--null",
                "--show-origin",
                "--name-only",
                "--list",
                "--includes",
            ],
            &self.root,
        )?;
        paths.extend(
            config_paths(&config)?
                .into_iter()
                .map(|path| self.root.join(path)),
        );
        for key in ["core.attributesFile", "core.excludesFile"] {
            let output = run_capture_bytes(
                "git",
                &["config", "--null", "--path", "--default", "", "--get", key],
                &self.root,
            )?;
            let value = output
                .strip_suffix(&[0])
                .ok_or_else(InvalidStoragePath::new)?;
            if !value.is_empty() {
                paths.push(self.root.join(path_text(value)?));
            }
        }
        Ok(paths)
    }

    #[cfg_attr(test, mutants::skip)] // The query retains Git's effective path interpretation.
    fn storage_path(&self, arguments: &[&str]) -> Result<PathBuf, AppError> {
        // Absolute-format rev-parse canonicalizes aliases. Retain Git's input spelling
        // relative to its actual cwd so admission protects both entries and referents.
        let mut command = vec!["rev-parse"];
        command.extend_from_slice(arguments);
        let bytes = run_capture_bytes("git", &command, &self.root)?;
        Ok(self.root.join(strip_terminator(path_text(&bytes)?)))
    }
}

#[cfg_attr(test, mutants::skip)] // Canonical matching consumes native Git's active store set.
fn retain_alternate_entries(
    text: &str,
    separator: char,
    base: &Path,
    active: &[PathBuf],
    paths: &mut Vec<PathBuf>,
) -> Result<(), AppError> {
    for entry in alternate_entries(text, separator)? {
        let path = base.join(entry);
        if active.contains(&resolve_path(&path)?) {
            paths.push(path);
        }
    }
    Ok(())
}

fn alternate_entries(mut text: &str, separator: char) -> Result<Vec<PathBuf>, AppError> {
    // Git's native alternate parser treats NUL as the end of the input buffer.
    text = text.split('\0').next().unwrap_or_default();
    let mut entries = Vec::new();
    while !text.is_empty() {
        let previous_length = text.len();
        if text.starts_with('"') {
            let mut escaped = false;
            let end = text.char_indices().skip(1).find_map(|(index, value)| {
                if escaped {
                    escaped = false;
                } else if value == '\\' {
                    escaped = true;
                } else if value == '"' {
                    return Some(index);
                }
                None
            });
            if let Some(end) = end {
                let (quoted, remainder) = text
                    .split_at_checked(end.checked_add(1).expect("index is inside text"))
                    .expect("the closing ASCII quote ends at a character boundary");
                // Git treats broken C quoting as a literal, unquoted path.
                if let Ok(bytes) = decode_quoted(quoted) {
                    let bytes = bytes.split(|byte| *byte == 0).next().unwrap_or_default();
                    if !bytes.is_empty() {
                        entries.push(PathBuf::from(path_text(bytes)?));
                    }
                    // Git consumes one delimiter byte after a quoted entry.
                    text = if remainder.is_empty() {
                        ""
                    } else {
                        remainder.get(1..).ok_or_else(InvalidStoragePath::new)?
                    };
                    debug_assert!(text.len() < previous_length);
                    continue;
                }
            }
        }
        let (entry, remaining) = text.split_once(separator).unwrap_or((text, ""));
        if !entry.is_empty() && !entry.starts_with('#') {
            entries.push(PathBuf::from(entry));
        }
        text = remaining;
        debug_assert!(text.len() < previous_length);
    }
    Ok(entries)
}

fn alternate_paths(output: &str) -> Result<Vec<PathBuf>, AppError> {
    output
        .split_terminator('\n')
        .filter_map(|line| line.strip_prefix("alternate: "))
        .map(decode_alternate_path)
        .collect()
}

// count-objects uses Git's C quoting, including three-digit octal byte escapes, not JSON.
fn decode_alternate_path(value: &str) -> Result<PathBuf, AppError> {
    if !value.starts_with('"') {
        if value.is_empty() {
            return Err(InvalidStoragePath::new().into());
        }
        return Ok(PathBuf::from(value));
    }
    let decoded = decode_quoted(value)?;
    if decoded.is_empty() || decoded.contains(&0) {
        return Err(InvalidStoragePath::new().into());
    }
    Ok(PathBuf::from(path_text(&decoded)?))
}

fn decode_quoted(value: &str) -> Result<Vec<u8>, InvalidStoragePath> {
    let quoted = value
        .strip_prefix('"')
        .and_then(|quoted| quoted.strip_suffix('"'))
        .ok_or_else(InvalidStoragePath::new)?;
    let mut bytes = quoted.bytes();
    let mut decoded = Vec::new();
    while let Some(byte) = bytes.next() {
        let byte = match byte {
            b'"' => return Err(InvalidStoragePath::new()),
            b'\\' => match bytes.next().ok_or_else(InvalidStoragePath::new)? {
                b'a' => b'\x07',
                b'b' => b'\x08',
                b't' => b'\t',
                b'n' => b'\n',
                b'v' => b'\x0b',
                b'f' => b'\x0c',
                b'r' => b'\r',
                b'\\' => b'\\',
                b'"' => b'"',
                first @ b'0'..=b'3' => {
                    let digits = [
                        first,
                        bytes.next().ok_or_else(InvalidStoragePath::new)?,
                        bytes.next().ok_or_else(InvalidStoragePath::new)?,
                    ];
                    let digits =
                        std::str::from_utf8(&digits).map_err(InvalidStoragePath::caused_by)?;
                    u8::from_str_radix(digits, 8).map_err(InvalidStoragePath::caused_by)?
                }
                _ => return Err(InvalidStoragePath::new()),
            },
            byte => byte,
        };
        decoded.push(byte);
    }
    Ok(decoded)
}

fn config_paths(output: &[u8]) -> Result<Vec<PathBuf>, AppError> {
    let mut fields = output.split_inclusive(|byte| *byte == 0);
    let mut paths = Vec::new();
    while let Some(origin) = fields.next() {
        let origin = origin
            .strip_suffix(&[0])
            .ok_or_else(InvalidStoragePath::new)?;
        let _key = fields
            .next()
            .and_then(|key| key.strip_suffix(&[0]))
            .ok_or_else(InvalidStoragePath::new)?;
        if let Some(path) = origin.strip_prefix(b"file:") {
            paths.push(PathBuf::from(path_text(path)?));
        }
    }
    Ok(paths)
}

/// Git's path output must be decoded without changing filesystem identity.
#[ohno::error]
#[display("Git returned an invalid repository storage path")]
struct InvalidStoragePath;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn alternate_paths_decode_native_git_quoting() {
        assert_eq!(
            alternate_paths(
                "count: 0\nalternate: /plain path \nalternate: \"/a\\n\\t\\\"b\\\\c\\303\\251\"\n"
            )
            .unwrap(),
            [
                PathBuf::from("/plain path "),
                PathBuf::from("/a\n\t\"b\\c\u{e9}")
            ]
        );
        assert_eq!(
            decode_alternate_path(r#""\a\b\v\f\r""#).unwrap(),
            PathBuf::from("\x07\x08\x0b\x0c\r")
        );
        assert!(alternate_paths("count: 1\nsize: 0\n").unwrap().is_empty());
    }

    #[test]
    fn alternate_entries_retain_git_input_spellings() {
        assert_eq!(
            alternate_entries(
                "#comment\n\n../relative\r\n\"/quoted\\npath\"\n\"\"\nlast\0ignored",
                '\n'
            )
            .unwrap(),
            [
                PathBuf::from("../relative\r"),
                PathBuf::from("/quoted\npath"),
                PathBuf::from("last")
            ]
        );
        for separator in [':', ';'] {
            let text = format!(
                "#ignored{separator}\"/embedded{separator}path\"{separator}plain{separator}\"/last\\\"quote\""
            );
            assert_eq!(
                alternate_entries(&text, separator).unwrap(),
                [
                    PathBuf::from(format!("/embedded{separator}path")),
                    PathBuf::from("plain"),
                    PathBuf::from("/last\"quote")
                ]
            );
        }
        assert_eq!(
            alternate_entries("\"\\q\"\n\"/prefix\\000suffix\"", '\n').unwrap(),
            [PathBuf::from("\"\\q\""), PathBuf::from("/prefix")]
        );
        assert_eq!(
            alternate_entries("\"\\1\u{e9}\"", '\n').unwrap(),
            [PathBuf::from("\"\\1\u{e9}\"")]
        );
        assert_eq!(
            alternate_entries("\"unclosed", '\n').unwrap(),
            [PathBuf::from("\"unclosed")]
        );
        assert!(alternate_entries("\"\"\n#end", '\n').unwrap().is_empty());
        alternate_entries("\"\\377\"", '\n').unwrap_err();
        alternate_entries("\"valid\"\u{e9}", '\n').unwrap_err();
    }

    #[test]
    fn malformed_alternate_paths_are_errors() {
        for value in [
            "",
            "\"",
            "\"\"",
            "\"unterminated",
            "\"a\"b\"",
            "\"\\q\"",
            "\"\\1\"",
            "\"\\18x\"",
            "\"\\400\"",
            "\"\\000\"",
            "\"\\377\"",
        ] {
            decode_alternate_path(value).unwrap_err();
        }
    }

    #[test]
    fn configuration_origins_preserve_path_bytes_not_values() {
        assert_eq!(
            config_paths(b"file:/a\nb\0core.hooksPath\0command line:\0x.y\0file:relative\0x.z\0")
                .unwrap(),
            [PathBuf::from("/a\nb"), PathBuf::from("relative")]
        );
        assert!(config_paths(b"").unwrap().is_empty());
        for value in [
            &b"file:path"[..],
            b"file:path\0",
            b"file:path\0key",
            b"file:\xff\0key\0",
        ] {
            config_paths(value).unwrap_err();
        }
    }
}
