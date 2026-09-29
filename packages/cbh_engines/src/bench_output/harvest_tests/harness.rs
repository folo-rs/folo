use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::future::ready;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, SystemTime};

use cbh_diag::RecordingReporter;
use cbh_model::Engine;

use crate::bench_output::{
    ALL_THE_TIME_DIR, ALLOC_TRACKER_DIR, CRITERION_DIR, FsBenchOutputSource, GUNGRAUN_DIR, Harvest,
};
use crate::output_files::{EntryType, OutputDirectory, OutputEntry, OutputFiles};

/// A finite script of expected native operations, consumed to detect extra reads and rescans.
#[derive(Default)]
pub(crate) struct FakeFiles {
    pub(crate) directories: RefCell<BTreeMap<PathBuf, io::Result<Directory>>>,
    pub(crate) contents: RefCell<BTreeMap<PathBuf, io::Result<String>>>,
}

impl FakeFiles {
    pub(crate) fn directory(&mut self, relative: &str, entries: impl IntoIterator<Item = Entry>) {
        assert!(
            self.directories
                .get_mut()
                .insert(
                    path(relative),
                    Ok(Directory(entries.into_iter().map(Ok).collect())),
                )
                .is_none()
        );
    }

    pub(crate) fn file(&mut self, relative: &str, content: &str, modified: SystemTime) -> Entry {
        assert!(
            self.contents
                .get_mut()
                .insert(path(relative), Ok(content.into()))
                .is_none()
        );
        Entry::file(relative, modified)
    }

    pub(crate) fn assert_consumed(&self) {
        assert!(self.directories.borrow().is_empty());
        assert!(self.contents.borrow().is_empty());
    }
}

impl OutputFiles for FakeFiles {
    type Directory = Directory;

    fn read_dir(&self, path: &Path) -> impl Future<Output = io::Result<Directory>> {
        ready(self.directories.borrow_mut().remove(path).unwrap())
    }

    fn read_to_string(&self, path: &Path) -> impl Future<Output = io::Result<String>> {
        ready(self.contents.borrow_mut().remove(path).unwrap())
    }
}

/// Scripted directory observations, including failures during iteration.
pub(crate) struct Directory(pub(crate) VecDeque<io::Result<Entry>>);

impl OutputDirectory for Directory {
    type Entry = Entry;

    fn next_entry(&mut self) -> impl Future<Output = io::Result<Option<Entry>>> {
        ready(self.0.pop_front().transpose())
    }
}

/// Entry facts with independent type and metadata failures.
pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    pub(crate) file_type: Result<EntryType, io::ErrorKind>,
    pub(crate) modified: Result<SystemTime, io::ErrorKind>,
}

impl Entry {
    pub(crate) fn file(relative: &str, modified: SystemTime) -> Self {
        Self {
            modified: Ok(modified),
            ..Self::ignored(relative)
        }
    }

    pub(crate) fn ignored(relative: &str) -> Self {
        Self {
            path: path(relative),
            file_type: Ok(EntryType::File),
            // Irrelevant files must not trigger metadata acquisition.
            modified: Err(io::ErrorKind::Unsupported),
        }
    }

    pub(crate) fn directory(relative: &str) -> Self {
        Self {
            file_type: Ok(EntryType::Directory),
            ..Self::ignored(relative)
        }
    }

    pub(crate) fn other(relative: &str) -> Self {
        Self {
            file_type: Ok(EntryType::Other),
            ..Self::ignored(relative)
        }
    }
}

impl OutputEntry for Entry {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }

    fn file_type(&self) -> impl Future<Output = io::Result<EntryType>> {
        ready(self.file_type.map_err(io::Error::from))
    }

    fn modified(&self) -> impl Future<Output = io::Result<SystemTime>> {
        ready(self.modified.map_err(io::Error::from))
    }
}

pub(crate) fn engines() -> [Engine; 4] {
    [
        Engine::Callgrind,
        Engine::Criterion,
        Engine::AllocTracker,
        Engine::AllTheTime,
    ]
}

pub(crate) fn engine_dir(engine: Engine) -> &'static str {
    match engine {
        Engine::Callgrind => GUNGRAUN_DIR,
        Engine::Criterion => CRITERION_DIR,
        Engine::AllocTracker => ALLOC_TRACKER_DIR,
        Engine::AllTheTime => ALL_THE_TIME_DIR,
    }
}

pub(crate) fn path(relative: &str) -> PathBuf {
    Path::new("target").join(relative)
}

pub(crate) fn boundary() -> SystemTime {
    // An arbitrary fixed boundary beyond the slack, with no wall-clock dependency.
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(100))
        .unwrap()
}

pub(crate) fn harvest_len(harvest: Harvest) -> usize {
    match harvest {
        Harvest::Callgrind(items) => items.len(),
        Harvest::Criterion(items) => items.len(),
        Harvest::AllocTracker(items) | Harvest::AllTheTime(items) => items.len(),
    }
}

pub(crate) fn collect(
    files: &FakeFiles,
    engine: Engine,
    since: Option<SystemTime>,
) -> io::Result<Harvest> {
    let source = FsBenchOutputSource::new("target");
    let reporter = RecordingReporter::new();
    // Every fake observation is immediately ready. One poll proves completion without a
    // runtime, a real clock or a polling loop that could hang under mutation testing.
    let Poll::Ready(result) = pin!(source.collect_with(files, engine, since, &reporter))
        .poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("in-memory acquisition must complete immediately");
    };
    result
}

pub(crate) fn candidate(engine: Engine, modified: SystemTime, included: bool) -> FakeFiles {
    let mut files = FakeFiles::default();
    let root = engine_dir(engine);
    let relative = match engine {
        Engine::Callgrind => format!("{root}/summary.json"),
        Engine::Criterion => format!("{root}/new/estimates.json"),
        Engine::AllocTracker | Engine::AllTheTime => format!("{root}/operation.json"),
    };
    let entry = Entry::file(&relative, modified);
    if included {
        files
            .contents
            .get_mut()
            .insert(path(&relative), Ok("contents".into()));
    }
    if engine == Engine::Criterion {
        files.directory(root, [Entry::directory("criterion/new")]);
        // Benchmark metadata is irrelevant: only estimates determines freshness.
        files.directory(
            "criterion/new",
            [Entry::ignored("criterion/new/benchmark.json"), entry],
        );
        if included {
            files
                .contents
                .get_mut()
                .insert(path("criterion/new/benchmark.json"), Ok("identity".into()));
        }
    } else {
        files.directory(root, [entry]);
    }
    files
}
