//! Orchestration of the `rekey` migration: scan the store, decide what may move,
//! judge whether the resulting merges are safe, and copy.
//!
//! The pass is a copy, never a move. Every eligible object is written to its
//! current-format key with the write-once [`Storage::put`], leaving the original in
//! place; nothing is ever deleted or overwritten. A destination that already holds a
//! byte-identical object is a completed copy from an earlier pass, which makes the
//! command idempotent without any state of its own. A destination that holds
//! *different* bytes is a genuine conflict and stops the pass, and a destination two
//! planned copies would both claim withdraws both of them.
//!
//! Because the copies an earlier pass made are themselves stored objects, a later pass
//! reads them back. Any object standing at a destination this pass would write is
//! therefore excluded from the merge assessment, which would otherwise compare a group
//! against a set containing it.
//!
//! Writing is opt-in (`--apply`). The default pass builds the identical plan, reports
//! it, and writes nothing.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::str;

use cbh_command::RekeyOptions;
use cbh_config::{
    load_config, resolve_config_path, resolve_local_path, resolve_project_id, resolve_repo,
    storage_env,
};
use cbh_diag::{Reporter, ReporterExt, StderrReporter, count_noun};
use cbh_git::{GitHistory, SystemGitHistory};
use cbh_model::{
    DiscriminantSet, MachineInfo, MachineKey, MetricKind, ObjectKind, Run, STORAGE_VERSION,
    StorageKey, parse_key,
};
use cbh_probe::{HardwareProfile, resolve_machine_key};
use cbh_storage::{
    Storage, StorageError, StorageFacade, finish_with_flush, project_objects_prefix,
    resolve_storage,
};
use serde::Serialize;

use crate::rekey::{
    GroupPair, MeasuredPoint, MergeAnalysis, PartitionMerge, analyze_merges, legacy_machine_key,
    merge_offset_tolerance,
};
use crate::{
    AnalyzeError, RenderedReports, ReportFormat, ReportRequest, format_value,
    load_objects_concurrently,
};

/// The ref whose first-parent line places stored commits in history when `--context`
/// is not given.
const DEFAULT_TARGET_REF: &str = "HEAD";

/// How many blocking partition pairs a refusal message names before summarizing the
/// rest. Enough to show the shape of the problem without burying the guidance that
/// follows it.
const REFUSAL_DETAIL_LIMIT: usize = 5;

/// The real `rekey`: load configuration, wire the configured storage and git history,
/// and orchestrate.
// Thin real-adapter wiring: loads config from disk, builds the configured storage, and
// shells out via `SystemGitHistory` before delegating every decision to the
// mutation-tested `rekey_with`. In-crate tests cannot drive these real adapters
// deterministically; the binary's integration tests cover this edge.
#[cfg_attr(test, mutants::skip)]
pub async fn execute(
    options: &RekeyOptions,
    workspace_dir: &Path,
    storage_override: Option<StorageFacade>,
) -> Result<RenderedReports, AnalyzeError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = resolve_config_path(workspace_dir, options.config_path.as_deref());
    reporter.note_with(|| format!("loading configuration from {}", config_path.display()));
    let config = load_config(&config_path, options.config_path.is_some()).await?;

    let project_id = resolve_project_id(&config, workspace_dir);
    let local = resolve_local_path(options.local.as_ref(), storage_env().as_deref())?;
    // No read-through cache: `rekey` writes objects, and the caching decorator is only
    // attached to commands that read the bulk history without storing anything.
    let storage = resolve_storage(
        storage_override,
        local.as_deref(),
        &config,
        workspace_dir,
        None,
        &reporter,
    )?;
    storage.synchronize_cache(&project_id, &reporter).await?;

    let git = SystemGitHistory::new(resolve_repo(workspace_dir, options.repo.as_deref()));

    let result = rekey_with(&git, &storage, &project_id, options, &reporter).await;
    // `rekey` only ever writes new keys, so it arms no cache invalidation; the flush
    // is still driven so a marker armed by the cache synchronization above cannot be
    // stranded.
    let flush = storage
        .flush_pending_invalidation(&project_id, &reporter)
        .await;
    storage.report_cache_tally(&reporter);
    finish_with_flush(result, flush)
}

/// Storage- and git-generic `rekey`: place the stored commits in history, classify
/// every stored object, judge the merges the plan would cause, and — under `--apply` —
/// copy.
pub(crate) async fn rekey_with<G, S>(
    git: &G,
    storage: &S,
    project_id: &str,
    options: &RekeyOptions,
    reporter: &dyn Reporter,
) -> Result<RenderedReports, AnalyzeError>
where
    G: GitHistory,
    S: Storage,
{
    let request = ReportRequest::resolve(
        options.no_text,
        options.markdown.as_deref(),
        options.json.as_deref(),
    )?;

    let target_ref = options.context.as_deref().unwrap_or(DEFAULT_TARGET_REF);
    let order = resolve_commit_order(git, target_ref, reporter).await?;
    let scan = scan_store(storage, project_id, reporter).await?;

    let mut plan = build_plan(
        project_id,
        target_ref,
        options.apply,
        &scan,
        &order,
        reporter,
    )?;

    announce_plan(&plan, reporter);
    refuse_unsafe_merges(&plan, options.allow_level_shift, reporter)?;

    if options.apply {
        apply_plan(storage, &mut plan, reporter).await?;
    } else {
        reporter.note_with(|| {
            "dry run: no object is written; re-run with --apply to perform the copies".to_owned()
        });
    }

    Ok(request.render(|format| render_plan(&plan, format)))
}

/// Places each commit on the target ref's first-parent line.
///
/// The map is the only ordering `rekey` uses, matching the rest of the tool: a run's
/// place in history is its commit's topological position, never a stored timestamp. A
/// commit that is absent (or a repository that cannot be read at all) leaves the
/// affected objects unplaced, which costs the interleaving classification but never
/// blocks a migration.
async fn resolve_commit_order<G: GitHistory>(
    git: &G,
    target_ref: &str,
    reporter: &dyn Reporter,
) -> Result<HashMap<String, usize>, AnalyzeError> {
    let Some(head) = git.resolve(target_ref).await.map_err(AnalyzeError::Io)? else {
        reporter.note_with(|| {
            format!(
                "{target_ref} does not resolve to a commit, so stored runs cannot be placed in \
                 history; every merge's interleaving will be reported as unknown"
            )
        });
        return Ok(HashMap::new());
    };
    reporter.note_with(|| format!("{target_ref} resolves to {head}"));

    let commits = git.first_parent(&head).await.map_err(AnalyzeError::Io)?;
    reporter.note_with(|| {
        format!(
            "the first-parent line of {head} carries {}, which is the order stored runs are \
             placed in",
            count_noun(commits.len(), "commit")
        )
    });
    Ok(commits
        .into_iter()
        .enumerate()
        .map(|(index, commit)| (commit.commit_id, index))
        .collect())
}

/// A measurement read out of a stored run, before it is placed in commit order.
#[derive(Clone, Debug)]
struct RunMeasurement {
    /// The qualified benchmark identity the measurement belongs to.
    benchmark: String,
    /// Which metric was measured.
    metric: MetricKind,
    /// The measured value, in the metric's own units.
    value: f64,
}

/// What a rekey pass needs to read out of one stored run.
#[derive(Clone, Debug)]
struct RunFacts {
    /// The hardware provenance recorded with the run. `None` on a run written before
    /// the field existed.
    machine: Option<MachineInfo>,
    /// Every measurement the run recorded, for the merge level comparison.
    measurements: Vec<RunMeasurement>,
}

/// Everything one listing of the store yielded.
struct Scan {
    /// Stored runs (clean and dirty), each with its parsed key and decoded facts.
    runs: Vec<(String, StorageKey, RunFacts)>,
    /// Stored blessing sidecars and their parsed keys. Sidecars carry no hardware
    /// provenance, so they are not fetched during the scan.
    blessings: Vec<(String, StorageKey)>,
    /// Keys the store returned that are not recognized storage keys.
    unrecognized: usize,
}

/// Lists the project's stored objects and decodes every run.
///
/// The whole store is listed: a migration that skipped a partition would leave it
/// fragmented with nothing to say which ones were covered, so `rekey` takes no facet
/// filters at all.
async fn scan_store<S: Storage>(
    storage: &S,
    project_id: &str,
    reporter: &dyn Reporter,
) -> Result<Scan, AnalyzeError> {
    let prefix = project_objects_prefix(project_id);
    reporter.note_with(|| {
        format!("listing every stored object of project {project_id} under prefix {prefix}")
    });
    let keys = storage.list(&prefix).await.map_err(AnalyzeError::Storage)?;
    reporter.note_with(|| format!("storage returned {}", count_noun(keys.len(), "object key")));

    let mut runs: Vec<(String, StorageKey)> = Vec::new();
    let mut blessings: Vec<(String, StorageKey)> = Vec::new();
    let mut unrecognized: usize = 0;
    for key in keys {
        let Some(parsed) = parse_key(&key) else {
            unrecognized = unrecognized.saturating_add(1);
            reporter.note_with(|| {
                format!("skipping {key}: not a recognized {STORAGE_VERSION} storage key")
            });
            continue;
        };
        if parsed.is_bless() {
            blessings.push((key, parsed));
        } else {
            runs.push((key, parsed));
        }
    }
    reporter.note_with(|| {
        format!(
            "the listing holds {} and {}",
            count_noun(runs.len(), "run"),
            count_noun(blessings.len(), "blessing sidecar")
        )
    });

    let mut loaded = load_objects_concurrently(storage, runs, decode_run).await?;
    loaded.sort_by(|left, right| left.0.cmp(&right.0));
    blessings.sort_by(|left, right| left.0.cmp(&right.0));

    Ok(Scan {
        runs: loaded,
        blessings,
        unrecognized,
    })
}

/// Decodes one stored run into the facts a rekey pass needs from it.
fn decode_run(key: &str, bytes: Vec<u8>) -> Result<RunFacts, AnalyzeError> {
    let text = String::from_utf8(bytes).map_err(|error| AnalyzeError::Analyze {
        message: format!("stored object {key} is not valid UTF-8: {error}"),
    })?;
    let run = Run::from_json(&text).map_err(|error| AnalyzeError::Analyze {
        message: format!("stored object {key} is not a valid result set: {error}"),
    })?;
    let measurements = run
        .results
        .into_iter()
        .flat_map(|result| {
            let benchmark = result.id.qualified();
            result
                .metrics
                .into_iter()
                .map(move |metric| RunMeasurement {
                    benchmark: benchmark.clone(),
                    metric: metric.kind,
                    value: metric.value,
                })
        })
        .collect();
    Ok(RunFacts {
        machine: run.context.machine,
        measurements,
    })
}

