//! Tables and figures for the Shape of the data and Collection chapters.
//!
//! These two chapters describe the storage layer, so their evidence is drawn from the model
//! types and key builders rather than transcribed: metric kinds come from the enum, interval
//! support comes from the engine contract, key grammar from the key builder, and the schema
//! version from the constant. A metric kind added to the model appears in the book on the next
//! regeneration, and one whose name changes cannot leave a stale name behind in the prose.

use std::fmt::Write as _;

#[cfg(test)]
use cbh_model::sanitize_segment;
use cbh_model::{
    DiscriminantSet, Engine, IntervalSupport, MachineKey, MetricKind, SCHEMA_VERSION, TargetTriple,
};

use crate::assets::Asset;
use crate::styles::occupancy::{Cell, Occupancy};

/// Every asset the two storage chapters embed.
#[must_use]
pub fn assets() -> Vec<Asset> {
    vec![
        Asset::new("shape-engine-series.md", engine_series()),
        Asset::new("shape-engines.md", engines()),
        Asset::new("shape-dispersion.md", dispersion()),
        Asset::new("shape-identity.md", identity()),
        Asset::new("shape-run.md", run_shape()),
        Asset::new("shape-key-grammar.md", key_grammar()),
        Asset::new("shape-object-kinds.md", object_kinds()),
        Asset::new("collection-conflicts.md", conflicts()),
        Asset::new("collection-machine-key.md", machine_key()),
        Asset::new("collection-occupancy.svg", occupancy()),
    ]
}

/// The metric kinds each engine reports, and what each one measures.
///
/// The mapping is stated here rather than read from the adapters because an adapter reports a
/// kind only when its input happens to contain one; the *contract* — which kinds an engine can
/// produce at all — is a design fact, and this table is where the appendix states it. The
/// [`metric_kinds_are_all_accounted_for`] test holds it to the model, so a kind added to the
/// enum cannot go unmentioned.
///
/// [`metric_kinds_are_all_accounted_for`]: tests::metric_kinds_are_all_accounted_for
fn engine_kinds() -> Vec<(Engine, &'static str, Vec<MetricKind>)> {
    vec![
        (
            Engine::Criterion,
            "Wall-clock time per iteration",
            vec![MetricKind::WallTime],
        ),
        (
            Engine::Callgrind,
            "Simulated instruction and branch counts",
            vec![
                MetricKind::InstructionCount,
                MetricKind::ConditionalBranches,
                MetricKind::IndirectBranches,
            ],
        ),
        (
            Engine::AllocTracker,
            "Heap allocation, in bytes and in count",
            vec![MetricKind::AllocatedBytes, MetricKind::AllocationCount],
        ),
        (
            Engine::AllTheTime,
            "Processor time per iteration",
            vec![MetricKind::ProcessorTime],
        ),
    ]
}

/// How many series one benchmark yields, per engine.
fn engine_series() -> String {
    let mut markdown =
        String::from("| Engine | One benchmark yields | Which series |\n|---|---|---|\n");
    for (engine, _, kinds) in engine_kinds() {
        let names: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
        writeln!(
            markdown,
            "| `{engine}` | {} | {} |",
            count_of(kinds.len(), "series", "series"),
            names
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", "),
        )
        .expect("writing to a String never fails");
    }
    markdown
}

/// What each engine measures, and the unit it is stored in.
fn engines() -> String {
    let mut markdown = String::from("| Engine | Measures | Stored in |\n|---|---|---|\n");
    for (engine, measures, kinds) in engine_kinds() {
        let units: Vec<&str> = {
            let mut seen: Vec<&str> = kinds.iter().map(|kind| unit_of(*kind)).collect();
            seen.dedup();
            seen
        };
        writeln!(
            markdown,
            "| `{engine}` | {measures} | {} |",
            units.join(", ")
        )
        .expect("writing to a String never fails");
    }
    markdown
}

/// The unit a metric kind is stored in.
///
/// Units are a property of the kind, and the appendix quotes them in several places, so they
/// are defined once here.
fn unit_of(kind: MetricKind) -> &'static str {
    match kind {
        MetricKind::WallTime | MetricKind::ProcessorTime => "nanoseconds",
        MetricKind::InstructionCount
        | MetricKind::ConditionalBranches
        | MetricKind::IndirectBranches
        | MetricKind::AllocationCount => "counts",
        MetricKind::AllocatedBytes => "bytes",
    }
}

