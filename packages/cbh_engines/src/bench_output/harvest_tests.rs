use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::future::ready;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use cbh_diag::RecordingReporter;

use super::*;

#[test]
fn callgrind_recurses_and_sorts_paths_with_their_contents() {
    let mut files = FakeFiles::default();
    let first = files.file("gungraun/a/summary.json", "first", boundary());
    let mut second = files.file("gungraun/z/nested/summary.json", "second", boundary());
    // Recursive collectors accept a matching non-directory, including a readable file link.
    second.file_type = Ok(EntryType::Other);
    files.directory(
        "gungraun",
        [
            Entry::directory("gungraun/a"),
            Entry::directory("gungraun/z"),
            Entry::ignored("gungraun/other.json"),
            Entry::other("gungraun/linked_group"),
        ],
    );
    files.directory("gungraun/a", [first]);
    files.directory("gungraun/z", [Entry::directory("gungraun/z/nested")]);
    files.directory("gungraun/z/nested", [second]);

    assert_eq!(
        collect(&files, Engine::Callgrind, Some(boundary())).unwrap(),
        Harvest::Callgrind(vec![
            RawSummary {
                path: path("gungraun/a/summary.json"),
                content: "first".into(),
            },
            RawSummary {
                path: path("gungraun/z/nested/summary.json"),
                content: "second".into(),
            },
        ])
    );
    files.assert_consumed();
}

#[test]
fn criterion_pairs_only_complete_new_directories_and_sorts_cases() {
    let mut files = FakeFiles::default();
    let a_benchmark = files.file("criterion/a/new/benchmark.json", "a-id", boundary());
    let a_estimates = files.file("criterion/a/new/estimates.json", "a-est", boundary());
    let mut z_benchmark = files.file("criterion/z/new/benchmark.json", "z-id", boundary());
    z_benchmark.file_type = Ok(EntryType::Other);
    let z_estimates = files.file("criterion/z/new/estimates.json", "z-est", boundary());
    files.directory(
        "criterion",
        [
            Entry::directory("criterion/a"),
            Entry::directory("criterion/z"),
            Entry::other("criterion/linked_group"),
        ],
    );
    files.directory(
        "criterion/a",
        [
            Entry::directory("criterion/a/new"),
            Entry::directory("criterion/a/base"),
        ],
    );
    files.directory("criterion/z", [Entry::directory("criterion/z/new")]);
    files.directory("criterion/a/new", [a_estimates, a_benchmark]);
    files.directory(
        "criterion/z/new",
        [
            z_benchmark,
            Entry::ignored("criterion/z/new/notes.txt"),
            z_estimates,
        ],
    );
    // Complete base output is structural, not a current case; contents must not be read.
    files.directory(
        "criterion/a/base",
        [
            Entry::file("criterion/a/base/benchmark.json", boundary()),
            Entry::file("criterion/a/base/estimates.json", boundary()),
        ],
    );

    assert_eq!(
        collect(&files, Engine::Criterion, None).unwrap(),
        Harvest::Criterion(vec![
            RawCriterionCase {
                dir: path("criterion/a/new"),
                benchmark: "a-id".into(),
                estimates: "a-est".into(),
            },
            RawCriterionCase {
                dir: path("criterion/z/new"),
                benchmark: "z-id".into(),
                estimates: "z-est".into(),
            },
        ])
    );
    files.assert_consumed();
}

#[test]
fn criterion_requires_both_files_and_does_not_treat_directories_as_files() {
    for entries in [
        vec![],
        vec![Entry::ignored("criterion/new/benchmark.json")],
        vec![Entry::file("criterion/new/estimates.json", boundary())],
        vec![Entry::ignored("criterion/new/other.json")],
        vec![
            Entry::directory("criterion/new/benchmark.json"),
            Entry::file("criterion/new/estimates.json", boundary()),
        ],
    ] {
        let mut files = FakeFiles::default();
        files.directory("criterion", [Entry::directory("criterion/new")]);
        for entry in &entries {
            if entry.file_type.unwrap() == EntryType::Directory {
                files
                    .directories
                    .get_mut()
                    .insert(entry.path.clone(), Ok(Directory(VecDeque::new())));
            }
        }
        files.directory("criterion/new", entries);
        assert_eq!(
            collect(&files, Engine::Criterion, Some(boundary())).unwrap(),
            Harvest::Criterion(vec![])
        );
        files.assert_consumed();
    }
}

