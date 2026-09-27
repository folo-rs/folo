use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ohno::AppError;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::command::{cancelled, interruption};
use crate::request::BuildRequest;
use crate::zip_writer::write_archive;

/// Fresh per-item staging keeps shared target artifacts separate from release asset contents.
pub(crate) struct Staging {
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
            executable: staged,
        })
    }

    // Filesystem and real-clock effects are covered by native integration tests.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn package(&self, deadline: Instant) -> Result<(), AppError> {
        let mut checkpoint = || {
            if let Some(reason) = interruption(Instant::now() >= deadline, cancelled()) {
                Err(ArchiveFailed::new(self.archive.clone(), reason).into())
            } else {
                Ok(())
            }
        };
        checkpoint()?;
        self.write_archive(&mut checkpoint)
            .map_err(|error| contextualize_archive_error(&self.archive, error))?;
        self.checksum(&mut checkpoint)
    }

    #[cfg_attr(test, mutants::skip)]
    fn write_archive(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let input = File::open(&self.executable)?;
        let metadata = input.metadata()?;
        #[cfg(unix)]
        let permissions = {
            use std::os::unix::fs::PermissionsExt;
            Some(metadata.permissions().mode())
        };
        #[cfg(not(unix))]
        let permissions = None;
        let directory = self.archive.parent().ok_or_else(|| {
            ArchiveFailed::new(self.archive.clone(), "Archive has no parent directory")
        })?;
        // ZipWriter may finalize partial contents on drop. Keep those bytes private
        // until explicit finalization succeeds, without replacing an existing asset.
        let mut temporary = NamedTempFile::new_in(directory)?;
        let output = BufWriter::new(temporary.as_file_mut());
        write_archive(
            input,
            output,
            archive_name(&self.executable)?,
            metadata.len(),
            permissions,
            checkpoint,
        )?;
        temporary
            .persist_noclobber(&self.archive)
            .map_err(|error| {
                ArchiveFailed::caused_by(
                    self.archive.clone(),
                    "Cannot promote completed ZIP archive",
                    error.error,
                )
            })?;
        Ok(())
    }

    // SHA-256 is provided by sha2; only filesystem serialization lives at this boundary.
    #[cfg_attr(test, mutants::skip)]
    fn checksum(
        &self,
        checkpoint: &mut impl FnMut() -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        let mut archive = BufReader::new(File::open(&self.archive).map_err(|error| {
            ArchiveFailed::caused_by(
                self.archive.clone(),
                "Cannot open archive for hashing",
                error,
            )
        })?);
        let mut hash = Sha256::new();
        loop {
            checkpoint()?;
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
        checkpoint()?;
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

/// Drives the real file-promotion boundary with an in-process interruption source.
#[cfg(any(test, feature = "private-test-util"))]
pub fn write_archive_for_test(
    executable: &Path,
    archive: &Path,
    mut checkpoint: impl FnMut() -> Result<(), AppError>,
) -> Result<(), AppError> {
    Staging {
        executable: executable.to_owned(),
        archive: archive.to_owned(),
        checksum: archive.with_extension("sha256"),
    }
    .write_archive(&mut checkpoint)
}

fn contextualize_archive_error(path: &Path, error: AppError) -> AppError {
    if error.find_source::<ArchiveFailed>().is_some() {
        error
    } else {
        ArchiveFailed::caused_by(path.to_path_buf(), "Cannot write ZIP archive", error).into()
    }
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
    use std::io;

    use super::*;

    #[test]
    fn archive_interruption_keeps_its_existing_context() {
        let error: AppError =
            ArchiveFailed::new(PathBuf::from("output.zip"), "batch cancelled").into();
        let error = contextualize_archive_error(Path::new("output.zip"), error);
        assert_eq!(
            error.find_source::<ArchiveFailed>().unwrap().message,
            "batch cancelled"
        );
        let error = contextualize_archive_error(
            Path::new("output.zip"),
            io::Error::other("write failure").into(),
        );
        assert_eq!(
            error.find_source::<ArchiveFailed>().unwrap().path,
            Path::new("output.zip")
        );
        assert!(error.find_source::<io::Error>().is_some());
    }

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