/// Which engines report dispersion, and under what conditions.
fn dispersion() -> String {
    let mut markdown =
        String::from("| Engine | Confidence interval | Standard deviation |\n|---|---|---|\n");
    for engine in Engine::ALL {
        writeln!(
            markdown,
            "| `{engine}` | {} | {} |",
            interval_support_description(engine.interval_support()),
            standard_deviation_description(engine),
        )
        .expect("writing to a String never fails");
    }
    markdown
}

/// The wording used in the storage chapter for interval support.
fn interval_support_description(support: IntervalSupport) -> &'static str {
    match support {
        IntervalSupport::Always => "Always",
        IntervalSupport::MultiSpanOnly => "Only when the operation was measured over several spans",
        IntervalSupport::Never => "Never — it reports a single simulated figure",
    }
}

/// The wording used in the storage chapter for standard-deviation storage.
fn standard_deviation_description(engine: Engine) -> &'static str {
    match engine {
        Engine::Criterion => "Recorded, never read",
        Engine::Callgrind | Engine::AllocTracker | Engine::AllTheTime => "Never",
    }
}

/// How each engine forms a benchmark identity.
fn identity() -> String {
    let mut markdown = String::from("| Engine | Segments | Note |\n|---|---|---|\n");
    for engine in Engine::ALL {
        let (segments, note) = identity_description(engine);
        writeln!(markdown, "| `{engine}` | {segments} | {note} |")
            .expect("writing to a String never fails");
    }
    markdown
}

/// The storage identity contract for one engine.
fn identity_description(engine: Engine) -> (&'static str, &'static str) {
    match engine {
        Engine::Criterion => (
            "group, function, and the parameter where the benchmark is parameterized",
            "Carries no package name, so identical names in different crates share a series",
        ),
        Engine::Callgrind => (
            "package directory, module path, function, and the case id where one is given",
            "Fully qualified",
        ),
        Engine::AllocTracker | Engine::AllTheTime => (
            "the operation name alone",
            "Carries no package name, so identical operation names in different crates share a \
             series",
        ),
    }
}

/// The shape of a stored run.
fn run_shape() -> String {
    format!(
        "```text\n\
         run\n\
         ├── schema_version          {SCHEMA_VERSION}\n\
         ├── context\n\
         │   ├── observed_at         when the measurement was taken (provenance only)\n\
         │   ├── git                 commit, branch, and whether the tree was dirty\n\
         │   ├── environment         local, or the CI provider that ran it\n\
         │   ├── toolchain           target triple and compiler version\n\
         │   ├── tool_version        which cargo-bench-history wrote this\n\
         │   ├── machine             host hardware description (recorded, never read)\n\
         │   └── best_of             repetitions run, when --best-of was used\n\
         └── results[]               one per benchmark case\n    \
             ├── id                  the benchmark identity\n    \
             └── metrics[]           one per metric kind, each with\n        \
                 ├── value           the measurement\n        \
                 ├── std_dev         where the engine reports one (recorded, never read)\n        \
                 └── interval        low and high, where the engine reports them\n\
         ```\n"
    )
}

/// The storage key grammar, rendered from a real key.
fn key_grammar() -> String {
    let set = DiscriminantSet::new(
        Engine::Callgrind,
        &TargetTriple::from("x86_64-unknown-linux-gnu"),
        &MachineKey::from("a1b2c3d4e5f60718"),
    );
    let key = set.clean_key("wordcount", "4f2a1c9d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39");

    format!(
        "```text\n{key}\n```\n\n\
         | Segment | Meaning |\n|---|---|\n\
         | `v1` | Storage layout version |\n\
         | `wordcount` | Project — which store this is |\n\
         | `objects` | Fixed; separates records from any future sibling namespace |\n\
         | `callgrind` | Discriminant set field: engine |\n\
         | `x86_64-unknown-linux-gnu` | Discriminant set field: target triple |\n\
         | `a1b2c3d4e5f60718` | Discriminant set field: machine key |\n\
         | `4f2a1c9…` | The commit |\n\
         | `clean.json` | Which kind of object |\n"
    )
}

