//! Exercises reusable collection without spending time measuring benchmark performance.

use std::env;
use std::path::PathBuf;

use cargo_bench_history_faker::{parse_criterion_arg, write_criterion_case};

fn main() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace.join(path)
            }
        })
        .unwrap_or_else(|| workspace.join("target"));
    // Representative fixed values exercise artifact transport and honest report qualification.
    let case = parse_criterion_arg("reusable-workflow|smoke=100@1/99:101");
    write_criterion_case(&target, &case);
}
