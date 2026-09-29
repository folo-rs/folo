use std::io;

use cbh_model::Engine;

use super::harness::{
    Entry, FakeFiles, boundary, candidate, collect, engine_dir, engines, harvest_len, path,
};

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