/// The three object kinds a commit directory holds.
fn object_kinds() -> String {
    String::from(
        "| File | What it is |\n|---|---|\n\
         | `clean.json` | The canonical run for that commit. At most one. |\n\
         | `dirty-<unix>.json` | A snapshot taken with uncommitted changes. Any number. |\n\
         | `bless-<unix>.json` | A recorded acceptance of the level at that commit. Any \
         number. |\n",
    )
}

/// What happens when a run already exists at a commit.
fn conflicts() -> String {
    String::from(
        "| Policy | Behaviour | When to use it |\n|---|---|---|\n\
         | default | Refuses, leaving the stored run untouched | Always, unless you have a \
         reason not to |\n\
         | `--skip-existing` | Leaves the stored run and reports success | Re-running a \
         collection over a range where some commits are already done |\n\
         | `--overwrite` | Replaces the stored run | Re-measuring a commit whose recorded run \
         you do not trust |\n",
    )
}

/// What the machine key does and does not hash.
fn machine_key() -> String {
    String::from(
        "| Hashed | Not hashed |\n|---|---|\n\
         | Processor count | Clock speeds — they vary with thermal state and \
         power policy on one machine |\n\
         | Memory-region count | Hostname — a renamed machine keeps its history |\n\
         | Processor models | Installed memory size |\n",
    )
}

/// A store as commits across, partitions down.
///
/// The shapes are laid out deliberately, one per lesson the Collection chapter draws from the
/// picture: a dense partition with an interior gap and a dirty tip, a partition that joined
/// the pool late, and one that stops when its benchmark was deleted.
fn occupancy() -> String {
    let span = 24_usize;

    let dense = (0..span).map(|position| match position {
        9 | 10 => Cell::Absent,
        23 => Cell::Dirty,
        _ => Cell::Clean,
    });
    let joined_late = (0..span).map(|position| {
        if position < 13 {
            Cell::Absent
        } else {
            Cell::Clean
        }
    });
    let retired = (0..span).map(|position| {
        if position > 17 {
            Cell::Absent
        } else {
            Cell::Clean
        }
    });

    Occupancy::new("one store, three partitions")
        .row("criterion / machine a1b2", dense)
        .row("criterion / machine 9f8e", joined_late)
        .row("callgrind / machine a1b2", retired)
        .render()
}