#[test]
fn flat_engines_select_regular_top_level_json_and_preserve_engine_shape() {
    for engine in [Engine::AllocTracker, Engine::AllTheTime] {
        let root = engine_dir(engine);
        let mut files = FakeFiles::default();
        let z = files.file(&format!("{root}/z.json"), "last", boundary());
        let a = files.file(&format!("{root}/a.json"), "first", boundary());
        let other = Entry::other(&format!("{root}/link.json"));
        files.directory(
            root,
            [
                z,
                Entry::directory(&format!("{root}/nested.json")),
                Entry::ignored(&format!("{root}/notes.txt")),
                other,
                a,
            ],
        );
        let expected = vec![
            RawOperationFile {
                path: path(&format!("{root}/a.json")),
                content: "first".into(),
            },
            RawOperationFile {
                path: path(&format!("{root}/z.json")),
                content: "last".into(),
            },
        ];
        let expected = match engine {
            Engine::AllocTracker => Harvest::AllocTracker(expected),
            Engine::AllTheTime => Harvest::AllTheTime(expected),
            _ => unreachable!(),
        };
        assert_eq!(collect(&files, engine, None).unwrap(), expected);
        files.assert_consumed();
    }
}

#[test]
fn recursive_freshness_includes_the_cutoff_and_newer_output_but_not_older_output() {
    assert_freshness([Engine::Callgrind, Engine::Criterion]);
}

#[test]
fn flat_freshness_includes_the_cutoff_and_newer_output_but_not_older_output() {
    assert_freshness([Engine::AllocTracker, Engine::AllTheTime]);
}

fn assert_freshness(engines: [Engine; 2]) {
    // Separate collector families keep the per-test Miri workload bounded.
    for engine in engines {
        let cutoff = boundary().checked_sub(MTIME_SLACK).unwrap();
        for (modified, included) in [
            (cutoff.checked_sub(Duration::from_secs(1)).unwrap(), false),
            (cutoff, true),
            (cutoff.checked_add(Duration::from_secs(1)).unwrap(), true),
        ] {
            let files = candidate(engine, modified, included);
            assert_eq!(
                harvest_len(collect(&files, engine, Some(boundary())).unwrap()),
                usize::from(included)
            );
            files.assert_consumed();
        }
    }
}

#[test]
fn disabled_freshness_admits_old_output_for_every_engine() {
    for engine in engines() {
        let files = candidate(engine, SystemTime::UNIX_EPOCH, true);
        assert_eq!(harvest_len(collect(&files, engine, None).unwrap()), 1);
        files.assert_consumed();
    }
}

#[test]
fn missing_roots_are_empty_but_other_directory_errors_propagate() {
    for engine in engines() {
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
            let mut files = FakeFiles::default();
            files
                .directories
                .get_mut()
                .insert(path(engine_dir(engine)), Err(kind.into()));
            let result = collect(&files, engine, None);
            if kind == io::ErrorKind::NotFound {
                assert_eq!(harvest_len(result.unwrap()), 0);
            } else {
                assert_eq!(result.unwrap_err().kind(), kind);
            }
            files.assert_consumed();
        }
    }
}

#[test]
fn recursive_collectors_skip_disappeared_children_but_propagate_other_scan_errors() {
    for engine in [Engine::Callgrind, Engine::Criterion] {
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
            let mut files = candidate(engine, boundary(), true);
            let root = path(engine_dir(engine));
            let child = root.join("vanished");
            let directory = files
                .directories
                .get_mut()
                .get_mut(&root)
                .unwrap()
                .as_mut()
                .unwrap();
            directory.0.push_back(Ok(Entry {
                path: child.clone(),
                ..Entry::directory("unused")
            }));
            files.directories.get_mut().insert(child, Err(kind.into()));
            let result = collect(&files, engine, None);
            if kind == io::ErrorKind::NotFound {
                assert_eq!(harvest_len(result.unwrap()), 1);
                files.assert_consumed();
            } else {
                assert_eq!(result.unwrap_err().kind(), kind);
            }
        }
    }
}

#[test]
fn entry_iteration_errors_propagate() {
    entry_errors_propagate("next_entry");
}

#[test]
fn entry_type_errors_propagate() {
    entry_errors_propagate("file_type");
}

#[test]
fn entry_metadata_errors_propagate() {
    entry_errors_propagate("modified");
}

#[test]
fn entry_contents_errors_propagate() {
    entry_errors_propagate("read_to_string");
}