/// How one stored object's machine-key segment relates to the two hashes of the
/// hardware the object itself recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SegmentIdentity {
    /// The segment is the retired-format hash of the recorded hardware, so the object
    /// is a hardware-keyed run of the old format and may move.
    Legacy,
    /// The segment is the current-format hash, so the object is already migrated.
    Current,
    /// The segment is neither hash, so it was chosen by an operator (a `--machine-key`
    /// override such as a CI pool name) rather than derived from the hardware. Its
    /// partitioning is a deliberate decision and must be left exactly as it is. It
    /// carries the retired hash it was compared against, which only exists when the
    /// recorded hardware renders under the retired format — the sole circumstance in
    /// which a segment can be told apart from that hash at all.
    Override { legacy: String },
}

/// One object the pass would copy.
#[derive(Clone, Debug)]
pub(crate) struct PlannedCopy {
    /// The key the object is stored under today.
    source_key: String,
    /// The key it would be copied to.
    destination_key: String,
    /// The destination partition.
    destination_set: DiscriminantSet,
    /// The machine key it is stored under today.
    source_machine_key: String,
}

/// The fully resolved migration plan, ready to render.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    /// The project the objects belong to.
    project: String,
    /// The ref whose first-parent line placed the stored commits in history.
    target_ref: String,
    /// Whether the pass writes (`--apply`) or only reports.
    apply: bool,
    /// How many stored objects the pass considered.
    scanned: usize,
    /// The copies the pass would perform, in storage-key order.
    copies: Vec<PlannedCopy>,
    /// Objects already stored under the current-format key.
    already_current: usize,
    /// Objects left alone because their machine key is an operator-supplied override,
    /// counted per key segment.
    key_overrides: BTreeMap<String, usize>,
    /// Blessing sidecars whose machine key no stored run maps, counted per key segment.
    /// A sidecar carries no hardware provenance of its own, so it can only follow the
    /// runs of its own partition.
    unmapped_blessings: BTreeMap<String, usize>,
    /// Runs that record no hardware provenance at all, by storage key. Their key
    /// cannot be recomputed, so they are reported rather than silently passed over.
    missing_provenance: Vec<String>,
    /// Runs whose recorded hardware does not render under the retired machine-key
    /// format, by storage key. No retired key can be recomputed for them, so nothing
    /// proves their key segment was an auto-detected hash and they are reported
    /// rather than moved on a rendering that could name a different machine.
    unrenderable_provenance: Vec<String>,
    /// Destinations two or more objects would both claim, listing the sources that
    /// competed for each. All of them are left where they are.
    collisions: BTreeMap<String, Vec<String>>,
    /// Keys the store returned that are not recognized storage keys.
    unrecognized: usize,
    /// The merge risk assessment of the plan.
    merges: MergeAnalysis,
    /// Objects copied by this pass. Zero on a dry run.
    copied: usize,
    /// Objects whose destination already held a byte-identical copy from an earlier
    /// pass. Zero on a dry run.
    already_present: usize,
}

impl Plan {
    /// The destination partitions the copies would write into, ascending.
    fn destination_sets(&self) -> Vec<&DiscriminantSet> {
        let mut sets: Vec<&DiscriminantSet> = self
            .copies
            .iter()
            .map(|copy| &copy.destination_set)
            .collect();
        sets.sort();
        sets.dedup();
        sets
    }

    /// Objects left untouched because their key is an operator-supplied override.
    fn key_override_objects(&self) -> usize {
        self.key_overrides.values().copied().sum()
    }

    /// Blessing sidecars left untouched because no run maps their machine key.
    fn unmapped_blessing_objects(&self) -> usize {
        self.unmapped_blessings.values().copied().sum()
    }
}

/// Why nothing can be proven about the machine-key segment a stored run sits under.
///
/// Both outcomes leave the segment unproven, so neither run moves. The report keeps
/// them apart because absent provenance is an artifact of the run's age that nothing
/// can repair, while unrenderable provenance points at a single damaged record an
/// operator can go and inspect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnprovableProvenance {
    /// The run records no hardware at all, as every run written before host hardware
    /// entered the schema does.
    Absent,
    /// The run records hardware the retired format cannot render, and its segment is
    /// not the current hash either, so the retired hash is the only thing that could
    /// have told a hardware key from an operator's own and it does not exist.
    Unrenderable,
}

impl UnprovableProvenance {
    /// Why the run is left alone, phrased to follow `skipping {key}: `.
    fn explanation(self) -> &'static str {
        match self {
            Self::Absent => {
                "it records no hardware provenance, so neither the retired nor the current \
                 machine key can be recomputed from it"
            }
            Self::Unrenderable => {
                "the hardware it records does not render under the retired machine-key format, \
                 so no retired key exists to prove its key segment is an auto-detected hash"
            }
        }
    }
}

/// Classifies every scanned object and assesses the merges the resulting plan causes.
///
/// # Errors
///
/// Returns [`AnalyzeError::Analyze`] when a stored run's recorded fingerprint is
/// neither the retired- nor the current-format hash of the hardware recorded beside
/// it. That would mean the reimplemented rendering is not the one that produced the
/// stored history, so no object's key segment can be trusted and the whole pass is
/// abandoned rather than continuing on a subset.
fn build_plan(
    project_id: &str,
    target_ref: &str,
    apply: bool,
    scan: &Scan,
    order: &HashMap<String, usize>,
    reporter: &dyn Reporter,
) -> Result<Plan, AnalyzeError> {
    let tolerance = merge_offset_tolerance();
    reporter.note_with(|| {
        format!(
            "merge tolerance derived from the detector's practical-significance floors: \
             relative {:.4}, counts {}, time {} ns, allocations {}",
            tolerance.relative,
            format_value(tolerance.absolute_count),
            format_value(tolerance.absolute_time),
            format_value(tolerance.absolute_alloc)
        )
    });

    let mut copies: Vec<PlannedCopy> = Vec::new();
    let mut already_current: usize = 0;
    let mut key_overrides: BTreeMap<String, usize> = BTreeMap::new();
    let mut missing_provenance: Vec<String> = Vec::new();
    let mut unrenderable_provenance: Vec<String> = Vec::new();
    // The mapping blessing sidecars follow: a retired-format machine key and the
    // current-format key the runs stored under it hash to.
    let mut mapping: BTreeMap<String, String> = BTreeMap::new();
    let mut current_keys: HashSet<String> = HashSet::new();
    // Measurements are staged against their own storage key so the copies an earlier
    // pass already made can be dropped before the merge assessment reads them.
    let mut staged: Vec<StagedPoints> = Vec::new();

    for (key, parsed, facts) in &scan.runs {
        let segment = parsed.set.machine_key.as_str();
        // Every decision about an object rests on recomputing its machine keys from
        // the hardware it records. A run that records nothing to recompute from, and
        // a run whose recorded hardware the retired rule cannot render into the one
        // hash that would place it, are alike unprovable and leave by the same door.
        let placement = match facts.machine.as_ref() {
            Some(machine) => {
                let current = current_machine_key(machine);
                let legacy = legacy_machine_key(machine);
                describe_hardware(key, machine, legacy.as_deref(), &current, reporter);

                // A run records the machine key its own capture computed, so the
                // fingerprint is the retired hash on history captured before the format
                // changed and the current hash on history captured after it. Either
                // proves the recomputation reproduces what really keyed the store; a
                // retired rendering that reproduces neither means the reimplemented
                // rendering is not the one that wrote this history, and every later
                // decision would rest on it. Hardware that does not render at all makes
                // no such claim to contradict, so it indicts only its own object.
                if let Some(legacy) = legacy.as_deref()
                    && legacy != machine.fingerprint
                    && current != machine.fingerprint
                {
                    return Err(AnalyzeError::Analyze {
                        message: format!(
                            "the machine-key renderings do not reproduce the fingerprint stored \
                             in {key}: the recorded hardware renders to {legacy} under the \
                             retired format and {current} under the current one, but the object \
                             records {stored}. The migration can only prove which objects are \
                             safe to move by recomputing their keys, so no object is touched.",
                            stored = machine.fingerprint
                        ),
                    });
                }

                classify_segment(segment, legacy.as_deref(), &current)
                    .map(|identity| (identity, current))
                    .ok_or(UnprovableProvenance::Unrenderable)
            }
            None => Err(UnprovableProvenance::Absent),
        };
        let (identity, current) = match placement {
            Ok(placement) => placement,
            Err(reason) => {
                reporter.note_with(|| format!("skipping {key}: {}", reason.explanation()));
                match reason {
                    UnprovableProvenance::Absent => missing_provenance.push(key.clone()),
                    UnprovableProvenance::Unrenderable => {
                        unrenderable_provenance.push(key.clone());
                    }
                }
                continue;
            }
        };

        match identity {
            SegmentIdentity::Current => {
                reporter.note_with(|| {
                    format!(
                        "leaving {key}: its key segment {segment} is already the current hash of \
                         its own recorded hardware"
                    )
                });
                already_current = already_current.saturating_add(1);
                _ = current_keys.insert(current.clone());
                staged.push(stage_points(
                    key,
                    &parsed.set,
                    segment,
                    parsed,
                    facts,
                    order,
                ));
            }
            SegmentIdentity::Legacy => {
                let destination_set = destination_set(&parsed.set, &current);
                let destination_key = destination_key(&destination_set, parsed);
                reporter.note_with(|| {
                    format!(
                        "copying {key}: its key segment {segment} is the retired hash of its own \
                         recorded hardware, whose current hash is {current}, so it belongs at \
                         {destination_key}"
                    )
                });
                // Every object reaching here proved its segment equals the retired
                // hash of its own recorded hardware, so two objects sharing a segment
                // hash their hardware alike and therefore map to one current key.
                _ = mapping.insert(segment.to_owned(), current.clone());
                staged.push(stage_points(
                    key,
                    &destination_set,
                    segment,
                    parsed,
                    facts,
                    order,
                ));
                copies.push(PlannedCopy {
                    source_key: key.clone(),
                    destination_key,
                    destination_set,
                    source_machine_key: segment.to_owned(),
                });
            }
            SegmentIdentity::Override { legacy } => {
                reporter.note_with(|| {
                    format!(
                        "leaving {key}: its key segment {segment} is neither the retired hash \
                         ({legacy}) nor the current hash ({current}) of its own recorded \
                         hardware, so it was partitioned under an explicit machine-key override \
                         that only an operator can change"
                    )
                });
                let counted = key_overrides.entry(segment.to_owned()).or_insert(0);
                *counted = counted.saturating_add(1);
            }
        }
    }

    let mut unmapped_blessings: BTreeMap<String, usize> = BTreeMap::new();
    for (key, parsed) in &scan.blessings {
        let segment = parsed.set.machine_key.as_str();
        if let Some(current) = mapping.get(segment) {
            let destination_set = destination_set(&parsed.set, current);
            let destination_key = destination_key(&destination_set, parsed);
            reporter.note_with(|| {
                format!(
                    "copying {key}: a blessing sidecar records no hardware of its own, and the \
                     runs stored under its machine key {segment} move to {current}, so it \
                     belongs at {destination_key}"
                )
            });
            copies.push(PlannedCopy {
                source_key: key.clone(),
                destination_key,
                destination_set,
                source_machine_key: segment.to_owned(),
            });
        } else if current_keys.contains(segment) {
            reporter.note_with(|| {
                format!(
                    "leaving {key}: its machine key {segment} already holds runs under the \
                     current key format"
                )
            });
            already_current = already_current.saturating_add(1);
        } else {
            reporter.note_with(|| {
                format!(
                    "leaving {key}: no stored run under machine key {segment} maps it to a \
                     current key, and a blessing sidecar records no hardware of its own"
                )
            });
            let counted = unmapped_blessings.entry(segment.to_owned()).or_insert(0);
            *counted = counted.saturating_add(1);
        }
    }

    copies.sort_by(|left, right| left.source_key.cmp(&right.source_key));
    let collisions = withdraw_colliding_copies(&mut copies, reporter);

    // An object an earlier pass already copied duplicates the source it came from, so
    // leaving it in the assessment would compare a group against a set containing
    // itself and invent a merge that the store does not actually face.
    let destinations: HashSet<&str> = copies
        .iter()
        .map(|copy| copy.destination_key.as_str())
        .collect();
    let mut groups: BTreeMap<DiscriminantSet, BTreeMap<String, Vec<MeasuredPoint>>> =
        BTreeMap::new();
    for entry in staged {
        if destinations.contains(entry.key.as_str()) {
            reporter.note_with(|| {
                format!(
                    "excluding {key} from the merge assessment: it is the destination of a copy \
                     this pass would make, so its measurements duplicate the source's",
                    key = entry.key
                )
            });
            continue;
        }
        groups
            .entry(entry.destination)
            .or_default()
            .entry(entry.source_machine_key)
            .or_default()
            .extend(entry.points);
    }

    Ok(Plan {
        project: project_id.to_owned(),
        target_ref: target_ref.to_owned(),
        apply,
        scanned: scan
            .runs
            .len()
            .saturating_add(scan.blessings.len())
            .saturating_add(scan.unrecognized),
        copies,
        already_current,
        key_overrides,
        unmapped_blessings,
        missing_provenance,
        unrenderable_provenance,
        collisions,
        unrecognized: scan.unrecognized,
        merges: analyze_merges(&groups, tolerance),
        copied: 0,
        already_present: 0,
    })
}

