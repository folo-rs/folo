use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use ohno::AppError;
use sha2::{Digest, Sha256};

use crate::request::BuildRequest;

/// Fresh per-item staging keeps shared target artifacts separate from release asset contents.
pub(crate) struct Staging {
    pub(crate) directory: PathBuf,
    pub(crate) executable: PathBuf,
    pub(crate) archive: PathBuf,
    pub(crate) checksum: PathBuf,
}

impl Staging {
    // Filesystem effects are exercised by the no-upload integration fixture.
    #[cfg_attr(test, mutants::skip)]
    #[expect(
        clippy::create_dir,
        reason = "Staging must fail on existing output rather than reuse stale files"
    )]
    pub(crate) fn create(
        output: &Path,
        binary: &BuildRequest,
        triple: &str,
        executable: &Path,
    ) -> Result<Self, AppError> {
        let directory = output.join(&binary.archive_base);
        fs::create_dir(&directory).map_err(|error| {
            ArchiveFailed::caused_by(directory.clone(), "Cannot create artifact staging", error)
        })?;
        let name = if triple.contains("-windows-") {
            format!("{}.exe", binary.bin)
        } else {
            binary.bin.clone()
        };
        let staged = directory.join(name);
        let metadata = fs::metadata(executable).map_err(|error| {
            ArchiveFailed::caused_by(
                executable.to_path_buf(),
                "Cannot inspect Cargo executable",
                error,
            )
        })?;
        if !metadata.is_file() {
            return Err(ArchiveFailed::new(
                executable.to_path_buf(),
                "Cargo executable is not a regular file",
            )
            .into());
        }
        fs::copy(executable, &staged).map_err(|error| {
            ArchiveFailed::caused_by(staged.clone(), "Cannot stage Cargo executable", error)
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&staged).map_err(|error| {
                ArchiveFailed::caused_by(staged.clone(), "Cannot inspect staged permissions", error)
            })?;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(ArchiveFailed::new(
                    staged,
                    "Staged binary has no executable permission",
                )
                .into());
            }
        }
        let base = &binary.archive_base;
        Ok(Self {
            archive: directory.join(format!("{base}.zip")),
            checksum: directory.join(format!("{base}.sha256")),
            directory,
            executable: staged,
        })
    }

    // SHA-256 is provided by sha2; only filesystem serialization lives at this boundary.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn checksum(&self) -> Result<(), AppError> {
        let mut archive = BufReader::new(File::open(&self.archive).map_err(|error| {
            ArchiveFailed::caused_by(
                self.archive.clone(),
                "Cannot open archive for hashing",
                error,
            )
        })?);
        let mut hash = Sha256::new();
        loop {
            let bytes = archive.fill_buf().map_err(|error| {
                ArchiveFailed::caused_by(
                    self.archive.clone(),
                    "Cannot read archive for hashing",
                    error,
                )
            })?;
            if bytes.is_empty() {
                break;
            }
            hash.update(bytes);
            let length = bytes.len();
            archive.consume(length);
        }
        let mut digest = String::new();
        for byte in hash.finalize() {
            write!(digest, "{byte:02x}")?;
        }
        let name = archive_name(&self.archive)?;
        let text = checksum_line(&digest, name, cfg!(windows));
        File::create_new(&self.checksum)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .map_err(|error| {
                ArchiveFailed::caused_by(
                    self.checksum.clone(),
                    "Cannot write archive checksum",
                    error,
                )
            })?;
        Ok(())
    }
}

/// Archive staging and checksum failures retain their artifact path and foreign cause.
#[ohno::error]
#[display("{message}: {}", path.display())]
struct ArchiveFailed {
    path: PathBuf,
    message: &'static str,
}

fn archive_name(path: &Path) -> Result<&str, AppError> {
    path.file_name()
        .ok_or_else(|| ArchiveFailed::new(path.to_path_buf(), "Archive has no filename"))?
        .to_str()
        .ok_or_else(|| ArchiveFailed::new(path.to_path_buf(), "Archive name is not UTF-8").into())
}

fn checksum_line(digest: &str, name: &str, windows: bool) -> String {
    // Match sha256sum's text/binary markers; either is accepted by checksum consumers.
    format!("{digest} {}{name}\n", if windows { "*" } else { " " })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn checksum_sidecar_uses_bare_name_and_lf_without_bom() {
        assert_eq!(checksum_line("abc", "tool.zip", false), "abc  tool.zip\n");
        assert_eq!(checksum_line("abc", "tool.zip", true), "abc *tool.zip\n");
    }

    #[test]
    fn invalid_archive_paths_have_artifact_context_not_plan_errors() {
        let error = archive_name(Path::new("")).unwrap_err();
        assert_eq!(
            error.find_source::<ArchiveFailed>().unwrap().path,
            Path::new("")
        );
        assert_eq!(
            archive_name(Path::new("output/archive.zip")).unwrap(),
            "archive.zip"
        );
    }
}