fn entry_errors_propagate(operation: &str) {
    for engine in engines() {
        // NotFound is optional only for read_dir, never for a selected entry or its contents.
        for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
            let mut files = candidate(engine, boundary(), true);
            let directory_path = if engine == Engine::Criterion {
                path("criterion/new")
            } else {
                path(engine_dir(engine))
            };
            let entries = &mut files
                .directories
                .get_mut()
                .get_mut(&directory_path)
                .unwrap()
                .as_mut()
                .unwrap()
                .0;
            // Criterion freshness belongs to estimates, never benchmark.json.
            let entry = entries.back_mut().unwrap().as_mut().unwrap();
            match operation {
                "next_entry" => entries.push_front(Err(kind.into())),
                "file_type" => entry.file_type = Err(kind),
                "modified" => entry.modified = Err(kind),
                "read_to_string" => {
                    files
                        .contents
                        .get_mut()
                        .insert(entry.path.clone(), Err(kind.into()));
                }
                _ => unreachable!(),
            }
            assert_eq!(collect(&files, engine, None).unwrap_err().kind(), kind);
        }
    }
}

#[test]
fn criterion_benchmark_read_errors_propagate() {
    let mut files = candidate(Engine::Criterion, boundary(), true);
    files.contents.get_mut().insert(
        path("criterion/new/benchmark.json"),
        Err(io::ErrorKind::InvalidData.into()),
    );
    assert_eq!(
        collect(&files, Engine::Criterion, None).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

fn engines() -> [Engine; 4] {
    [
        Engine::Callgrind,
        Engine::Criterion,
        Engine::AllocTracker,
        Engine::AllTheTime,
    ]
}

fn engine_dir(engine: Engine) -> &'static str {
    match engine {
        Engine::Callgrind => GUNGRAUN_DIR,
        Engine::Criterion => CRITERION_DIR,
        Engine::AllocTracker => ALLOC_TRACKER_DIR,
        Engine::AllTheTime => ALL_THE_TIME_DIR,
    }
}

fn path(relative: &str) -> PathBuf {
    Path::new("target").join(relative)
}

fn boundary() -> SystemTime {
    // An arbitrary fixed boundary beyond the slack, with no wall-clock dependency.
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(100))
        .unwrap()
}

fn harvest_len(harvest: Harvest) -> usize {
    match harvest {
        Harvest::Callgrind(items) => items.len(),
        Harvest::Criterion(items) => items.len(),
        Harvest::AllocTracker(items) | Harvest::AllTheTime(items) => items.len(),
    }
}

fn collect(files: &FakeFiles, engine: Engine, since: Option<SystemTime>) -> io::Result<Harvest> {
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

fn candidate(engine: Engine, modified: SystemTime, included: bool) -> FakeFiles {
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

/// A finite script of expected native operations, consumed to detect extra reads and rescans.
#[derive(Default)]
struct FakeFiles {
    directories: RefCell<BTreeMap<PathBuf, io::Result<Directory>>>,
    contents: RefCell<BTreeMap<PathBuf, io::Result<String>>>,
}

impl FakeFiles {
    fn directory(&mut self, relative: &str, entries: impl IntoIterator<Item = Entry>) {
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

    fn file(&mut self, relative: &str, content: &str, modified: SystemTime) -> Entry {
        assert!(
            self.contents
                .get_mut()
                .insert(path(relative), Ok(content.into()))
                .is_none()
        );
        Entry::file(relative, modified)
    }

    fn assert_consumed(&self) {
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
struct Directory(VecDeque<io::Result<Entry>>);

impl OutputDirectory for Directory {
    type Entry = Entry;

    fn next_entry(&mut self) -> impl Future<Output = io::Result<Option<Entry>>> {
        ready(self.0.pop_front().transpose())
    }
}

/// Entry facts with independent type and metadata failures.
struct Entry {
    path: PathBuf,
    file_type: Result<EntryType, io::ErrorKind>,
    modified: Result<SystemTime, io::ErrorKind>,
}

impl Entry {
    fn file(relative: &str, modified: SystemTime) -> Self {
        Self {
            modified: Ok(modified),
            ..Self::ignored(relative)
        }
    }

    fn ignored(relative: &str) -> Self {
        Self {
            path: path(relative),
            file_type: Ok(EntryType::File),
            // Irrelevant files must not trigger metadata acquisition.
            modified: Err(io::ErrorKind::Unsupported),
        }
    }

    fn directory(relative: &str) -> Self {
        Self {
            file_type: Ok(EntryType::Directory),
            ..Self::ignored(relative)
        }
    }

    fn other(relative: &str) -> Self {
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