/// Removes every copy whose destination another planned copy also claims, returning
/// the withdrawn destinations with the sources that competed for them.
///
/// Two machine keys that merge can each hold an object for the same commit, and one
/// key holds one object. The competing objects are not copies of each other — their
/// hardware renders differently, which is why they were keyed apart — so neither can
/// stand for the other and both are left where they are. The rest of the migration
/// proceeds: the merged series simply gains no point at that commit, which the
/// analysis reads as an ordinary gap.
fn withdraw_colliding_copies(
    copies: &mut Vec<PlannedCopy>,
    reporter: &dyn Reporter,
) -> BTreeMap<String, Vec<String>> {
    let mut claimants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for copy in copies.iter() {
        claimants
            .entry(copy.destination_key.clone())
            .or_default()
            .push(copy.source_key.clone());
    }
    claimants.retain(|_, sources| sources.len() > 1);

    for (destination, sources) in &claimants {
        reporter.note_with(|| {
            format!(
                "leaving {sources}: they would all land at {destination}, and they are distinct \
                 objects rather than copies of one another, so none of them may claim it",
                sources = sources.join(" and ")
            )
        });
    }
    copies.retain(|copy| !claimants.contains_key(&copy.destination_key));
    claimants
}

/// Whether a machine-key segment is the retired hash, the current hash, or neither, or
/// `None` when the retired hash is the only thing that could settle the question and
/// the recorded hardware does not render under the retired format.
///
/// The current hash is recomputed by the live probe rather than by a reimplementation,
/// so a segment equal to it is proven a hardware hash on its own and needs no retired
/// hash to confirm it. Every other segment could be either the retired hash or an
/// operator's own constant, and without the retired hash nothing tells the two apart.
fn classify_segment(segment: &str, legacy: Option<&str>, current: &str) -> Option<SegmentIdentity> {
    if segment == current {
        return Some(SegmentIdentity::Current);
    }
    let legacy = legacy?;
    if segment == legacy {
        Some(SegmentIdentity::Legacy)
    } else {
        Some(SegmentIdentity::Override {
            legacy: legacy.to_owned(),
        })
    }
}

/// The current-format machine key of the hardware `machine` records.
fn current_machine_key(machine: &MachineInfo) -> String {
    resolve_machine_key(
        None,
        &HardwareProfile {
            processors: machine.processors,
            memory_regions: machine.memory_regions,
            processor_models: machine.processor_models.clone(),
            processor_speeds: machine.processor_speeds.clone(),
        },
    )
}

/// The same partition under a different machine key.
fn destination_set(source: &DiscriminantSet, machine_key: &str) -> DiscriminantSet {
    DiscriminantSet::new(
        source.engine,
        &source.target_triple,
        &MachineKey::from(machine_key),
    )
}

/// The key `parsed` would occupy in `set`, preserving its object kind.
fn destination_key(set: &DiscriminantSet, parsed: &StorageKey) -> String {
    match parsed.kind {
        ObjectKind::Clean => set.clean_key(&parsed.project, &parsed.commit),
        ObjectKind::Dirty { observation_unix } => {
            set.dirty_key(&parsed.project, &parsed.commit, observation_unix)
        }
        ObjectKind::Bless { issued_unix } => {
            set.bless_key(&parsed.project, &parsed.commit, issued_unix)
        }
    }
}

/// One run's measurements, staged against the object's own key so a duplicate an
/// earlier pass created can be dropped before the merge assessment reads it.
struct StagedPoints {
    key: String,
    destination: DiscriminantSet,
    source_machine_key: String,
    points: Vec<MeasuredPoint>,
}

/// Stages one run's measurements against the destination partition and the machine key
/// the run is stored under today, so the merge assessment can compare the groups that
/// would be spliced together.
fn stage_points(
    key: &str,
    destination: &DiscriminantSet,
    source_machine_key: &str,
    parsed: &StorageKey,
    facts: &RunFacts,
    order: &HashMap<String, usize>,
) -> StagedPoints {
    let position = order.get(&parsed.commit).copied();
    StagedPoints {
        key: key.to_owned(),
        destination: destination.clone(),
        source_machine_key: source_machine_key.to_owned(),
        points: facts
            .measurements
            .iter()
            .map(|measurement| MeasuredPoint {
                benchmark: measurement.benchmark.clone(),
                metric: measurement.metric,
                value: measurement.value,
                position,
            })
            .collect(),
    }
}

/// Emits the hardware facts behind an object's candidate keys, so a decision the
/// pass makes about it can be reconstructed from the log alone.
fn describe_hardware(
    key: &str,
    machine: &MachineInfo,
    legacy: Option<&str>,
    current: &str,
    reporter: &dyn Reporter,
) {
    reporter.note_with(|| {
        format!(
            "{key} records processors={processors}, memory_regions={regions}, \
             processor_models=[{models}], processor_speeds=[{speeds}]; those factors hash to \
             {current} under the current key format and {retired}",
            processors = machine.processors,
            regions = machine.memory_regions,
            models = machine.processor_models.join(", "),
            speeds = machine
                .processor_speeds
                .iter()
                .map(|(speed, count)| format!("{speed}x{count}"))
                .collect::<Vec<_>>()
                .join(", "),
            retired = match legacy {
                Some(legacy) => format!("{legacy} under the retired one"),
                None => "nothing under the retired one, which cannot render them".to_owned(),
            },
        )
    });
}

/// The always-on one-line summary of what the pass resolved, printed regardless of
/// `--verbose` so even a refusal states what it was looking at.
fn announce_plan(plan: &Plan, reporter: &dyn Reporter) {
    reporter.announce(&format!(
        "rekey: project {project}, ordered by {target_ref}; {scanned} scanned, {copies} to copy \
         into {sets}, {merges} merging",
        project = plan.project,
        target_ref = plan.target_ref,
        scanned = count_noun(plan.scanned, "object"),
        copies = count_noun(plan.copies.len(), "object"),
        sets = count_noun(plan.destination_sets().len(), "discriminant set"),
        merges = count_noun(plan.merges.merges.len(), "partition"),
    ));
}

/// Refuses a plan whose merges would manufacture a step change.
///
/// # Errors
///
/// Returns [`AnalyzeError::Analyze`] when a pair of merging partitions sits
/// systematically apart by at least the merge tolerance and `allow_level_shift` is not
/// set.
fn refuse_unsafe_merges(
    plan: &Plan,
    allow_level_shift: bool,
    reporter: &dyn Reporter,
) -> Result<(), AnalyzeError> {
    let blocking = plan.merges.blocking();
    if blocking.is_empty() {
        reporter.note_with(|| {
            format!(
                "no pair of the {} that would merge sits systematically apart by the merge \
                 tolerance, so splicing them cannot manufacture a step change; individual \
                 benchmarks may still disagree, which becomes ordinary noise within the merged \
                 series",
                count_noun(plan.merges.merges.len(), "partition")
            )
        });
        return Ok(());
    }

    if allow_level_shift {
        reporter.note_with(|| {
            format!(
                "--allow-level-shift is set, so proceeding despite {} that sit systematically \
                 apart by at least the merge tolerance",
                count_noun(blocking.len(), "partition pair")
            )
        });
        return Ok(());
    }

    let detail: Vec<String> = blocking
        .iter()
        .take(REFUSAL_DETAIL_LIMIT)
        .map(|(merge, pair)| describe_pair(merge, pair))
        .collect();
    let remainder = blocking.len().saturating_sub(detail.len());
    let more = if remainder == 0 {
        String::new()
    } else {
        format!(" (and {} more)", count_noun(remainder, "pair"))
    };

    Err(AnalyzeError::Analyze {
        message: format!(
            "merging these machine-key partitions would splice measurement levels that \
             systematically disagree beyond the merge tolerance, manufacturing a step change the \
             next analysis would report as a regression: {detail}{more}. Inspect the partitions \
             and either keep them apart or, if the difference is understood and acceptable, \
             re-run with --allow-level-shift.",
            detail = detail.join("; ")
        ),
    })
}

