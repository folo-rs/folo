use cbh_model::Engine;

use super::harness::{Entry, FakeFiles, boundary, collect, engine_dir, path};
use crate::bench_output::{Harvest, RawOperationFile};

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
