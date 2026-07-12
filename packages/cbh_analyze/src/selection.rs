//! The data-set `Selection` the analyze/list/prune/examine/bless query commands
//! share, built from each command's options.

use cbh_command::{
    AnalyzeOptions, BlessOptions, ExamineOptions, ListOptions, PruneOptions, UnblessOptions,
};

/// The data-set selection parameters shared by the query commands: which stored
/// objects to consider (facets + `--since`) and how to resolve the git timeline
/// (`--repo` is resolved by the caller into the [`GitHistory`] adapter;
/// `--context` / `--base` / `--no-dirty` steer the topology query). Analyze's
/// benchmark-prefix scope is deliberately *not* here: it filters which series are
/// built, not which runs load.
///
/// Each facet (`engine` / `target_triple` / `machine_key`) carries the raw,
/// repeatable command-line values; [`resolve_facets`] turns them into
/// [`FacetFilter`]s, applying the current-machine auto-detect default and the
/// `all` keyword.
pub(crate) struct Selection<'a> {
    pub(crate) context: Option<&'a str>,
    pub(crate) base: Option<&'a str>,
    pub(crate) no_dirty: bool,
    pub(crate) since: Option<&'a str>,
    pub(crate) engine: &'a [String],
    pub(crate) target_triple: &'a [String],
    pub(crate) machine_key: &'a [String],
}

impl<'a> Selection<'a> {
    pub(crate) fn from_analyze(options: &'a AnalyzeOptions) -> Self {
        Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
        }
    }

    pub(crate) fn from_list(options: &'a ListOptions) -> Self {
        Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
        }
    }

    pub(crate) fn from_examine(options: &'a ExamineOptions) -> Self {
        Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            no_dirty: options.no_dirty,
            since: options.since.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
        }
    }

    pub(crate) fn from_prune(options: &'a PruneOptions) -> Self {
        Self {
            context: options.context.as_deref(),
            base: options.base.as_deref(),
            // `prune` resolves the data set with dirty admission always on; the
            // base-tip exception is applied unconditionally (see
            // `DirtyTipPolicy::Always`), and the per-object scope (`--dirty` /
            // `--clean`) decides which runs are actually removed.
            no_dirty: false,
            since: options.since.as_deref(),
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
        }
    }

    /// Selection facets for `bless`. Only the discriminant facets (and `base`)
    /// matter: a blessing always acts at the current commit, so it has no
    /// `context` / `since` / topology selectors.
    pub(crate) fn from_bless(options: &'a BlessOptions) -> Self {
        Self {
            context: None,
            base: options.base.as_deref(),
            no_dirty: false,
            since: None,
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
        }
    }

    /// Selection facets for `unbless`. Mirrors [`from_bless`](Self::from_bless).
    pub(crate) fn from_unbless(options: &'a UnblessOptions) -> Self {
        Self {
            context: None,
            base: options.base.as_deref(),
            no_dirty: false,
            since: None,
            engine: &options.engine,
            target_triple: &options.target_triple,
            machine_key: &options.machine_key,
        }
    }
}