/// `count` with a singular or plural noun.
fn count_of(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn every_documented_asset_is_produced() {
        let paths: Vec<String> = assets().into_iter().map(|asset| asset.path).collect();

        for expected in [
            "shape-engine-series.md",
            "shape-engines.md",
            "shape-dispersion.md",
            "shape-identity.md",
            "shape-run.md",
            "shape-key-grammar.md",
            "shape-object-kinds.md",
            "collection-conflicts.md",
            "collection-machine-key.md",
            "collection-occupancy.svg",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} missing"
            );
        }
    }

    /// A metric kind added to the model must appear in the chapter's tables. Without this the
    /// appendix would keep describing a complete set that had quietly stopped being complete.
    #[test]
    fn metric_kinds_are_all_accounted_for() {
        let documented: Vec<MetricKind> = engine_kinds()
            .into_iter()
            .flat_map(|(_, _, kinds)| kinds)
            .collect();

        for kind in MetricKind::ALL {
            assert!(
                documented.contains(&kind),
                "{} is in the model but no engine row mentions it",
                kind.as_str()
            );
        }
        assert_eq!(
            documented.len(),
            MetricKind::ALL.len(),
            "an engine row claims a metric kind twice"
        );
    }

    #[test]
    fn dispersion_mentions_every_engine() {
        assert_every_engine_has_one_row(&dispersion());
    }

    #[test]
    fn identity_mentions_every_engine() {
        assert_every_engine_has_one_row(&identity());
    }

    #[test]
    fn every_metric_kind_has_a_unit() {
        for kind in MetricKind::ALL {
            assert!(!unit_of(kind).is_empty());
        }
    }

    /// The grammar table names the segments of a real key, so the two must agree on how many
    /// there are.
    #[test]
    fn the_key_grammar_table_matches_a_real_key() {
        let markdown = key_grammar();
        let key = markdown
            .lines()
            .find(|line| line.starts_with("v1/"))
            .expect("the fragment leads with the rendered key");

        assert_eq!(
            key.split('/').count(),
            8,
            "the grammar table describes an eight-segment key"
        );
        assert!(markdown.contains("| `callgrind` | Discriminant set field: engine |"));
    }

    /// Sanitisation lowercases, so two keys differing only in case collapse into one
    /// partition. The chapter warns about this, so it is pinned here.
    #[test]
    fn segments_differing_only_in_case_collapse() {
        assert_eq!(
            sanitize_segment("MachineKey"),
            sanitize_segment("machinekey")
        );
    }

    #[test]
    fn the_run_shape_names_the_current_schema_version() {
        assert!(run_shape().contains(&SCHEMA_VERSION.to_string()));
    }

    #[test]
    fn the_object_kind_table_names_real_key_files() {
        let set = DiscriminantSet::new(
            Engine::Criterion,
            &TargetTriple::from("x86_64-pc-windows-msvc"),
            &MachineKey::from("m1"),
        );
        let observation_unix = 1_700_000_000;
        let issued_unix = 1_700_000_001;

        let clean_key = set.clean_key("folo", "abc123");
        let dirty_key = set.dirty_key("folo", "abc123", observation_unix);
        let bless_key = set.bless_key("folo", "abc123", issued_unix);
        let clean_file = file_segment(&clean_key);
        let dirty_file = file_segment(&dirty_key);
        let bless_file = file_segment(&bless_key);
        let dirty_pattern = dirty_file.replace(&observation_unix.to_string(), "<unix>");
        let bless_pattern = bless_file.replace(&issued_unix.to_string(), "<unix>");

        let table = object_kinds();
        for file in [clean_file.to_owned(), dirty_pattern, bless_pattern] {
            assert!(table.contains(&format!("| `{file}` |")));
        }
    }

    /// The hashed column names the fingerprint factors, not a broader hardware picture.
    /// The keys come from `cbh_probe::describe_fingerprint_components`, so a new hashed
    /// factor fails here until the table names it, and the labels cannot drift back to
    /// topology or layout.
    #[test]
    fn the_machine_key_table_names_the_fingerprint_factors() {
        let profile = cbh_probe::HardwareProfile {
            processors: 8,
            memory_regions: 1,
            processor_models: vec!["CPU".to_owned()],
            processor_speeds: vec![(1000, 8)],
        };
        let described = cbh_probe::describe_fingerprint_components(&profile);
        let keys: Vec<&str> = described
            .split(", ")
            .filter_map(|part| part.split('=').next())
            .collect();
        assert_eq!(
            keys,
            [
                "version",
                "processors",
                "memory_regions",
                "processor_models"
            ],
            "{described}"
        );

        let table = machine_key();
        // Version is a fingerprint tag, not a hardware factor, so it is not a row.
        // The remaining keys are the hashed hardware factors.
        for (factor, label) in [
            ("processors", "Processor count"),
            ("memory_regions", "Memory-region count"),
            ("processor_models", "Processor models"),
        ] {
            assert!(
                described.contains(&format!("{factor}=")),
                "{factor} is a fingerprint component"
            );
            assert!(
                table.contains(&format!("| {label} |")),
                "{factor} must appear as {label}: {table}"
            );
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "plotters SVG generation is host graphics, not memory-safety-relevant, and exceeds the Miri CI budget"
    )]
    fn rendering_is_reproducible() {
        assert_eq!(assets(), assets());
    }

    fn assert_every_engine_has_one_row(table: &str) {
        let rows: Vec<&str> = table
            .lines()
            .filter(|line| line.starts_with("| `"))
            .collect();

        for engine in Engine::ALL {
            let prefix = format!("| `{engine}` |");
            assert_eq!(
                rows.iter()
                    .filter(|row| row.starts_with(prefix.as_str()))
                    .count(),
                1
            );
        }
        assert_eq!(rows.len(), Engine::ALL.len());
    }

    fn file_segment(key: &str) -> &str {
        key.rsplit('/').next().unwrap()
    }
}
