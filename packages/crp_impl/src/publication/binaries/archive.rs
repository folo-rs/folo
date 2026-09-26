use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use ohno::AppError;
use sha2::{Digest, Sha256};

use crate::publication::binaries::model::{Binary, InvalidPlan};

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
        binary: &Binary,
        triple: &str,
        executable: &Path,
    ) -> Result<Self, AppError> {
        let directory = output.join(binary.archive_base(triple));
        fs::create_dir(&directory)?;
        let name = if triple.contains("-windows-") {
            format!("{}.exe", binary.bin)
        } else {
            binary.bin.clone()
        };
        let staged = directory.join(name);
        if !fs::metadata(executable)?.is_file() {
            return Err(
                InvalidPlan::new("Cargo executable is not a regular file".to_owned()).into(),
            );
        }
        fs::copy(executable, &staged)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&staged)?.permissions().mode() & 0o111 == 0 {
                return Err(InvalidPlan::new(
                    "Staged binary has no executable permission".to_owned(),
                )
                .into());
            }
        }
        let base = binary.archive_base(triple);
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
        let mut archive = BufReader::new(File::open(&self.archive)?);
        let mut hash = Sha256::new();
        loop {
            let bytes = archive.fill_buf()?;
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
        let name = self
            .archive
            .file_name()
            .ok_or_else(|| InvalidPlan::new("Archive has no filename".to_owned()))?
            .to_str()
            .ok_or_else(|| InvalidPlan::new("Archive name is not UTF-8".to_owned()))?;
        let text = checksum_line(&digest, name, cfg!(windows));
        File::create_new(&self.checksum)?.write_all(text.as_bytes())?;
        Ok(())
    }
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
}
