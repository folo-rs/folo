use cbh_model::Engine;

use super::harness::{Entry, FakeFiles, boundary, collect, path};
use crate::bench_output::{Harvest, RawSummary};
use crate::output_files::EntryType;

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