/// Renders one blocking partition pair for a refusal message.
fn describe_pair(merge: &PartitionMerge, pair: &GroupPair) -> String {
    format!(
        "{set}: {baseline} -> {incoming} ({interleaving}), {distance}",
        set = merge.set,
        baseline = pair.baseline_key,
        incoming = pair.incoming_key,
        interleaving = pair.interleaving.as_str(),
        distance = describe_systematic(pair),
    )
}

/// Renders the systematic offset that decides a pair's merge verdict, naming how many
/// shared offsets it was read across so the evidence behind it is visible.
fn describe_systematic(pair: &GroupPair) -> String {
    pair.systematic.map_or_else(
        || {
            "no shared offset is a large enough move to read, so there is no distance to \
            judge"
                .to_owned()
        },
        |systematic| {
            format!(
                "the two sit {percent} apart across {offsets}",
                percent = signed_percent(systematic.relative),
                offsets = count_noun(systematic.offsets, "shared offset")
            )
        },
    )
}

/// The one-word standing of a pair against the merge tolerance, so a reader never has
/// to infer the verdict from the numbers beside it.
fn describe_verdict(pair: &GroupPair) -> &'static str {
    if pair.manufactures_step {
        "blocked"
    } else {
        "clear"
    }
}

/// Copies every planned object, treating an identical destination as a completed copy.
///
/// # Errors
///
/// Returns [`AnalyzeError::Analyze`] when a destination already holds an object whose
/// bytes differ from the source's, and [`AnalyzeError::Storage`] when a fetch or write
/// fails.
async fn apply_plan<S: Storage>(
    storage: &S,
    plan: &mut Plan,
    reporter: &dyn Reporter,
) -> Result<(), AnalyzeError> {
    for copy in &plan.copies {
        // The bytes are re-fetched rather than retained from the scan: a full history
        // held resident is what a migration can least afford, and a `--cache` run
        // serves the second read from the local mirror.
        let bytes = storage
            .get(&copy.source_key)
            .await
            .map_err(AnalyzeError::Storage)?;
        match storage.put(&copy.destination_key, &bytes).await {
            Ok(()) => {
                reporter.note_with(|| {
                    format!(
                        "copied {source} to {destination}; the source object is left in place",
                        source = copy.source_key,
                        destination = copy.destination_key
                    )
                });
                plan.copied = plan.copied.saturating_add(1);
            }
            Err(StorageError::AlreadyExists { .. }) => {
                let existing = storage
                    .get(&copy.destination_key)
                    .await
                    .map_err(AnalyzeError::Storage)?;
                if existing == bytes {
                    reporter.note_with(|| {
                        format!(
                            "{destination} already holds the same object as {source}, so an \
                             earlier pass already copied it",
                            destination = copy.destination_key,
                            source = copy.source_key
                        )
                    });
                    plan.already_present = plan.already_present.saturating_add(1);
                } else {
                    return Err(AnalyzeError::Analyze {
                        message: format!(
                            "{destination} already holds a different object than {source} would \
                             write, so the destination is not a copy of the source and \
                             overwriting it would destroy a distinct measurement. Resolve the \
                             conflict before re-running.",
                            destination = copy.destination_key,
                            source = copy.source_key
                        ),
                    });
                }
            }
            Err(error) => return Err(AnalyzeError::Storage(error)),
        }
    }
    Ok(())
}

/// Renders the migration plan in the requested format.
fn render_plan(plan: &Plan, format: ReportFormat) -> String {
    match format {
        ReportFormat::Text => render_plan_text(plan),
        ReportFormat::Markdown => render_plan_markdown(plan),
        ReportFormat::Json => render_plan_json(plan),
    }
}

/// The verb describing what the pass did or would do with the planned copies.
fn verb(apply: bool) -> &'static str {
    if apply { "Copied" } else { "Would copy" }
}

/// A signed absolute magnitude, so the direction of a level offset is unambiguous.
fn signed(value: f64) -> String {
    let sign = if value < 0.0 { "-" } else { "+" };
    format!("{sign}{}", format_value(value.abs()))
}

/// A signed percentage of the baseline level.
fn signed_percent(relative: f64) -> String {
    const PERCENT_SCALE: f64 = 100.0;
    let sign = if relative < 0.0 { "-" } else { "+" };
    format!("{sign}{}%", format_value((relative * PERCENT_SCALE).abs()))
}

fn render_plan_text(plan: &Plan) -> String {
    let mut lines = vec![format!(
        "Rekey plan for project {} (ordered by {})",
        plan.project, plan.target_ref
    )];

    for merge in &plan.merges.merges {
        lines.push(String::new());
        lines.push(format!("{} (merging)", merge.set));
        lines.push(format!(
            "  {} merge into this partition: {}",
            count_noun(merge.source_keys.len(), "machine key"),
            merge.source_keys.join(", ")
        ));
        for pair in &merge.pairs {
            lines.push(format!(
                "  {} -> {}: {} over {}",
                pair.baseline_key,
                pair.incoming_key,
                pair.interleaving.as_str(),
                count_noun(pair.blocks, "block")
            ));
            lines.push(format!(
                "    merge verdict: {verdict} — {distance}",
                verdict = describe_verdict(pair),
                distance = describe_systematic(pair)
            ));
            lines.push(format!(
                "    {} beyond the merge tolerance individually, which is informational only",
                count_noun(pair.outlying_offsets().count(), "offset")
            ));
            for offset in &pair.offsets {
                lines.push(format!(
                    "    {} {}: {} -> {} ({}, {}){}",
                    offset.benchmark,
                    offset.metric.as_str(),
                    format_value(offset.baseline_level),
                    format_value(offset.incoming_level),
                    signed(offset.absolute),
                    signed_percent(offset.relative),
                    if offset.beyond_tolerance {
                        "  [beyond tolerance, informational]"
                    } else {
                        ""
                    }
                ));
            }
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "{} {} into {}",
        verb(plan.apply),
        count_noun(plan.copies.len(), "object"),
        count_noun(plan.destination_sets().len(), "discriminant set")
    ));
    if plan.apply {
        lines.push(format!(
            "  {} written, {} already present from an earlier pass",
            count_noun(plan.copied, "object"),
            count_noun(plan.already_present, "object")
        ));
    }
    lines.push(format!(
        "  {} already stored under the current key format",
        count_noun(plan.already_current, "object")
    ));
    for line in untouched_lines(plan) {
        lines.push(format!("  {line}"));
    }
    format!("{}\n", lines.join("\n"))
}

fn render_plan_markdown(plan: &Plan) -> String {
    let mut lines = vec![format!(
        "# Rekey plan for {} (ordered by {})",
        plan.project, plan.target_ref
    )];

    for merge in &plan.merges.merges {
        lines.push(String::new());
        lines.push(format!("## {}", merge.set));
        lines.push(String::new());
        lines.push(format!(
            "{} merge into this partition: {}",
            count_noun(merge.source_keys.len(), "machine key"),
            merge.source_keys.join(", ")
        ));
        for pair in &merge.pairs {
            lines.push(String::new());
            lines.push(format!(
                "`{}` -> `{}`: {} over {}",
                pair.baseline_key,
                pair.incoming_key,
                pair.interleaving.as_str(),
                count_noun(pair.blocks, "block")
            ));
            lines.push(String::new());
            lines.push(format!(
                "**Merge verdict: {verdict}** — {distance}. {outlying} beyond the merge \
                 tolerance individually, which is informational only.",
                verdict = describe_verdict(pair),
                distance = describe_systematic(pair),
                outlying = count_noun(pair.outlying_offsets().count(), "offset")
            ));
            lines.push(String::new());
            lines.push(
                "| Benchmark | Metric | Baseline | Incoming | Offset | Relative | Beyond \
                 tolerance |"
                    .to_owned(),
            );
            lines.push("| --- | --- | --- | --- | --- | --- | --- |".to_owned());
            for offset in &pair.offsets {
                lines.push(format!(
                    "| {} | {} | {} | {} | {} | {} | {} |",
                    offset.benchmark,
                    offset.metric.as_str(),
                    format_value(offset.baseline_level),
                    format_value(offset.incoming_level),
                    signed(offset.absolute),
                    signed_percent(offset.relative),
                    if offset.beyond_tolerance { "yes" } else { "no" },
                ));
            }
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "**{} {} into {}**",
        verb(plan.apply),
        count_noun(plan.copies.len(), "object"),
        count_noun(plan.destination_sets().len(), "discriminant set")
    ));
    for line in untouched_lines(plan) {
        lines.push(String::new());
        lines.push(line);
    }
    format!("{}\n", lines.join("\n"))
}

