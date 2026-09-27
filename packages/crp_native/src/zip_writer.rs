#[cfg(any(test, feature = "private-test-util"))]
use std::io::Cursor;
use std::io::{BufRead, BufReader, Read, Seek, Write};

use flate2::Compression;
use ohno::AppError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

/// Writes one root executable with bounded buffering and cooperative interruption.
pub(crate) fn write_archive(
    input: impl Read,
    output: impl Write + Seek,
    name: &str,
    size: u64,
    permissions: Option<u32>,
    mut checkpoint: impl FnMut() -> Result<(), AppError>,
) -> Result<(), AppError> {
    checkpoint()?;
    let mut options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(i64::from(Compression::default().level())))
        // Source filesystem timestamps are not part of the release asset identity.
        .last_modified_time(DateTime::default())
        .large_file(size >= u64::from(u32::MAX));
    if let Some(permissions) = permissions {
        options = options.unix_permissions(permissions);
    }
    let mut archive = ZipWriter::new(output);
    archive.start_file(name, options)?;
    let mut input = BufReader::new(input);
    loop {
        // Do not use an uninterrupted io::copy: compression must share the item's
        // cancellation/deadline checks without retaining the executable in memory.
        checkpoint()?;
        let bytes = input.fill_buf()?;
        if bytes.is_empty() {
            break;
        }
        archive.write_all(bytes)?;
        let length = bytes.len();
        input.consume(length);
    }
    checkpoint()?;
    let mut output = archive.finish()?;
    output.flush()?;
    checkpoint()
}

/// Exercises the production writer without filesystem or clock noise.
#[cfg(any(test, feature = "private-test-util"))]
pub fn benchmark_archive(contents: &[u8]) -> Result<Vec<u8>, AppError> {
    let mut output = Cursor::new(Vec::new());
    write_archive(
        contents,
        &mut output,
        "benchmark",
        u64::try_from(contents.len())?,
        None,
        || Ok(()),
    )?;
    Ok(output.into_inner())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::io::{self, Cursor, SeekFrom};

    use zip::ZipArchive;

    use super::*;

    #[test]
    fn in_memory_driver_writes_repeatable_metadata_and_contents() {
        let contents = b"archive benchmark input";
        let first = benchmark_archive(contents).unwrap();
        assert_eq!(first, benchmark_archive(contents).unwrap());
        let mut archive = ZipArchive::new(Cursor::new(first)).unwrap();
        let mut entry = archive.by_name("benchmark").unwrap();
        let mut restored = Vec::new();
        entry.read_to_end(&mut restored).unwrap();
        assert_eq!(restored, contents);
    }

    #[test]
    fn streams_deflated_root_file_with_explicit_permissions() {
        // Cross several reader buffers without turning this into a large compression test.
        let contents = b"representative executable bytes\n".repeat(600);
        let mut output = Cursor::new(Vec::new());
        let mut checkpoints = 0;
        write_archive(
            contents.as_slice(),
            &mut output,
            "program with spaces",
            u64::try_from(contents.len()).unwrap(),
            Some(0o751),
            || {
                checkpoints += 1;
                Ok(())
            },
        )
        .unwrap();
        assert!(checkpoints > 3);
        let mut archive = ZipArchive::new(Cursor::new(output.into_inner())).unwrap();
        assert_eq!(archive.len(), 1);
        let mut entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name(), "program with spaces");
        assert_eq!(entry.compression(), CompressionMethod::Deflated);
        assert_eq!(entry.unix_mode().unwrap() & 0o777, 0o751);
        let mut restored = Vec::new();
        entry.read_to_end(&mut restored).unwrap();
        assert_eq!(restored, contents);
    }

    #[test]
    fn empty_and_zip64_entries_are_readable() {
        for declared_size in [0, u64::from(u32::MAX)] {
            let mut output = Cursor::new(Vec::new());
            write_archive(
                io::empty(),
                &mut output,
                "program.exe",
                declared_size,
                None,
                || Ok(()),
            )
            .unwrap();
            let mut archive = ZipArchive::new(Cursor::new(output.into_inner())).unwrap();
            let entry = archive.by_index(0).unwrap();
            assert_eq!(entry.size(), 0);
            assert_eq!(entry.name(), "program.exe");
        }
    }

    #[test]
    fn interruption_is_observed_before_reading_and_between_chunks() {
        let read = Cell::new(false);
        let mut input = TrackingReader { read: &read };
        let mut output = Cursor::new(Vec::new());
        write_archive(&mut input, &mut output, "program", 1, None, || {
            Err(io::Error::other("cancelled").into())
        })
        .unwrap_err();
        assert!(!read.get());
        assert!(output.get_ref().is_empty());

        write_archive(&mut input, &mut output, "program", 1, None, || {
            if read.get() {
                Err(io::Error::other("deadline").into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(read.get());
        assert!(!output.get_ref().is_empty());
    }

    #[test]
    fn propagates_read_write_seek_and_flush_failures() {
        for failure in [Failure::Write, Failure::Seek, Failure::Flush] {
            write_archive(
                b"payload".as_slice(),
                FailingWriter { failure },
                "program",
                7,
                None,
                || Ok(()),
            )
            .unwrap_err();
        }
        write_archive(
            FailingReader,
            Cursor::new(Vec::new()),
            "program",
            0,
            None,
            || Ok(()),
        )
        .unwrap_err();
    }

    /// Supplies input until its observer requests interruption, without a real clock.
    struct TrackingReader<'a> {
        read: &'a Cell<bool>,
    }

    impl Read for TrackingReader<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            // A second read means the intervening interruption check was lost.
            assert!(!self.read.replace(true));
            *buf.first_mut().unwrap() = 42;
            Ok(1)
        }
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("read"))
        }
    }

    enum Failure {
        Write,
        Seek,
        Flush,
    }

    /// Separates output failure paths without using the filesystem.
    struct FailingWriter {
        failure: Failure,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if matches!(self.failure, Failure::Write) {
                Err(io::Error::other("write"))
            } else {
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            if matches!(self.failure, Failure::Flush) {
                Err(io::Error::other("flush"))
            } else {
                Ok(())
            }
        }
    }

    impl Seek for FailingWriter {
        fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
            if matches!(self.failure, Failure::Seek) {
                Err(io::Error::other("seek"))
            } else {
                Ok(0)
            }
        }
    }
}