/// The lines describing what the pass deliberately left alone, so a reader can tell a
/// skipped object from an overlooked one.
fn untouched_lines(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    if !plan.key_overrides.is_empty() {
        lines.push(format!(
            "{} left under explicit machine keys: {}",
            count_noun(plan.key_override_objects(), "object"),
            describe_counts(&plan.key_overrides)
        ));
    }
    if !plan.unmapped_blessings.is_empty() {
        lines.push(format!(
            "{} left under machine keys no stored run maps: {}",
            count_noun(plan.unmapped_blessing_objects(), "blessing sidecar"),
            describe_counts(&plan.unmapped_blessings)
        ));
    }
    if !plan.missing_provenance.is_empty() {
        lines.push(format!(
            "{} left without recorded hardware provenance: {}",
            count_noun(plan.missing_provenance.len(), "run"),
            plan.missing_provenance.join(", ")
        ));
    }
    if !plan.unrenderable_provenance.is_empty() {
        lines.push(format!(
            "{} left with hardware provenance that does not render under the retired \
             machine-key format: {}",
            count_noun(plan.unrenderable_provenance.len(), "run"),
            plan.unrenderable_provenance.join(", ")
        ));
    }
    if !plan.collisions.is_empty() {
        lines.push(format!(
            "{} left because two or more distinct objects would claim each of them: {}",
            count_noun(plan.collisions.len(), "destination"),
            plan.collisions
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if plan.unrecognized > 0 {
        lines.push(format!(
            "skipped {} the store returned that no storage-key format recognizes",
            count_noun(plan.unrecognized, "key")
        ));
    }
    lines
}

/// Renders a per-key object tally as `key (n), key (n)`.
fn describe_counts(counts: &BTreeMap<String, usize>) -> String {
    counts
        .iter()
        .map(|(key, count)| format!("{key} ({count})"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_plan_json(plan: &Plan) -> String {
    #[derive(Serialize)]
    struct JsonCopy<'a> {
        source: &'a str,
        destination: &'a str,
        engine: &'a str,
        target_triple: &'a str,
        source_machine_key: &'a str,
        destination_machine_key: &'a str,
    }
    #[derive(Serialize)]
    struct JsonOffset<'a> {
        benchmark: &'a str,
        metric: &'a str,
        baseline_level: f64,
        incoming_level: f64,
        absolute: f64,
        relative: f64,
        beyond_tolerance: bool,
    }
    #[derive(Serialize)]
    struct JsonPair<'a> {
        baseline_machine_key: &'a str,
        incoming_machine_key: &'a str,
        interleaving: &'a str,
        blocks: usize,
        systematic_relative: Option<f64>,
        systematic_offsets: usize,
        outlying_offsets: usize,
        manufactures_step: bool,
        offsets: Vec<JsonOffset<'a>>,
    }
    #[derive(Serialize)]
    struct JsonMerge<'a> {
        engine: &'a str,
        target_triple: &'a str,
        machine_key: &'a str,
        source_machine_keys: &'a [String],
        pairs: Vec<JsonPair<'a>>,
    }
    #[derive(Serialize)]
    struct JsonCollision<'a> {
        destination: &'a str,
        sources: &'a [String],
    }
    #[derive(Serialize)]
    struct JsonTotals {
        scanned: usize,
        copies: usize,
        copied: usize,
        already_present: usize,
        already_current: usize,
        key_override: usize,
        unmapped_blessings: usize,
        missing_provenance: usize,
        unrenderable_provenance: usize,
        collisions: usize,
        unrecognized: usize,
        discriminant_sets: usize,
    }
    #[derive(Serialize)]
    struct JsonPlan<'a> {
        project: &'a str,
        target_ref: &'a str,
        apply: bool,
        totals: JsonTotals,
        copies: Vec<JsonCopy<'a>>,
        merges: Vec<JsonMerge<'a>>,
        key_overrides: Vec<JsonCountEntry<'a>>,
        unmapped_blessings: Vec<JsonCountEntry<'a>>,
        missing_provenance: &'a [String],
        unrenderable_provenance: &'a [String],
        collisions: Vec<JsonCollision<'a>>,
    }

    let copies: Vec<JsonCopy<'_>> = plan
        .copies
        .iter()
        .map(|copy| JsonCopy {
            source: &copy.source_key,
            destination: &copy.destination_key,
            engine: copy.destination_set.engine.as_str(),
            target_triple: copy.destination_set.target_triple.as_str(),
            source_machine_key: &copy.source_machine_key,
            destination_machine_key: copy.destination_set.machine_key.as_str(),
        })
        .collect();

    let merges: Vec<JsonMerge<'_>> = plan
        .merges
        .merges
        .iter()
        .map(|merge| JsonMerge {
            engine: merge.set.engine.as_str(),
            target_triple: merge.set.target_triple.as_str(),
            machine_key: merge.set.machine_key.as_str(),
            source_machine_keys: &merge.source_keys,
            pairs: merge
                .pairs
                .iter()
                .map(|pair| JsonPair {
                    baseline_machine_key: &pair.baseline_key,
                    incoming_machine_key: &pair.incoming_key,
                    interleaving: pair.interleaving.as_str(),
                    blocks: pair.blocks,
                    systematic_relative: pair.systematic.map(|systematic| systematic.relative),
                    systematic_offsets: pair.systematic.map_or(0, |systematic| systematic.offsets),
                    outlying_offsets: pair.outlying_offsets().count(),
                    manufactures_step: pair.manufactures_step,
                    offsets: pair
                        .offsets
                        .iter()
                        .map(|offset| JsonOffset {
                            benchmark: &offset.benchmark,
                            metric: offset.metric.as_str(),
                            baseline_level: offset.baseline_level,
                            incoming_level: offset.incoming_level,
                            absolute: offset.absolute,
                            relative: offset.relative,
                            beyond_tolerance: offset.beyond_tolerance,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect();

    let document = JsonPlan {
        project: &plan.project,
        target_ref: &plan.target_ref,
        apply: plan.apply,
        totals: JsonTotals {
            scanned: plan.scanned,
            copies: plan.copies.len(),
            copied: plan.copied,
            already_present: plan.already_present,
            already_current: plan.already_current,
            key_override: plan.key_override_objects(),
            unmapped_blessings: plan.unmapped_blessing_objects(),
            missing_provenance: plan.missing_provenance.len(),
            unrenderable_provenance: plan.unrenderable_provenance.len(),
            collisions: plan.collisions.len(),
            unrecognized: plan.unrecognized,
            discriminant_sets: plan.destination_sets().len(),
        },
        copies,
        merges,
        key_overrides: json_counts(&plan.key_overrides),
        unmapped_blessings: json_counts(&plan.unmapped_blessings),
        missing_provenance: &plan.missing_provenance,
        unrenderable_provenance: &plan.unrenderable_provenance,
        collisions: plan
            .collisions
            .iter()
            .map(|(destination, sources)| JsonCollision {
                destination,
                sources,
            })
            .collect(),
    };
    serde_json::to_string_pretty(&document).expect("rekey plan structures always serialize to JSON")
}

/// Turns a per-key object tally into its serializable form.
fn json_counts(counts: &BTreeMap<String, usize>) -> Vec<JsonCountEntry<'_>> {
    counts
        .iter()
        .map(|(machine_key, objects)| JsonCountEntry {
            machine_key,
            objects: *objects,
        })
        .collect()
}

/// One entry of a per-key object tally in the JSON report.
#[derive(Serialize)]
struct JsonCountEntry<'a> {
    machine_key: &'a str,
    objects: usize,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::path::PathBuf;

    use cbh_diag::RecordingReporter;
    use cbh_git::FakeGitHistory;
    use cbh_model::{
        BenchmarkId, BenchmarkResult, EnvironmentInfo, GitInfo, Metric, RunContext, ToolchainInfo,
    };
    use cbh_storage::MemoryStorage;
    use futures::executor::block_on;
    use jiff::Timestamp;
    use nonempty::nonempty;

    use super::*;

    /// The project every fixture stores under.
    const PROJECT: &str = "folo";

    /// Recorded hardware whose speed histogram is populated, so its retired and
    /// current keys differ.
    fn wobbly_machine(speed: u64) -> MachineInfo {
        let mut machine = MachineInfo {
            processors: 8,
            memory_regions: 1,
            processor_models: vec!["Test CPU 3000".to_owned()],
            processor_speeds: vec![(speed, 8)],
            fingerprint: String::new(),
        };
        machine.fingerprint = legacy_machine_key(&machine).unwrap();
        machine
    }

    /// The key the retired format stored `machine` under.
    fn legacy_of(machine: &MachineInfo) -> String {
        legacy_machine_key(machine).unwrap()
    }

    /// Recorded hardware whose speed counts sum past `usize`, as a damaged stored
    /// record can. Its fingerprint is [`clamped_key`], so an object stored under that
    /// key is exactly the object a rendering that clamped the sum would have moved.
    fn overflowing_machine() -> MachineInfo {
        let mut machine = wobbly_machine(3141);
        machine.processor_speeds.push((3141, usize::MAX));
        machine.fingerprint = clamped_key();
        machine
    }

    /// The retired key a rendering that clamped [`overflowing_machine`]'s sum to the
    /// largest representable count would produce.
    fn clamped_key() -> String {
        let mut clamped = wobbly_machine(3141);
        clamped.processor_speeds = vec![(3141, usize::MAX)];
        legacy_of(&clamped)
    }

    /// The key the current format stores `machine` under.
    fn current_of(machine: &MachineInfo) -> String {
        current_machine_key(machine)
    }

    /// Hardware recorded by a capture that ran after the key format changed, so its
    /// fingerprint is the current hash rather than the retired one.
    fn settled_machine(speed: u64) -> MachineInfo {
        let mut machine = wobbly_machine(speed);
        machine.fingerprint = current_machine_key(&machine);
        machine
    }

    /// A stored run at `commit` recording `machine` and one instruction-count
    /// measurement of `value`.
    fn run(commit: &str, machine: &MachineInfo, value: f64) -> Run {
        let mut context = RunContext::new(
            Timestamp::from_second(0).unwrap(),
            GitInfo {
                commit: Some(commit.to_owned()),
                branch: Some("master".to_owned()),
                dirty: false,
            },
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );
        context.machine = Some(machine.clone());
        Run::new(
            context,
            vec![BenchmarkResult::new(
                BenchmarkId::new(nonempty![
                    "nm".to_owned(),
                    "nm::observe".to_owned(),
                    "pull".to_owned(),
                ]),
                vec![Metric::new(MetricKind::InstructionCount, value)],
            )],
        )
    }

    /// A run measuring one benchmark per entry of `values`, so a single pair of
    /// partitions can carry a whole family of level offsets.
    fn scattered_run(commit: &str, machine: &MachineInfo, values: &[f64]) -> Run {
        let mut scattered = run(commit, machine, 0.0);
        scattered.results = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                BenchmarkResult::new(
                    BenchmarkId::new(nonempty![
                        "nm".to_owned(),
                        "nm::observe".to_owned(),
                        format!("pull{index}"),
                    ]),
                    vec![Metric::new(MetricKind::InstructionCount, *value)],
                )
            })
            .collect();
        scattered
    }

    /// The partition prefix every fixture stores under, up to the machine key.
    fn partition(machine_key: &str) -> String {
        format!("v1/{PROJECT}/objects/callgrind/x86_64-unknown-linux-gnu/{machine_key}")
    }

    fn clean_key(machine_key: &str, commit: &str) -> String {
        format!("{}/{commit}/clean.json", partition(machine_key))
    }

    fn dirty_key(machine_key: &str, commit: &str, unix: i64) -> String {
        format!("{}/{commit}/dirty-{unix}.json", partition(machine_key))
    }

    fn bless_key(machine_key: &str, commit: &str, unix: i64) -> String {
        format!("{}/{commit}/bless-{unix}.json", partition(machine_key))
    }

    fn store(storage: &MemoryStorage, key: &str, value: &Run) {
        block_on(storage.put(key, value.to_json().unwrap().as_bytes())).unwrap();
    }

    /// Stores raw bytes under `key`, for sidecars and malformed fixtures.
    fn store_raw(storage: &MemoryStorage, key: &str, bytes: &[u8]) {
        block_on(storage.put(key, bytes)).unwrap();
    }

    fn keys(storage: &MemoryStorage) -> Vec<String> {
        let mut keys = block_on(storage.list(STORAGE_VERSION)).unwrap();
        keys.sort();
        keys
    }

    fn bytes_at(storage: &MemoryStorage, key: &str) -> Vec<u8> {
        block_on(storage.get(key)).unwrap()
    }

    /// A linear history `c0 - c1 - c2 - c3` with HEAD at the tip.
    fn linear_git() -> FakeGitHistory {
        let mut git = FakeGitHistory::new();
        git.commit("c0", None)
            .commit("c1", Some("c0"))
            .commit("c2", Some("c1"))
            .commit("c3", Some("c2"))
            .branch("master", "c3")
            .head("master")
            .mark_default("master");
        git
    }

    fn dry_run_options() -> RekeyOptions {
        RekeyOptions::default()
    }

    fn apply_options() -> RekeyOptions {
        RekeyOptions {
            apply: true,
            ..RekeyOptions::default()
        }
    }

    /// Drives `rekey_with` and unwraps the rendered text report.
    fn rekey(storage: &MemoryStorage, git: &FakeGitHistory, options: &RekeyOptions) -> String {
        rekey_reported(storage, git, options, &RecordingReporter::new())
    }

    /// Drives `rekey_with` against a caller-supplied reporter.
    fn rekey_reported(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &RekeyOptions,
        reporter: &RecordingReporter,
    ) -> String {
        block_on(rekey_with(git, storage, PROJECT, options, reporter))
            .unwrap()
            .text
            .expect("rekey renders the text report by default")
    }

    /// Drives `rekey_with` expecting it to refuse, and returns the message.
    fn rekey_error(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &RekeyOptions,
    ) -> String {
        let error = block_on(rekey_with(
            git,
            storage,
            PROJECT,
            options,
            &RecordingReporter::new(),
        ))
        .expect_err("the pass was expected to refuse");
        error.to_string()
    }

    /// Drives `rekey_with` requesting only the JSON report.
    fn rekey_json(storage: &MemoryStorage, git: &FakeGitHistory, options: &RekeyOptions) -> String {
        let options = RekeyOptions {
            no_text: true,
            json: Some(PathBuf::from("report.json")),
            ..options.clone()
        };
        block_on(rekey_with(
            git,
            storage,
            PROJECT,
            &options,
            &RecordingReporter::new(),
        ))
        .unwrap()
        .json
        .expect("the JSON report was rendered for the requested path")
    }

    /// Drives `rekey_with` requesting only the Markdown report.
    fn rekey_markdown(
        storage: &MemoryStorage,
        git: &FakeGitHistory,
        options: &RekeyOptions,
    ) -> String {
        let options = RekeyOptions {
            no_text: true,
            markdown: Some(PathBuf::from("report.md")),
            ..options.clone()
        };
        block_on(rekey_with(
            git,
            storage,
            PROJECT,
            &options,
            &RecordingReporter::new(),
        ))
        .unwrap()
        .markdown
        .expect("the Markdown report was rendered for the requested path")
    }

    #[test]
    fn current_key_differs_from_legacy_key_when_speeds_are_recorded() {
        let machine = wobbly_machine(3141);
        assert_ne!(legacy_of(&machine), current_of(&machine));
    }

    #[test]
    fn speed_is_the_only_factor_the_two_formats_disagree_on() {
        // Two recordings of the same machine whose boot-time speed calibration
        // differed: the retired format forks them, the current one does not. That is
        // exactly the fragmentation the migration repairs.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        assert_ne!(legacy_of(&slow), legacy_of(&fast));
        assert_eq!(current_of(&slow), current_of(&fast));

        // Every other factor still forks both formats, so dropping the speeds did not
        // widen the bucket any further than intended.
        let mut other_model = slow.clone();
        other_model.processor_models = vec!["Test CPU 4000".to_owned()];
        assert_ne!(legacy_of(&slow), legacy_of(&other_model));
        assert_ne!(current_of(&slow), current_of(&other_model));

        let mut other_count = slow.clone();
        other_count.processors = 4;
        assert_ne!(legacy_of(&slow), legacy_of(&other_count));
        assert_ne!(current_of(&slow), current_of(&other_count));

        let mut other_regions = slow.clone();
        other_regions.memory_regions = 2;
        assert_ne!(legacy_of(&slow), legacy_of(&other_regions));
        assert_ne!(current_of(&slow), current_of(&other_regions));
    }

    #[test]
    fn a_fingerprint_matching_neither_key_format_aborts_the_pass() {
        let mut machine = wobbly_machine(3141);
        machine.fingerprint = "0000000000000000".to_owned();
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );

        let message = rekey_error(&storage, &linear_git(), &dry_run_options());
        assert!(
            message.contains("do not reproduce the fingerprint"),
            "{message}"
        );
        assert!(message.contains("0000000000000000"), "{message}");
    }

    #[test]
    fn a_run_captured_after_the_format_change_is_left_alone() {
        // The capture stamped the current hash into the fingerprint, so the retired
        // rendering can never match it. Demanding the retired rendering would reject
        // every object written since the format changed.
        let machine = settled_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&current_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(
            report.contains("Copied 0 objects"),
            "expected nothing to move: {report}"
        );
        assert!(
            report.contains("1 object already stored under the current key format"),
            "{report}"
        );
    }

    #[test]
    fn two_objects_claiming_one_destination_are_both_left_where_they_are() {
        // Two hardware renderings that hashed apart under the retired format hash
        // alike under the current one, and both partitions hold c1. One key holds one
        // object, and neither of the two is a copy of the other.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let contested = clean_key(&current_of(&slow), "c1");
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c1"),
            &run("c1", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c1"),
            &run("c1", &fast, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c2"),
            &run("c2", &fast, 100.0),
        );

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(
            report.contains("Copied 1 object"),
            "only the uncontested object moves: {report}"
        );
        assert!(
            report.contains("1 destination left because two or more distinct objects"),
            "{report}"
        );
        assert!(report.contains(&contested), "{report}");
        assert!(
            !keys(&storage).contains(&contested),
            "the contested destination stays empty"
        );
        assert!(
            keys(&storage).contains(&clean_key(&current_of(&fast), "c2")),
            "the uncontested copy still lands"
        );
    }

    #[test]
    fn a_second_apply_over_a_merged_store_does_not_re_examine_its_own_copies() {
        // The copies the first pass made are themselves stored objects, so a second
        // pass reads them back. Counting them as a third group would invent a merge
        // that the store does not face and could refuse a re-run that changes nothing.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c1"),
            &run("c1", &fast, 100.0),
        );

        _ = rekey(&storage, &linear_git(), &apply_options());
        let second = rekey(&storage, &linear_git(), &apply_options());

        assert!(
            second.contains("0 objects written, 2 objects already present"),
            "{second}"
        );
        assert!(
            second.contains("2 machine keys merge into this partition"),
            "the merge is still the same two source keys: {second}"
        );
        assert!(
            second.contains("2 objects already stored under the current key format"),
            "{second}"
        );
    }

    #[test]
    fn an_override_keyed_object_is_left_where_it_is() {
        // CI stores under a machine-pool name rather than the detected hardware hash.
        // Its recorded fingerprint is still the real hardware's, so the fingerprint
        // check passes while the key segment matches neither hash.
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key("github", "c1"),
            &run("c1", &machine, 100.0),
        );
        store(
            &storage,
            &clean_key("github", "c2"),
            &run("c2", &machine, 100.0),
        );
        store(
            &storage,
            &clean_key("azure", "c1"),
            &run("c1", &machine, 100.0),
        );

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert_eq!(
            keys(&storage),
            vec![
                clean_key("azure", "c1"),
                clean_key("github", "c1"),
                clean_key("github", "c2")
            ]
        );
        assert!(report.contains("azure (1), github (2)"), "{report}");
        assert!(
            report.contains("3 objects left under explicit machine keys"),
            "{report}"
        );
        // Every key the store returned parsed, so the report says nothing about
        // unrecognized ones.
        assert!(
            !report.contains("no storage-key format recognizes"),
            "{report}"
        );
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let machine = wobbly_machine(3141);
        let legacy = legacy_of(&machine);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy, "c1"),
            &run("c1", &machine, 100.0),
        );

        let report = rekey(&storage, &linear_git(), &dry_run_options());
        assert_eq!(keys(&storage), vec![clean_key(&legacy, "c1")]);
        assert!(report.contains("Would copy 1 object"), "{report}");
    }

    #[test]
    fn applying_copies_the_object_and_leaves_the_source_in_place() {
        let machine = wobbly_machine(3141);
        let legacy = legacy_of(&machine);
        let current = current_of(&machine);
        let storage = MemoryStorage::new();
        let source = clean_key(&legacy, "c1");
        store(&storage, &source, &run("c1", &machine, 100.0));

        let report = rekey(&storage, &linear_git(), &apply_options());
        let destination = clean_key(&current, "c1");
        assert_eq!(keys(&storage), {
            let mut expected = vec![source.clone(), destination.clone()];
            expected.sort();
            expected
        });
        assert_eq!(
            bytes_at(&storage, &destination),
            bytes_at(&storage, &source)
        );
        assert!(report.contains("Copied 1 object"), "{report}");
        assert!(report.contains("1 object written"), "{report}");
    }

    #[test]
    fn a_second_apply_writes_nothing_further() {
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );

        _ = rekey(&storage, &linear_git(), &apply_options());
        let after_first = keys(&storage);
        let report = rekey(&storage, &linear_git(), &apply_options());
        assert_eq!(keys(&storage), after_first);
        assert!(report.contains("0 objects written"), "{report}");
        assert!(
            report.contains("1 object already present from an earlier pass"),
            "{report}"
        );
    }

    #[test]
    fn every_object_kind_migrates() {
        let machine = wobbly_machine(3141);
        let legacy = legacy_of(&machine);
        let current = current_of(&machine);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy, "c1"),
            &run("c1", &machine, 100.0),
        );
        store(
            &storage,
            &dirty_key(&legacy, "c2", 300),
            &run("c2", &machine, 100.0),
        );
        store_raw(&storage, &bless_key(&legacy, "c2", 400), b"{}");

        _ = rekey(&storage, &linear_git(), &apply_options());
        let stored = keys(&storage);
        assert!(stored.contains(&clean_key(&current, "c1")), "{stored:?}");
        assert!(
            stored.contains(&dirty_key(&current, "c2", 300)),
            "{stored:?}"
        );
        assert!(
            stored.contains(&bless_key(&current, "c2", 400)),
            "{stored:?}"
        );
        assert_eq!(stored.len(), 6, "{stored:?}");
    }

    #[test]
    fn a_merge_with_a_large_systematic_level_offset_is_refused() {
        // Two speed buckets of the "same" machine whose measurement levels are far
        // apart. Merging them would splice a step change into the series.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c1"),
            &run("c1", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c2"),
            &run("c2", &fast, 200.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 200.0),
        );

        let message = rekey_error(&storage, &linear_git(), &apply_options());
        assert!(
            message.contains("systematically disagree beyond the merge tolerance"),
            "{message}"
        );
        assert!(message.contains("time-blocked"), "{message}");
        // The refusal names the pair and the systematic distance that decided it,
        // together with the evidence behind that number.
        assert!(
            message.contains(&format!("{} -> {}", legacy_of(&slow), legacy_of(&fast))),
            "{message}"
        );
        assert!(
            message.contains("the two sit +100% apart across 1 shared offset"),
            "{message}"
        );
        assert!(message.contains("--allow-level-shift"), "{message}");
        // One pair of partitions makes one blocking entry, which the message names in
        // full rather than summarizing.
        assert!(!message.contains("more"), "{message}");
        assert_eq!(keys(&storage).len(), 4, "no object may be written");
    }

    #[test]
    fn a_refusal_naming_every_blocking_pair_would_bury_its_guidance() {
        // Four speed buckets of one machine make six merging pairs — more than the
        // message names individually — so the tail is summarized instead of pushing
        // the remedy out of sight.
        assert_eq!(
            REFUSAL_DETAIL_LIMIT, 5,
            "the fixture is sized to produce one more pair than the message names"
        );
        let storage = MemoryStorage::new();
        for (speed, commit, level) in [
            (3141, "c0", 100.0),
            (3142, "c1", 200.0),
            (3143, "c2", 400.0),
            (3144, "c3", 800.0),
        ] {
            let machine = wobbly_machine(speed);
            store(
                &storage,
                &clean_key(&legacy_of(&machine), commit),
                &run(commit, &machine, level),
            );
        }

        let message = rekey_error(&storage, &linear_git(), &apply_options());
        assert!(message.contains("(and 1 pair more)"), "{message}");
        assert!(message.contains("--allow-level-shift"), "{message}");
    }

    #[test]
    fn per_benchmark_scatter_around_zero_does_not_refuse_the_merge() {
        // Six benchmarks whose individual offsets run from -5% to +5%, symmetric about
        // zero: the family as a whole did not move, so a merge cannot splice a step in
        // however far the individual benchmarks wander. A gate reading each benchmark
        // separately would refuse this, and refusing it is what teaches an operator to
        // always pass --allow-level-shift.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &scattered_run("c0", &slow, &[1_000.0; 6]),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &scattered_run(
                "c3",
                &fast,
                &[1_050.0, 950.0, 1_040.0, 960.0, 1_030.0, 970.0],
            ),
        );

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(
            report.contains("merge verdict: clear — the two sit +0% apart across 6 shared offsets"),
            "{report}"
        );
        assert!(
            report.contains("6 offsets beyond the merge tolerance individually"),
            "{report}"
        );
        assert!(
            report.contains("1000 -> 1050 (+50, +5%)  [beyond tolerance, informational]"),
            "{report}"
        );
        assert!(report.contains("Copied 2 objects"), "{report}");
    }

    #[test]
    fn the_same_scatter_shifted_as_a_family_is_refused() {
        // The scatter of the previous fixture with a systematic +10% laid over it. The
        // spread is identical; only the family's common move differs, and that alone
        // decides.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &scattered_run("c0", &slow, &[1_000.0; 6]),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &scattered_run(
                "c3",
                &fast,
                &[1_150.0, 1_050.0, 1_140.0, 1_060.0, 1_130.0, 1_070.0],
            ),
        );

        let message = rekey_error(&storage, &linear_git(), &apply_options());
        assert!(
            message.contains("the two sit +10% apart across 6 shared offsets"),
            "{message}"
        );
        assert_eq!(keys(&storage).len(), 2, "no object may be written");
    }

    #[test]
    fn a_merge_whose_offsets_cannot_be_resolved_reports_no_systematic_distance() {
        // Every benchmark moved by two instruction counts: beyond the relative
        // tolerance at this level, but too small a move to mean anything, so there is
        // nothing a splice could turn into a step and nothing to report a distance
        // from either.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &scattered_run("c0", &slow, &[100.0; 4]),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &scattered_run("c3", &fast, &[102.0; 4]),
        );

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(
            report.contains(
                "merge verdict: clear — no shared offset is a large enough move to read, so \
                 there is no distance to judge"
            ),
            "{report}"
        );
        assert!(
            report.contains("0 offsets beyond the merge tolerance individually"),
            "{report}"
        );
        assert!(report.contains("Copied 2 objects"), "{report}");
    }

    #[test]
    fn an_interleaved_merge_with_no_level_offset_proceeds() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c1"),
            &run("c1", &fast, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c2"),
            &run("c2", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 100.0),
        );

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(report.contains("interleaved"), "{report}");
        // A zero offset reads as `+0`, not `-0`: the sign states the direction of a
        // move, and no move is not a downward one.
        assert!(report.contains("100 -> 100 (+0, +0%)"), "{report}");
        assert!(report.contains("Copied 4 objects"), "{report}");
        assert_eq!(keys(&storage).len(), 8);
    }

    #[test]
    fn a_refused_merge_proceeds_under_allow_level_shift() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 200.0),
        );

        let options = RekeyOptions {
            allow_level_shift: true,
            ..apply_options()
        };
        let report = rekey(&storage, &linear_git(), &options);
        // The pair still reads as blocked, so a report produced under the override names
        // exactly what the operator has taken responsibility for.
        assert!(
            report.contains(
                "merge verdict: blocked — the two sit +100% apart across 1 shared offset"
            ),
            "{report}"
        );
        assert_eq!(keys(&storage).len(), 4);
    }

    #[test]
    fn the_reported_offset_and_interleaving_describe_the_fixture() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        // The earlier group measures 100; the later one 101. One instruction count is
        // too small a move to say anything about a level, so the pair reports no
        // distance and does not block.
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 101.0),
        );

        let report = rekey(&storage, &linear_git(), &dry_run_options());
        assert!(report.contains("100 -> 101 (+1, +1%)"), "{report}");
        assert!(report.contains("time-blocked over 2 blocks"), "{report}");
        assert!(
            !report.contains("beyond tolerance, informational"),
            "{report}"
        );
    }

    #[test]
    fn the_baseline_is_the_group_history_reaches_first() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c0"),
            &run("c0", &fast, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c3"),
            &run("c3", &slow, 99.0),
        );

        let report = rekey(&storage, &linear_git(), &dry_run_options());
        assert!(
            report.contains(&format!("{} -> {}", legacy_of(&fast), legacy_of(&slow))),
            "{report}"
        );
        assert!(report.contains("100 -> 99 (-1, -1%)"), "{report}");
    }

    #[test]
    fn an_object_already_under_the_current_key_is_left_alone_but_still_compared() {
        // A partially migrated store: one group already carries the current key, so
        // the merge assessment must still weigh it against the group joining it.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let current = current_of(&slow);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&current, "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 400.0),
        );

        let message = rekey_error(&storage, &linear_git(), &dry_run_options());
        assert!(message.contains("beyond the merge tolerance"), "{message}");

        let report = rekey(
            &storage,
            &linear_git(),
            &RekeyOptions {
                allow_level_shift: true,
                ..dry_run_options()
            },
        );
        assert!(
            report.contains("1 object already stored under the current key format"),
            "{report}"
        );
    }

    #[test]
    fn a_run_without_recorded_hardware_is_reported_and_skipped() {
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        let mut orphan = run("c1", &machine, 100.0);
        orphan.context.machine = None;
        store(&storage, &clean_key("m1", "c1"), &orphan);

        let reporter = RecordingReporter::new();
        let report = rekey_reported(&storage, &linear_git(), &apply_options(), &reporter);
        assert_eq!(keys(&storage), vec![clean_key("m1", "c1")]);
        assert!(
            report.contains("1 run left without recorded hardware provenance"),
            "{report}"
        );
        assert!(report.contains(&clean_key("m1", "c1")), "{report}");
        let notes = reporter.notes().join("\n");
        assert!(
            notes.contains("it records no hardware provenance"),
            "{notes}"
        );
    }

    #[test]
    fn a_run_whose_hardware_does_not_render_is_reported_and_skipped() {
        // The speed counts are read back from the object rather than probed, so a
        // damaged record can carry a histogram that sums past `usize`. This one sits
        // under the very key a rendering that clamped the sum would produce, so a
        // clamping renderer would read the segment as a retired hardware hash and copy
        // the object — into the partition of whatever machine really has that
        // histogram. Facts that do not render prove nothing, so the run stays put.
        let storage = MemoryStorage::new();
        let segment = clamped_key();
        store(
            &storage,
            &clean_key(&segment, "c1"),
            &run("c1", &overflowing_machine(), 100.0),
        );

        let (report, json) = (
            rekey(&storage, &linear_git(), &apply_options()),
            rekey_json(&storage, &linear_git(), &dry_run_options()),
        );
        assert_eq!(keys(&storage), vec![clean_key(&segment, "c1")]);
        assert!(
            report.contains(
                "1 run left with hardware provenance that does not render under the retired \
                 machine-key format"
            ),
            "{report}"
        );
        assert!(report.contains(&clean_key(&segment, "c1")), "{report}");

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["totals"]["unrenderable_provenance"], 1);
        assert_eq!(value["totals"]["missing_provenance"], 0);
        assert_eq!(
            value["unrenderable_provenance"][0],
            clean_key(&segment, "c1").as_str()
        );

        let reporter = RecordingReporter::new();
        _ = rekey_reported(&storage, &linear_git(), &dry_run_options(), &reporter);
        let notes = reporter.notes().join("\n");
        assert!(
            notes.contains("nothing under the retired one, which cannot render them"),
            "{notes}"
        );
        assert!(
            notes.contains("does not render under the retired machine-key format"),
            "{notes}"
        );
    }

    #[test]
    fn unrenderable_hardware_already_under_the_current_key_is_still_compared() {
        // The current key format does not read the speed histogram, so hardware that
        // fails to render under the retired one still hashes to a current key, and a
        // segment equal to that key proves itself without any retired hash. Such a run
        // needs no migration — but it is the incumbent of the partition another one
        // merges into, so dropping it would leave the merge with a single source and
        // no pair to judge, silently disarming the level-shift gate.
        let mut incumbent = overflowing_machine();
        incumbent.fingerprint = current_of(&incumbent);
        let joining = wobbly_machine(3142);
        let current = current_of(&incumbent);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&current, "c0"),
            &run("c0", &incumbent, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&joining), "c3"),
            &run("c3", &joining, 400.0),
        );

        let message = rekey_error(&storage, &linear_git(), &dry_run_options());
        assert!(message.contains("beyond the merge tolerance"), "{message}");

        let report = rekey(
            &storage,
            &linear_git(),
            &RekeyOptions {
                allow_level_shift: true,
                ..dry_run_options()
            },
        );
        assert!(
            report.contains("1 object already stored under the current key format"),
            "{report}"
        );
    }

    #[test]
    fn a_blessing_sidecar_no_run_maps_is_reported_and_skipped() {
        let storage = MemoryStorage::new();
        store_raw(&storage, &bless_key("orphan", "c1", 400), b"{}");
        store_raw(&storage, &bless_key("orphan", "c2", 500), b"{}");
        store_raw(&storage, &bless_key("stray", "c1", 600), b"{}");

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert_eq!(
            keys(&storage),
            vec![
                bless_key("orphan", "c1", 400),
                bless_key("orphan", "c2", 500),
                bless_key("stray", "c1", 600)
            ]
        );
        assert!(
            report.contains("3 blessing sidecars left under machine keys no stored run maps"),
            "{report}"
        );
        assert!(report.contains("orphan (2), stray (1)"), "{report}");
    }

    #[test]
    fn a_destination_holding_different_bytes_aborts_the_pass() {
        // The destination measures the same level — so the merge assessment passes —
        // but records a different tool version, so its bytes are not a copy of the
        // source's and overwriting it would destroy a distinct record.
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );
        let mut impostor = run("c1", &machine, 100.0);
        impostor.context.tool_version = "9.9.9".to_owned();
        store(&storage, &clean_key(&current_of(&machine), "c1"), &impostor);

        let message = rekey_error(&storage, &linear_git(), &apply_options());
        assert!(
            message.contains("already holds a different object"),
            "{message}"
        );
    }

    #[test]
    fn a_key_the_store_returns_that_is_not_a_storage_key_is_counted() {
        let storage = MemoryStorage::new();
        store_raw(&storage, "v1/folo/objects/stray.json", b"{}");

        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(
            report
                .contains("skipped 1 key the store returned that no storage-key format recognizes"),
            "{report}"
        );
        assert_eq!(
            keys(&storage),
            vec!["v1/folo/objects/stray.json".to_owned()]
        );
    }

    #[test]
    fn an_unresolvable_context_leaves_the_interleaving_unknown() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 100.0),
        );

        let options = RekeyOptions {
            context: Some("nowhere".to_owned()),
            ..dry_run_options()
        };
        let report = rekey(&storage, &linear_git(), &options);
        assert!(report.contains("unknown over 0 blocks"), "{report}");
    }

    #[test]
    fn the_verbose_trail_explains_each_decision() {
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );
        store(
            &storage,
            &clean_key("github", "c1"),
            &run("c1", &machine, 100.0),
        );

        let reporter = RecordingReporter::new();
        _ = rekey_reported(&storage, &linear_git(), &dry_run_options(), &reporter);
        let notes = reporter.notes().join("\n");
        assert!(notes.contains("processors=8"), "{notes}");
        assert!(notes.contains("Test CPU 3000"), "{notes}");
        assert!(notes.contains("3141x8"), "{notes}");
        assert!(
            notes.contains("under the current key format and"),
            "{notes}"
        );
        assert!(notes.contains("explicit machine-key override"), "{notes}");
        assert!(notes.contains("HEAD resolves to c3"), "{notes}");
        assert!(notes.contains("merge tolerance derived from"), "{notes}");
        assert!(notes.contains("re-run with --apply"), "{notes}");
    }

    #[test]
    fn the_announcement_summarizes_the_pass() {
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );

        let reporter = RecordingReporter::new();
        _ = rekey_reported(&storage, &linear_git(), &dry_run_options(), &reporter);
        let announcements = reporter.announcements().join("\n");
        assert!(
            announcements.contains("rekey: project folo"),
            "{announcements}"
        );
        assert!(announcements.contains("ordered by HEAD"), "{announcements}");
        assert!(
            announcements.contains("1 object to copy"),
            "{announcements}"
        );
    }

    #[test]
    fn the_json_report_carries_the_plan() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &scattered_run("c0", &slow, &[1_000.0; 3]),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &scattered_run("c3", &fast, &[1_010.0; 3]),
        );
        store(
            &storage,
            &clean_key("github", "c1"),
            &run("c1", &slow, 100.0),
        );

        let json = rekey_json(&storage, &linear_git(), &dry_run_options());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["project"], "folo");
        assert_eq!(value["apply"], false);
        assert_eq!(value["totals"]["copies"], 2);
        assert_eq!(value["totals"]["key_override"], 1);
        assert_eq!(value["key_overrides"][0]["machine_key"], "github");
        assert_eq!(value["merges"][0]["machine_key"], current_of(&slow));
        let pair = &value["merges"][0]["pairs"][0];
        assert_eq!(pair["interleaving"], "time-blocked");
        assert_eq!(pair["blocks"], 2);
        // Every offset is a move of ten counts, well clear of the metric's absolute
        // floor, so all three carry the systematic reading; none reaches the relative
        // tolerance, so none is an outlier and the merge stands.
        assert_eq!(pair["systematic_relative"], 0.01);
        assert_eq!(pair["systematic_offsets"], 3);
        assert_eq!(pair["outlying_offsets"], 0);
        assert_eq!(pair["manufactures_step"], false);
        let offset = &pair["offsets"][0];
        assert_eq!(offset["metric"], "instruction_count");
        assert_eq!(offset["baseline_level"], 1000.0);
        assert_eq!(offset["incoming_level"], 1010.0);
        assert_eq!(offset["absolute"], 10.0);
        assert_eq!(offset["beyond_tolerance"], false);
        assert_eq!(
            value["copies"][0]["destination_machine_key"],
            current_of(&slow)
        );
    }

    #[test]
    fn the_json_report_states_a_pair_that_blocks_the_merge() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &scattered_run("c0", &slow, &[1_000.0; 3]),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &scattered_run("c3", &fast, &[1_100.0, 1_100.0, 1_000.0]),
        );

        let options = RekeyOptions {
            allow_level_shift: true,
            ..dry_run_options()
        };
        let json = rekey_json(&storage, &linear_git(), &options);
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let pair = &value["merges"][0]["pairs"][0];
        // The third benchmark did not move, so it carries no information and stays out
        // of the median; the two that did agree on +10%.
        assert_eq!(pair["systematic_relative"], 0.1);
        assert_eq!(pair["systematic_offsets"], 2);
        assert_eq!(pair["outlying_offsets"], 2);
        assert_eq!(pair["manufactures_step"], true);
    }

    #[test]
    fn the_json_report_states_an_unreadable_systematic_offset_as_null() {
        // Two counts apart at a level of a hundred: beyond the relative tolerance but
        // too small a move to say anything about a level, so there is no distance to
        // report rather than a measured zero.
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &run("c0", &slow, 100.0),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &run("c3", &fast, 102.0),
        );

        let json = rekey_json(&storage, &linear_git(), &dry_run_options());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let pair = &value["merges"][0]["pairs"][0];
        assert!(pair["systematic_relative"].is_null(), "{json}");
        assert_eq!(pair["systematic_offsets"], 0);
        assert_eq!(pair["manufactures_step"], false);
    }

    #[test]
    fn the_markdown_report_carries_the_plan() {
        let slow = wobbly_machine(3141);
        let fast = wobbly_machine(3142);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&slow), "c0"),
            &scattered_run("c0", &slow, &[1_000.0, 1_000.0, 1_000.0]),
        );
        store(
            &storage,
            &clean_key(&legacy_of(&fast), "c3"),
            &scattered_run("c3", &fast, &[1_003.0, 1_003.0, 1_100.0]),
        );

        let markdown = rekey_markdown(&storage, &linear_git(), &dry_run_options());
        assert!(markdown.contains("# Rekey plan for folo"), "{markdown}");
        assert!(
            markdown.contains(
                "| Benchmark | Metric | Baseline | Incoming | Offset | Relative | Beyond \
                 tolerance |"
            ),
            "{markdown}"
        );
        assert!(
            markdown.contains(
                "**Merge verdict: clear** — the two sit +0.3% apart across 3 shared offsets. 1 \
                 offset beyond the merge tolerance individually, which is informational only."
            ),
            "{markdown}"
        );
        assert!(markdown.contains("| +3 | +0.3% | no |"), "{markdown}");
        assert!(markdown.contains("| +100 | +10% | yes |"), "{markdown}");
        assert!(
            markdown.contains("**Would copy 2 objects into 1 discriminant set**"),
            "{markdown}"
        );
    }

    #[test]
    fn a_single_source_partition_is_not_a_merge() {
        let machine = wobbly_machine(3141);
        let storage = MemoryStorage::new();
        store(
            &storage,
            &clean_key(&legacy_of(&machine), "c1"),
            &run("c1", &machine, 100.0),
        );

        let json = rekey_json(&storage, &linear_git(), &dry_run_options());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["merges"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn an_empty_store_is_a_no_op() {
        let storage = MemoryStorage::new();
        let report = rekey(&storage, &linear_git(), &apply_options());
        assert!(report.contains("Copied 0 objects"), "{report}");
        assert!(keys(&storage).is_empty());
    }

    #[test]
    fn a_run_that_is_not_a_valid_result_set_aborts_the_pass() {
        let storage = MemoryStorage::new();
        store_raw(&storage, &clean_key("m1", "c1"), b"not json");

        let message = rekey_error(&storage, &linear_git(), &dry_run_options());
        assert!(message.contains("is not a valid result set"), "{message}");
    }
}
